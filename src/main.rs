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
    // events arrive at all. The guard is optional: `None` only while the
    // external editor owns the terminal (Phase 11).
    let mut guard = Some(terminal::enable(config.mouse)?);
    let mut state = AppState::initial(config.page_size);
    // Reply-all excludes the configured account address (Phase 7.5).
    state.account_email = config.account_email.clone();
    // Periodic refresh timer (Phase 9.4); `0` disables it.
    state.refresh_interval_seconds = config.refresh_interval_seconds;
    // Draft autosave debounce (Phase 10.4 wiring of
    // `[post.composer].autosave_delay_ms`).
    state.autosave_delay_ms = config.autosave_delay_ms;
    // The external editor argv (Phase 11.4); `None` = builtin editor.
    state.editor_command = config.editor_command.clone();
    // Post-owned summary cache (ticket haeb): instant warm starts, the
    // fresh page always loads in the background afterwards.
    state.page_cache = tmail::app::page_cache::PageCache::open_default(
        config.account.as_deref(),
        tmail::app::page_cache::CacheLimits {
            max_messages: config.cache_max_messages,
            max_bytes: config.cache_max_bytes,
        },
    );
    // Mouse capture starts in the configured mode (Phase 10.4); `m`
    // toggles it at runtime via `Action::ToggleMouseCapture`.
    state.mouse_capture = config.mouse;
    if let Ok((width, height)) = crossterm::terminal::size() {
        state.size = (width, height);
    }
    // Ticket kjfq: `page_size_auto` sizes each page to the number of
    // message rows the terminal can show, so the page fits the list
    // without scrolling; manual pagination keeps `[post.mail].page_size`.
    state.page_size_auto = config.page_size_auto;
    if config.page_size_auto {
        state.messages.limit = tmail::ui::layout::message_rows_visible(state.size).max(1);
    }
    tracing::info!(size = ?state.size, "shell started (real backend)");

    let (mut events, events_control) = events::spawn();
    // `[post.theme].name` plus any `[post.theme]` color overrides (ticket
    // wrs7), falling back to plain terminal colors when the environment
    // asks for no color (plan §18) — NO_COLOR wins over custom colors.
    let theme = if Theme::no_color_requested() {
        Theme::monochrome()
    } else {
        let mut theme = Theme::from_name(&config.theme_name);
        for (token, hex) in &config.theme_overrides {
            // Validation guarantees known tokens and valid hex; a stale
            // parse would only skip the override, never crash startup.
            if let Some(color) =
                tmail::config::parse_hex_color(hex).and_then(|hex| Theme::color_from_hex(&hex))
            {
                theme.set_token(token, color);
            }
        }
        theme
    };

    // Backend results re-enter the reducer as actions; the manager spawns
    // one cancellable task per effect.
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    let manager = OperationManager::new(Arc::clone(&backend), opener, result_tx);
    // The capture mode the terminal is currently in; the reducer owns the
    // intent as `state.mouse_capture`, and the runtime applies any change.
    let mut capture_applied = config.mouse;

    // Startup work flows through the same reducer path as everything else:
    // with no mailboxes loaded yet, Refresh starts the mailbox listing;
    // LoadDrafts restores any crash-safe draft from the journal (plan §14).
    let effects = reducer::reduce(&mut state, &Action::Refresh);
    handle_effects(&mut state, &manager, &mut guard, &events_control, effects).await?;
    let effects = reducer::reduce(&mut state, &Action::LoadDrafts);
    handle_effects(&mut state, &manager, &mut guard, &events_control, effects).await?;

    loop {
        let now = Local::now().fixed_offset();
        // The top-right clock is config-gated and off by default (ticket
        // w7f5): an empty label renders nothing.
        let clock_label = if config.ui_clock {
            format_clock(now)
        } else {
            String::new()
        };
        let ctx = RenderContext::new(now, clock_label);
        // The hit map of the frame currently on screen: mouse events are
        // hit-tested against exactly what the user sees (plan §10).
        let mut hits = mouse::HitMap::default();
        guard
            .as_mut()
            .expect("terminal guard alive while drawing")
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
                        handle_effects(&mut state, &manager, &mut guard, &events_control, effects).await?;
                        sync_mouse_capture(&state, &mut capture_applied);
                    }
                }
                Some(events::Event::Mouse(mouse_event)) => {
                    // Phase 10: hit-test against the frame on screen and
                    // dispatch the same actions the keyboard produces.
                    if let Some(action) = mouse::to_action(mouse_event, &hits, &state) {
                        tracing::debug!(?action, "mouse dispatch");
                        let effects = reducer::reduce(&mut state, &action);
                        handle_effects(&mut state, &manager, &mut guard, &events_control, effects).await?;
                        sync_mouse_capture(&state, &mut capture_applied);
                    }
                }
                Some(events::Event::Resize { width, height }) => {
                    let effects = reducer::reduce(
                        &mut state,
                        &Action::Resize { width, height },
                    );
                    handle_effects(&mut state, &manager, &mut guard, &events_control, effects).await?;
                    sync_mouse_capture(&state, &mut capture_applied);
                }
                Some(events::Event::Tick) => {
                    let effects = reducer::reduce(
                        &mut state,
                        &Action::Tick {
                            now: Box::new(Local::now().fixed_offset()),
                        },
                    );
                    handle_effects(&mut state, &manager, &mut guard, &events_control, effects).await?;
                    sync_mouse_capture(&state, &mut capture_applied);
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
                    handle_effects(&mut state, &manager, &mut guard, &events_control, effects).await?;
                    sync_mouse_capture(&state, &mut capture_applied);
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
/// exactly that work. The `EditExternally` effect never reaches the
/// manager: the external editor must run on the thread that owns the
/// terminal (plan §14, Phase 11), so it is awaited inline by the caller —
/// `handle_effects` below.
fn launch(manager: &OperationManager, state: &AppState, effects: Vec<Effect>) {
    for effect in effects {
        if matches!(
            effect.kind,
            tmail::app::operation::OperationKind::EditExternally { .. }
        ) {
            tracing::warn!(id = %effect.id, "external editor effect reached the manager; dropped");
            continue;
        }
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

/// Route one batch of reducer effects: async backend operations go to the
/// manager; the external editor runs here, synchronously, on the terminal
/// owner (plan §14 steps 2–7, Phase 11): pause the event reader so it
/// cannot steal the editor's keystrokes, suspend the TUI, run the editor
/// (the runtime stays live, so the pre-launch draft save completes), then
/// always resume — success or failure (step 7) — and force a full redraw.
async fn handle_effects(
    state: &mut AppState,
    manager: &OperationManager,
    guard: &mut Option<terminal::TerminalGuard>,
    events_control: &events::EventControl,
    effects: Vec<Effect>,
) -> anyhow::Result<()> {
    for effect in effects {
        if let tmail::app::operation::OperationKind::EditExternally { program, body } =
            effect.kind.clone()
        {
            // Suspend: drop the guard (its Drop restores the terminal) and
            // pause the event reader so it cannot steal the editor's
            // keystrokes (plan §14 steps 2 and 5).
            events_control.pause();
            drop(guard.take().expect("terminal guard to suspend"));
            let mouse = state.mouse_capture;
            let result = tmail::runtime::editor::run(&program, &body).await;
            // Step 7: restore the terminal even on editor failure —
            // unconditionally, before anything else runs. The fresh guard
            // repaints from scratch on the next draw.
            *guard = Some(terminal::reenter(mouse).context("terminal resume failed")?);
            events_control.resume();
            let effects = reducer::reduce(
                state,
                &Action::EditorFinished {
                    id: effect.id,
                    result,
                },
            );
            // The import's follow-up save is a plain backend effect; the
            // editor flow itself cannot re-enter here.
            launch(manager, state, effects);
        } else {
            launch_one(manager, state, effect);
        }
    }
    Ok(())
}

/// Launch one non-editor effect.
fn launch_one(manager: &OperationManager, state: &AppState, effect: Effect) {
    let Some(token) = state.operations.cancellation(effect.id) else {
        tracing::warn!(id = %effect.id, "effect without a registered operation");
        return;
    };
    let ctx = RequestContext {
        operation: effect.id,
        cancellation: token,
    };
    manager.launch(effect, ctx);
}

/// Keep the terminal's mouse-capture mode in sync with the mode the
/// reducer owns (plan §10 feedback: `m` toggles capture at runtime). The
/// reducer stays I/O-free; this is where the terminal actually changes.
fn sync_mouse_capture(state: &AppState, applied: &mut bool) {
    if state.mouse_capture != *applied {
        if let Err(err) = terminal::set_mouse_capture(state.mouse_capture) {
            tracing::warn!(%err, "failed to switch mouse capture");
        }
        *applied = state.mouse_capture;
        tracing::debug!(
            mouse_capture = state.mouse_capture,
            "mouse capture switched"
        );
    }
}
