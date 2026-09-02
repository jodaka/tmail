//! Post — application entry point (Phase 2: real backend vertical slice).
//!
//! Wires config → backend → event loop. The reducer stays I/O-free: state
//! transitions return [`Effect`]s, this runtime spawns the typed backend
//! requests, and their results flow back through the result channel into
//! the same reducer. The Phase 3 operation manager replaces this minimal
//! dispatch loop with the operation registry, cancellation, and the
//! Retry/Dismiss modal.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use chrono::Local;
use tokio::sync::mpsc;

use tmail::app::{Action, AppState, Effect, reducer};
use tmail::backend::{MailBackend, himalaya::HimalayaCliBackend};
use tmail::config::Config;
use tmail::input::keyboard;
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

    // Backend results re-enter the reducer as actions.
    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<Action>();
    dispatch(&backend, &[Effect::LoadMailboxes], &result_tx);

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
                        dispatch(&backend, &effects, &result_tx);
                    }
                }
                Some(events::Event::Resize { width, height }) => {
                    let effects = reducer::reduce(
                        &mut state,
                        &Action::Resize { width, height },
                    );
                    dispatch(&backend, &effects, &result_tx);
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
                Some(action) => {
                    let effects = reducer::reduce(&mut state, &action);
                    dispatch(&backend, &effects, &result_tx);
                }
                // The runtime holds a sender for the whole session.
                None => bail!("backend result channel closed unexpectedly"),
            },
        }
    }
    Ok(())
}

/// Spawn one task per effect; results re-enter the loop as actions.
fn dispatch(
    backend: &Arc<dyn MailBackend>,
    effects: &[Effect],
    result_tx: &mpsc::UnboundedSender<Action>,
) {
    for effect in effects {
        match effect {
            Effect::LoadMailboxes => {
                let backend = Arc::clone(backend);
                let result_tx = result_tx.clone();
                tokio::spawn(async move {
                    let result = backend
                        .list_mailboxes()
                        .await
                        .map_err(|err| err.to_string());
                    let _ = result_tx.send(Action::MailboxesLoaded(result));
                });
            }
            Effect::LoadPage(request) => {
                let backend = Arc::clone(backend);
                let result_tx = result_tx.clone();
                let request = request.clone();
                tokio::spawn(async move {
                    let result = backend
                        .list_messages(request.clone())
                        .await
                        .map_err(|err| err.to_string());
                    let _ = result_tx.send(Action::PageLoaded { request, result });
                });
            }
        }
    }
}
