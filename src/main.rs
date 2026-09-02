//! Post — application entry point.
//!
//! Wires config → backend → operation manager → event loop. The reducer
//! stays I/O-free: state transitions register operations and return
//! [`Effect`]s; the operation manager ([`OperationManager`]) spawns the
//! typed backend requests with their cancellation tokens, and results flow
//! back through the task result channel into the same reducer as
//! `Action::BackendCompleted` (plan §11).

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use chrono::Local;
use tokio::sync::mpsc;

use tmail::app::{Action, AppState, Effect, reducer};
use tmail::backend::{MailBackend, RequestContext, himalaya::HimalayaCliBackend};
use tmail::config::Config;
use tmail::input::keyboard;
use tmail::runtime::tasks::OperationManager;
use tmail::runtime::{events, logging, terminal};
use tmail::ui::dates::format_clock;
use tmail::ui::{RenderContext, Theme};

fn main() -> ExitCode {
    let _logging = logging::init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "post starting");

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("post: failed to start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(%err, "fatal error");
            // Terminal restoration happens in TerminalGuard's Drop; this
            // message lands after the alternate screen is left.
            eprintln!("post: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    // Optional explicit config path: `post [path/to/config.toml]`. Without
    // one, `POST_CONFIG` or the well-known himalaya locations are used.
    let cli_config = std::env::args().nth(1).map(std::path::PathBuf::from);
    let config = Config::load(cli_config.as_deref());
    tracing::info!(
        config = ?config.path,
        account = ?config.account,
        page_size = config.page_size,
        "configuration loaded"
    );
    let backend: Arc<dyn MailBackend> = Arc::new(HimalayaCliBackend::from_config(&config));

    let mut guard = terminal::enable()?;
    let mut state = AppState::initial(config.page_size);
    if let Ok((width, height)) = crossterm::terminal::size() {
        state.size = (width, height);
    }
    tracing::info!(size = ?state.size, "shell started (real backend)");

    let mut events = events::spawn();
    let theme = Theme::default_dark();

    // Backend results re-enter the reducer as actions; the manager spawns
    // one cancellable task per effect.
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    let manager = OperationManager::new(Arc::clone(&backend), result_tx);

    // Startup work flows through the same reducer path as everything else:
    // with no mailboxes loaded yet, Refresh starts the mailbox listing.
    let effects = reducer::reduce(&mut state, &Action::Refresh);
    launch(&manager, &state, effects);

    loop {
        let now = Local::now().fixed_offset();
        let ctx = RenderContext::new(now, format_clock(now));
        guard
            .terminal_mut()
            .draw(|frame| tmail::ui::render(frame, &state, &theme, &ctx))
            .context("terminal draw failed")?;

        if state.quit_requested {
            tracing::info!("quit requested; leaving event loop");
            break;
        }

        // Test hook: `POST_INDUCE_PANIC=1` panics after the first draw to
        // prove panic-safe terminal restoration (plan §19 Phase 1).
        if std::env::var_os("POST_INDUCE_PANIC").is_some() {
            panic!("induced panic: POST_INDUCE_PANIC is set (restoration test)");
        }

        tokio::select! {
            event = events.recv() => match event {
                Some(events::Event::Key(key)) => {
                    if let Some(action) = keyboard::to_action(key, state.focus) {
                        tracing::debug!(?action, "dispatch");
                        let effects = reducer::reduce(&mut state, &action);
                        launch(&manager, &state, effects);
                    }
                }
                Some(events::Event::Resize { width, height }) => {
                    let effects = reducer::reduce(
                        &mut state,
                        &Action::Resize { width, height },
                    );
                    launch(&manager, &state, effects);
                }
                Some(events::Event::Tick) => {
                    reducer::reduce(&mut state, &Action::Tick);
                }
                None => {
                    tracing::warn!("event stream closed");
                    break;
                }
            },
            action = result_rx.recv() => match action {
                Some(result) => {
                    let effects = reducer::reduce(
                        &mut state,
                        &Action::BackendCompleted(result),
                    );
                    launch(&manager, &state, effects);
                }
                // The manager holds a sender for the whole session.
                None => bail!("backend result channel closed unexpectedly"),
            },
        }
    }
    Ok(())
}

/// Launch the effects a state transition produced: each one gets its
/// cancellation token from the registry, so a later `Esc` can cancel
/// exactly that work.
fn launch(manager: &OperationManager, state: &AppState, effects: Vec<Effect>) {
    for effect in effects {
        let Some(token) = state.operations.cancellation(effect.id) else {
            tracing::warn!(id = %effect.id, "effect without a registered operation");
            continue;
        };
        let ctx = RequestContext {
            operation: effect.id,
            cancellation: token,
        };
        manager.launch(effect, ctx);
    }
}
