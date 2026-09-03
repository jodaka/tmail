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
use tmail::backend::{
    MailBackend, PathOpener, RequestContext, SystemOpener, himalaya::HimalayaCliBackend,
};
use tmail::input::{keyboard, mouse};
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
    let (config, issues) = tmail::config::Config::load_with_issues(cli_config.as_deref());
    // Startup validation reports every detected problem together, before
    // the TUI starts: a broken config is fixed in the file, not navigated
    // in the app (plan §17/§19 Phase 10). Details are actionable and
    // sanitized; secrets never reach these messages.
    let mut issues: Vec<String> = issues.iter().cloned().collect();
    if !tmail::backend::himalaya::executable_available("himalaya") {
        issues.push(String::from(
            "the himalaya executable was not found on PATH; install it or point PATH at it",
        ));
    }
    if !issues.is_empty() {
        let mut message = String::from("configuration problems (fix the file, then start again):");
        for issue in &issues {
            message.push_str(&format!("\n  - {issue}"));
        }
        bail!("{}", message);
    }
    tracing::info!(
        config = ?config.path,
        account = ?config.account,
        page_size = config.page_size,
        "configuration loaded"
    );
    let backend: Arc<dyn MailBackend> = Arc::new(HimalayaCliBackend::from_config(&config));
    // Platform open-with adapter for saved attachments (plan §15).
    let opener: Arc<dyn PathOpener> = Arc::new(SystemOpener);

    // Mouse capture is opt-in (`[post].mouse`, plan §10): with capture off,
    // terminal text selection keeps its native behavior and no mouse
    // events arrive at all.
    let mut guard = terminal::enable(config.mouse)?;
    let mut state = AppState::initial(config.page_size);
    // Reply-all excludes the configured account address (Phase 7.5).
    state.account_email = config.account_email.clone();
    // Periodic refresh timer (Phase 9.4); `0` disables it.
    state.refresh_interval_seconds = config.refresh_interval_seconds;
    // Draft autosave debounce (Phase 10.4 wiring of
    // `[post.composer].autosave_delay_ms`).
    state.autosave_delay_ms = config.autosave_delay_ms;
    if let Ok((width, height)) = crossterm::terminal::size() {
        state.size = (width, height);
    }
    tracing::info!(size = ?state.size, "shell started (real backend)");

    let mut events = events::spawn();
    // `[post.theme].name`, falling back to plain terminal colors when the
    // environment asks for no color (plan §18).
    let theme = if Theme::no_color_requested() {
        Theme::monochrome()
    } else {
        Theme::from_name(&config.theme_name)
    };

    // Backend results re-enter the reducer as actions; the manager spawns
    // one cancellable task per effect.
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    let manager = OperationManager::new(Arc::clone(&backend), opener, result_tx);

    // Startup work flows through the same reducer path as everything else:
    // with no mailboxes loaded yet, Refresh starts the mailbox listing;
    // LoadDrafts restores any crash-safe draft from the journal (plan §14).
    let effects = reducer::reduce(&mut state, &Action::Refresh);
    launch(&manager, &state, effects);
    let effects = reducer::reduce(&mut state, &Action::LoadDrafts);
    launch(&manager, &state, effects);

    loop {
        let now = Local::now().fixed_offset();
        let ctx = RenderContext::new(now, format_clock(now));
        // The hit map of the frame currently on screen: mouse events are
        // hit-tested against exactly what the user sees (plan §10).
        let mut hits = mouse::HitMap::default();
        guard
            .terminal_mut()
            .draw(|frame| tmail::ui::render(frame, &state, &theme, &ctx, &mut hits))
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
                Some(events::Event::Mouse(mouse_event)) => {
                    // Phase 10: hit-test against the frame on screen and
                    // dispatch the same actions the keyboard produces.
                    if let Some(action) = mouse::to_action(mouse_event, &hits, &state) {
                        tracing::debug!(?action, "mouse dispatch");
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
                    let effects = reducer::reduce(
                        &mut state,
                        &Action::Tick {
                            now: Box::new(Local::now().fixed_offset()),
                        },
                    );
                    launch(&manager, &state, effects);
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
