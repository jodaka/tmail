//! Tmail — application entry point.
//!
//! Wires config → backend → operation manager → event loop. The reducer
//! stays I/O-free: state transitions register operations and return
//! [`Effect`]s; the operation manager ([`OperationManager`]) spawns the
//! typed backend requests with their cancellation tokens, and results flow
//! back through the task result channel into the same reducer as
//! `Action::BackendCompleted` (plan §11).
//!
//! The account configuration wizard (ADR 0003) joins the wiring: it
//! triggers on first run (no usable account) or via `tmail --configure`,
//! reusing the same loop, renderer, event stream, and manager, with the
//! discoverer injected like the backend and opener.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use chrono::Local;
use tokio::sync::mpsc;

use tmail::app::{Action, AppState, Effect, OperationResult, reducer};
use tmail::backend::{
    MailBackend, Notifier, PathOpener, RequestContext, SystemNotifier, SystemOpener,
    himalaya::HimalayaCliBackend,
};
use tmail::discovery::{EmailConfigDiscoverer, FakeDiscoverer, PimDiscoverer};
use tmail::input::mouse;
use tmail::runtime::tasks::OperationManager;
use tmail::runtime::{events, logging, terminal};
use tmail::ui::dates::format_clock;
use tmail::ui::{RenderContext, Theme};

/// What the command line asked for. Flags may appear in any order:
/// `--configure [path]` starts the wizard regardless of config state
/// (ADR 0003 §3.1), `--theme <name>` overrides the configured palette,
/// and the positional argument is the config path. Any other `-`
/// argument is a hard usage error. `--version` short-circuits the run
/// (packagers probe it), so it composes with any other flags.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    configure: bool,
    config: Option<PathBuf>,
    theme: Option<String>,
    version: bool,
}

const USAGE: &str = "usage: tmail [--version] [--configure [path]] [--theme <name>] [config.toml]";

fn parse_invocation() -> Result<Invocation, String> {
    parse_args(std::env::args().skip(1))
}

fn parse_args<I>(args: I) -> Result<Invocation, String>
where
    I: IntoIterator<Item = String>,
{
    let mut invocation = Invocation {
        configure: false,
        config: None,
        theme: None,
        version: false,
    };
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" => invocation.version = true,
            "--configure" => {
                invocation.configure = true;
                // The optional path value: only when it does not look
                // like the next flag (which stays for the loop to
                // classify).
                if let Some(next) = args.peek()
                    && !next.starts_with('-')
                {
                    let path = args.next().expect("value peeked above");
                    set_positional(&mut invocation, PathBuf::from(path))?;
                }
            }
            "--theme" => {
                let name = match args.peek() {
                    Some(next) if !next.starts_with('-') => {
                        args.next().expect("value peeked above")
                    }
                    _ => return Err(String::from("--theme requires a theme name")),
                };
                if invocation.theme.is_some() {
                    return Err(String::from("--theme given more than once"));
                }
                invocation.theme = Some(name);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag {other}"));
            }
            other => {
                set_positional(&mut invocation, PathBuf::from(other))?;
            }
        }
    }
    Ok(invocation)
}

/// The single positional argument is the config path; a second one is a
/// usage error.
fn set_positional(invocation: &mut Invocation, path: PathBuf) -> Result<(), String> {
    if invocation.config.is_some() {
        return Err(String::from("too many arguments"));
    }
    invocation.config = Some(path);
    Ok(())
}

fn main() -> ExitCode {
    let _logging = logging::init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "tmail starting");

    let invocation = match parse_invocation() {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("tmail: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    // Version probe before any runtime or config work: packagers and
    // users get a stable, side-effect-free answer even on a box with a
    // broken config.
    if invocation.version {
        println!("tmail {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("tmail: failed to start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(invocation)) {
        Ok(code) => code,
        Err(err) => {
            tracing::error!(%err, "fatal error");
            // Terminal restoration happens in TerminalGuard's Drop; this
            // message lands after the alternate screen is left.
            eprintln!("tmail: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// One session's exit reason (ADR 0003 §3.2 W7): after the wizard saves a
/// first-run account, the app re-runs configuration and enters the normal
/// mailbox UI without restarting the process.
enum SessionOutcome {
    Exit(ExitCode),
    /// The wizard completed (first-run): reload the config and start the
    /// normal UI.
    Restart,
}

async fn run(invocation: Invocation) -> anyhow::Result<ExitCode> {
    loop {
        match session(&invocation).await? {
            SessionOutcome::Exit(code) => return Ok(code),
            SessionOutcome::Restart => continue,
        }
    }
}

async fn session(invocation: &Invocation) -> anyhow::Result<SessionOutcome> {
    let (config, keymap) = load_validated_config(invocation)?;

    // First-run trigger (ADR 0003 §3.1): the resolved config yields no
    // drivable account — no file found anywhere, or a file whose
    // `[accounts]` table is missing or empty. A file that exists with
    // accounts is never hijacked, even the multi-account-without-default
    // case (startup issues explain it, as today).
    let wizard_needed = invocation.configure
        || match &config.path {
            None => true,
            Some(path) => !tmail::config::accounts_present(path),
        };

    let backend: Arc<dyn MailBackend> = Arc::new(HimalayaCliBackend::from_config(&config));
    // Platform open-with adapter for saved attachments (plan §15).
    let opener: Arc<dyn PathOpener> = Arc::new(SystemOpener);
    // New-mail notification adapter (ticket b28p): the terminal bell or a
    // notify-rust desktop notification. Injected like the opener, and
    // delivered off the UI thread by the operation manager.
    let notifier: Arc<dyn Notifier> = Arc::new(SystemNotifier);
    // The email settings discoverer (ADR 0003 §3.3): injected like the
    // backend and opener. `TMAIL_FAKE_DISCOVERY=1` selects the canned
    // fake — tests and smoke runs never touch the network.
    let discoverer: Arc<dyn EmailConfigDiscoverer> =
        if std::env::var_os("TMAIL_FAKE_DISCOVERY").is_some() {
            Arc::new(FakeDiscoverer)
        } else {
            Arc::new(PimDiscoverer)
        };
    // Tmail-owned summary cache (ticket haeb): instant warm starts, the
    // fresh page always loads in the background afterwards. Owned by the
    // operation manager — cache reads and writes are file I/O and run on
    // the blocking pool, never on the reducer thread.
    let page_cache = tmail::app::page_cache::PageCache::open_default(
        config.account.as_deref(),
        tmail::app::page_cache::CacheLimits {
            max_messages: config.cache.max_messages,
            max_bytes: config.cache.max_bytes,
        },
    );

    // The events stream starts before the first draw; the terminal guard
    // and the title cache ride in `SessionAssets` so effect handling stays
    // a short argument list. Mouse capture is opt-in (`[tmail].mouse`,
    // plan §10): with capture off, terminal text selection keeps its native
    // behavior and no mouse events arrive at all. The guard is optional:
    // `None` only while the external editor owns the terminal (Phase 11).
    let (mut events, events_control) = events::spawn();
    let mut assets = SessionAssets {
        guard: Some(terminal::enable(config.mouse)?),
        events_control,
        applied_title: String::new(),
    };
    let mut state = seed_state(&config, keymap, invocation.theme.as_deref())?;
    tracing::info!(size = ?state.session.size, "shell started (real backend)");

    // Backend results re-enter the reducer as actions; the manager spawns
    // one cancellable task per effect.
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    let manager = OperationManager::new(
        Arc::clone(&backend),
        opener,
        notifier,
        discoverer,
        String::from("himalaya"),
        page_cache,
        result_tx,
    );

    if wizard_needed {
        start_wizard(&mut state, invocation);
    } else {
        // Startup work flows through the same reducer path as everything
        // else: with no mailboxes loaded yet, Refresh starts the mailbox
        // listing; LoadDrafts restores any crash-safe draft from the
        // journal (plan §14).
        let effects = reducer::reduce(&mut state, Action::Refresh);
        handle_effects(&mut state, &manager, &mut assets, effects).await?;
        let effects = reducer::reduce(&mut state, Action::LoadDrafts);
        handle_effects(&mut state, &manager, &mut assets, effects).await?;
    }

    // Loader pacing: the event loop's tick cadence flips only while a
    // foreground operation keeps the scanner animating.
    let events_pace = assets.events_control.clone();

    run_event_loop(
        &mut state,
        &manager,
        &mut assets,
        &mut events,
        &mut result_rx,
        &config,
        &events_pace,
    )
    .await?;

    // Terminal restoration (guarded Drop) leaves the window title set;
    // hand the session's original title back (best-effort, popped from
    // the title stack pushed by `enable`).
    terminal::restore_title();
    Ok(finish_session(&state, &config))
}

/// The terminal and event-reader assets the loop owns between effects: the
/// terminal guard (dropped while the external editor runs), the event
/// reader's pause/resume control, and the last terminal title applied.
struct SessionAssets {
    guard: Option<terminal::TerminalGuard>,
    events_control: events::EventControl,
    applied_title: String,
}

/// Load and validate the config, returning it with the built keymap.
/// Startup validation reports every detected problem together, before the
/// TUI starts: a broken config is fixed in the file, not navigated in the
/// app (plan §17/§19 Phase 10). Details are actionable and sanitized;
/// secrets never reach these messages. `--configure` starts the wizard
/// regardless of config state (ADR 0003 §3.1) — config issues are the
/// wizard's job there — but the himalaya check stays: the credential test
/// cannot run without it.
fn load_validated_config(
    invocation: &Invocation,
) -> anyhow::Result<(tmail::config::Config, tmail::input::keymap::KeyMap)> {
    let (config, issues) = tmail::config::Config::load_with_issues(invocation.config.as_deref());
    let mut issues: Vec<String> = if invocation.configure {
        Vec::new()
    } else {
        issues
    };
    if !tmail::backend::himalaya::executable_available("himalaya") {
        issues.push(String::from(
            "the himalaya executable was not found on PATH; install it or point PATH at it",
        ));
    }
    // The keymap is built here (not inside the config parser): conflicts
    // and structural checks are policy, not syntax, and their warnings
    // describe adjustments — the app still starts (configurable
    // keybindings; syntax problems come back fatal, like theme tokens).
    let tmail::input::keymap::KeymapBuild {
        keymap,
        errors,
        warnings,
    } = tmail::input::keymap::KeyMap::build(&config.keybindings);
    if !invocation.configure {
        issues.extend(errors);
    }
    if !issues.is_empty() {
        let mut message = String::from("configuration problems (fix the file, then start again):");
        for issue in &issues {
            message.push_str(&format!("\n  - {issue}"));
        }
        bail!("{}", message);
    }
    // Warnings print before the alternate screen swallows stderr: a
    // refused conflict or restored default must be visible somewhere.
    for warning in &warnings {
        eprintln!("tmail: warning: {warning}");
        tracing::warn!(warning, "keybinding config adjusted");
    }
    tracing::info!(
        config = ?config.path,
        account = ?config.account,
        page_size = config.mail.page_size,
        "configuration loaded"
    );
    Ok((config, keymap))
}

/// Build the runtime state from the validated config: the set-once knobs,
/// the runtime-switchable theme list (NO_COLOR wins over all of it, plan
/// §18), and the `--theme` override.
fn seed_state(
    config: &tmail::config::Config,
    keymap: tmail::input::keymap::KeyMap,
    requested_theme: Option<&str>,
) -> anyhow::Result<AppState> {
    let mut state = AppState::initial(config.mail.page_size);
    // Runtime-switchable theme list (ticket z0s4): the two built-ins —
    // the `[tmail.theme]` selection with its color overrides (ticket wrs7)
    // landing on the startup entry — plus every `[tmail.themes.<name>]`
    // table. NO_COLOR wins over all of it (plan §18): every palette
    // becomes monochrome, so switching stays a harmless no-op.
    let (mut themes, mut theme_index) = Theme::theme_list(
        &config.theme.name,
        &config.theme.overrides,
        &config.theme.tables,
    );
    if Theme::no_color_requested() {
        for (_, theme) in &mut themes {
            *theme = Theme::monochrome();
        }
    }
    // `--theme <name>` overrides the configured selection (feedback:
    // `tmail --configure --theme light`). Validated against the built
    // list, so `[tmail.themes.<name>]` tables are selectable too; an
    // unknown name is a startup error naming the alternatives.
    if let Some(name) = requested_theme {
        theme_index = themes
            .iter()
            .position(|(candidate, _)| candidate == name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown theme {name:?}; available: {}",
                    themes
                        .iter()
                        .map(|(candidate, _)| candidate.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
    }
    // The set-once config knobs collapse into one construction (the
    // per-field plumbing moved here when `Settings` was grouped): the
    // configured keymap replaces the defaults-only seed; reply-all excludes
    // the account address (Phase 7.5); the refresh timer (Phase 9.4,
    // `0` disables), draft autosave debounce (Phase 10.4), list density
    // (Gmail-style separators), status fade window (ticket h1d7), external
    // editor argv (Phase 11.4, `None` = builtin), and initial mouse capture
    // (Phase 10.4) all come straight from the file.
    state.settings = tmail::app::Settings {
        keymap,
        account_email: config.account_email.clone(),
        refresh_interval_seconds: config.mail.refresh_interval_seconds,
        page_size_auto: config.mail.page_size_auto,
        autosave_delay_ms: config.composer.autosave_delay_ms,
        view_mode: config.view_mode,
        status_timeout_seconds: config.status_timeout,
        notifications: config.notifications,
        editor_command: config.composer.editor_command.clone(),
        mouse_capture: config.mouse,
        themes,
        theme_index,
    };
    if let Ok((width, height)) = crossterm::terminal::size() {
        state.session.size = (width, height);
    }
    // Ticket kjfq: `page_size_auto` sizes each page to the number of
    // message rows the terminal can show, so the page fits the list
    // without scrolling; manual pagination keeps `[tmail.mail].mail.page_size`.
    // The view mode decides how many lines a message costs.
    if config.mail.page_size_auto {
        state.messages.limit =
            tmail::ui::layout::messages_visible(state.session.size, state.settings.view_mode)
                .max(1);
    }
    Ok(state)
}

/// Start the account configuration wizard (ADR 0003 §3.1): it replaces the
/// first `Refresh`/`LoadDrafts` warmup — while its route is active the
/// reducer suppresses mailbox warmup and swallows mailbox keys. The file
/// snapshots (existing account names, default holder, permissions) happen
/// here — I/O stays out of the reducer.
fn start_wizard(state: &mut AppState, invocation: &Invocation) {
    let save_path = tmail::config::default_save_path(invocation.config.as_deref());
    let (existing_names, default_name, shared_readable) = save_path
        .as_deref()
        .map(|path| {
            (
                tmail::config::write::existing_account_names(path),
                tmail::config::write::file_default_account(path),
                tmail::config::write::file_shared_readable(path),
            )
        })
        .unwrap_or_else(|| (Vec::new(), None, false));
    state.session.wizard = Some(tmail::app::wizard::WizardState::new(
        invocation.configure,
        tmail::app::wizard::ConfigSnapshot {
            save_path,
            existing_names,
            existing_default_name: default_name,
            existing_shared_readable: shared_readable,
        },
    ));
    state.session.routes.push(tmail::app::route::Route::Wizard);
    state.session.focus = tmail::app::Focus::Wizard;
}

/// The main loop: draw a frame, then wait for one batch of input or one
/// backend result, applying effects between frames. Returns when the user
/// quits or the event stream closes.
async fn run_event_loop(
    state: &mut AppState,
    manager: &OperationManager,
    assets: &mut SessionAssets,
    events: &mut mpsc::UnboundedReceiver<events::Event>,
    result_rx: &mut mpsc::UnboundedReceiver<OperationResult>,
    config: &tmail::config::Config,
    events_pace: &events::EventControl,
) -> anyhow::Result<()> {
    // Mouse capture starts in the configured mode; the reducer owns the
    // intent as `state.settings.mouse_capture`, and the runtime applies any change.
    let mut capture_applied = config.mouse;
    let mut fast_ticks = false;
    loop {
        // Title sync ahead of the draw: the reducer may have changed the
        // mode (open the composer/wizard, switch mailboxes, unread tick).
        let title = state.terminal_title();
        if title != assets.applied_title {
            if let Err(err) =
                crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(&title))
            {
                tracing::warn!(%err, "could not set the terminal title");
            }
            assets.applied_title = title;
        }
        let now = Local::now().fixed_offset();
        // The top-right clock is config-gated and off by default (ticket
        // w7f5): an empty label renders nothing.
        let clock_label = if config.ui.clock {
            format_clock(now)
        } else {
            String::new()
        };
        let ctx = RenderContext::new(now, clock_label);
        // The hit map of the frame currently on screen: mouse events are
        // hit-tested against exactly what the user sees (plan §10).
        let mut hits = mouse::HitMap::default();
        // The palette the reducer last selected (ticket z0s4): `t` swaps
        // it at runtime, and the next frame picks it up from state.
        let theme = state.active_theme();
        assets
            .guard
            .as_mut()
            .expect("terminal guard alive while drawing")
            .terminal_mut()
            .draw(|frame| tmail::ui::render(frame, state, &theme, &ctx, &mut hits))
            .context("terminal draw failed")?;

        if state.session.quit_requested {
            tracing::info!("quit requested; leaving event loop");
            break;
        }

        // Test hook: `TMAIL_INDUCE_PANIC=1` panics after the first draw to
        // prove panic-safe terminal restoration (plan §19 Phase 1).
        if std::env::var_os("TMAIL_INDUCE_PANIC").is_some() {
            panic!("induced panic: TMAIL_INDUCE_PANIC is set (restoration test)");
        }

        tokio::select! {
            event = events.recv() => match event {
                Some(first) => {
                    // Drain what already queued behind this event (a
                    // touchpad gesture arrives as a burst of wheel events):
                    // the whole batch dispatches between two frames, so a
                    // keypress is never stuck behind wheel events, and the
                    // burst costs one draw instead of one per event. The
                    // cap bounds a single frame's work; leftovers drain on
                    // the following frames.
                    let mut batch = Vec::with_capacity(8);
                    batch.push(first);
                    while batch.len() < events::MAX_BATCH {
                        match events.try_recv() {
                            Ok(next) => batch.push(next),
                            Err(_) => break,
                        }
                    }
                    for action in events::coalesce(batch, &hits, state) {
                        tracing::debug!(?action, "dispatch");
                        let effects = reducer::reduce(state, action);
                        handle_effects(state, manager, assets, effects).await?;
                        sync_mouse_capture(state, &mut capture_applied);
                    }
                    pace_loader(state, events_pace, &mut fast_ticks);
                }
                None => {
                    tracing::warn!("event stream closed");
                    break;
                }
            },
            action = result_rx.recv() => match action {
                Some(result) => {
                    let effects = reducer::reduce(state, Action::BackendCompleted(result));
                    handle_effects(state, manager, assets, effects).await?;
                    sync_mouse_capture(state, &mut capture_applied);
                    pace_loader(state, events_pace, &mut fast_ticks);
                }
                // The manager holds a sender for the whole session.
                None => bail!("backend result channel closed unexpectedly"),
            },
        }
    }
    Ok(())
}

/// Map the post-loop state to the session outcome (ADR 0003 §3.1):
/// `--configure` completion prints the saved path and exits 0; an
/// `Esc`-cancel exits 1 with "configuration not changed". A first-run
/// completion restarts into the normal mailbox UI without leaving the
/// process.
fn finish_session(state: &AppState, config: &tmail::config::Config) -> SessionOutcome {
    if let Some(wizard) = &state.session.wizard {
        if wizard.completed {
            if wizard.manual {
                if let Some(path) = wizard
                    .confirm
                    .saved_path
                    .clone()
                    .or_else(|| config.path.clone())
                {
                    println!("{}", path.display());
                }
                return SessionOutcome::Exit(ExitCode::SUCCESS);
            }
            return SessionOutcome::Restart;
        }
        if wizard.cancelled {
            eprintln!("tmail: configuration not changed");
            return SessionOutcome::Exit(ExitCode::from(1));
        }
    }
    SessionOutcome::Exit(ExitCode::SUCCESS)
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
    assets: &mut SessionAssets,
    effects: Vec<Effect>,
) -> anyhow::Result<()> {
    for effect in effects {
        if let tmail::app::operation::OperationKind::EditExternally { program, body } =
            effect.kind.clone()
        {
            // Suspend: drop the guard (its Drop restores the terminal) and
            // pause the event reader so it cannot steal the editor's
            // keystrokes (plan §14 steps 2 and 5).
            assets.events_control.pause();
            drop(assets.guard.take().expect("terminal guard to suspend"));
            let mouse = state.settings.mouse_capture;
            let result = tmail::runtime::editor::run(&program, &body).await;
            // Step 7: restore the terminal even on editor failure —
            // unconditionally, before anything else runs. The fresh guard
            // repaints from scratch on the next draw.
            assets.guard = Some(terminal::reenter(mouse).context("terminal resume failed")?);
            assets.events_control.resume();
            // The title that is on screen is the editor's business: forget
            // the applied one, so the next frame re-asserts Tmail's.
            assets.applied_title = String::new();
            let effects = reducer::reduce(
                state,
                Action::EditorFinished {
                    id: effect.id,
                    result,
                },
            );
            // The import's follow-up save is a plain backend effect; the
            // editor flow itself cannot re-enter here.
            for effect in effects {
                launch_one(manager, state, effect);
            }
        } else {
            launch_one(manager, state, effect);
        }
    }
    Ok(())
}

/// Launch one non-editor effect. `EditExternally` must never pass here: the
/// external editor runs on the terminal owner in [`handle_effects`] — if
/// one reaches this point it is a wiring bug, so it is logged and dropped
/// instead of spawned like a backend call.
fn launch_one(manager: &OperationManager, state: &AppState, effect: Effect) {
    if matches!(
        effect.kind,
        tmail::app::operation::OperationKind::EditExternally { .. }
    ) {
        tracing::warn!(id = %effect.id, "external editor effect reached the manager; dropped");
        return;
    }
    let Some(token) = state.session.operations.cancellation(effect.id) else {
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
    if state.settings.mouse_capture != *applied {
        if let Err(err) = terminal::set_mouse_capture(state.settings.mouse_capture) {
            tracing::warn!(%err, "failed to switch mouse capture");
        }
        *applied = state.settings.mouse_capture;
        tracing::debug!(
            mouse_capture = state.settings.mouse_capture,
            "mouse capture switched"
        );
    }
}

/// Match the event loop's tick cadence to the loader: fast ticks only
/// while a foreground operation keeps the scanner animating. `applied`
/// mirrors the cadence the event loop currently runs, so a call is a
/// no-op unless the pace actually changed.
fn pace_loader(state: &AppState, control: &events::EventControl, applied: &mut bool) {
    let fast = state.session.operations.foreground().is_some();
    if fast != *applied {
        control.set_fast_ticks(fast);
        *applied = fast;
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn invocation(values: &[&str]) -> Invocation {
        parse_args(args(values)).expect("parses")
    }

    #[test]
    fn no_arguments_is_a_plain_run() {
        let invocation = invocation(&[]);
        assert!(!invocation.configure);
        assert_eq!(invocation.config, None);
        assert_eq!(invocation.theme, None);
    }

    #[test]
    fn positional_config_path() {
        let invocation = invocation(&["path/to/config.toml"]);
        assert!(!invocation.configure);
        assert_eq!(
            invocation.config,
            Some(PathBuf::from("path/to/config.toml"))
        );
    }

    #[test]
    fn configure_without_and_with_path() {
        let bare = invocation(&["--configure"]);
        assert!(bare.configure);
        assert_eq!(bare.config, None);

        let with_path = invocation(&["--configure", "cfg.toml"]);
        assert!(with_path.configure);
        assert_eq!(with_path.config, Some(PathBuf::from("cfg.toml")));
    }

    #[test]
    fn theme_flag_accepts_a_name_in_any_order() {
        let after = invocation(&["--configure", "--theme", "light"]);
        assert!(after.configure);
        assert_eq!(after.theme.as_deref(), Some("light"));
        assert_eq!(after.config, None, "--theme's value is not a config path");

        let before = invocation(&["--theme", "light", "--configure"]);
        assert!(before.configure);
        assert_eq!(before.theme.as_deref(), Some("light"));

        let with_path = invocation(&["--configure", "cfg.toml", "--theme", "light"]);
        assert_eq!(with_path.config, Some(PathBuf::from("cfg.toml")));
        assert_eq!(with_path.theme.as_deref(), Some("light"));

        let plain = invocation(&["--theme", "default"]);
        assert!(!plain.configure);
        assert_eq!(plain.theme.as_deref(), Some("default"));
    }

    #[test]
    fn version_flag_is_recognized_alone_and_in_company() {
        let bare = invocation(&["--version"]);
        assert!(bare.version);

        let composed = invocation(&["--version", "--configure", "cfg.toml"]);
        assert!(composed.version, "--version composes with other flags");
        assert!(composed.configure);
        assert_eq!(composed.config, Some(PathBuf::from("cfg.toml")));

        let trailing = invocation(&["--theme", "light", "--version"]);
        assert!(trailing.version);
    }

    #[test]
    fn version_defaults_to_false() {
        assert!(!invocation(&[]).version);
        assert!(!invocation(&["--configure"]).version);
    }

    #[test]
    fn unknown_flags_are_usage_errors() {
        assert!(parse_args(args(&["--bogus"])).is_err());
        assert!(parse_args(args(&["--configure", "--bogus"])).is_err());
    }

    #[test]
    fn theme_without_a_value_is_a_usage_error() {
        assert!(parse_args(args(&["--theme"])).is_err());
        assert!(parse_args(args(&["--theme", "--configure"])).is_err());
    }

    #[test]
    fn repeated_flags_and_second_positional_are_usage_errors() {
        assert!(parse_args(args(&["--theme", "light", "--theme", "dark"])).is_err());
        assert!(parse_args(args(&["a.toml", "b.toml"])).is_err());
        assert!(parse_args(args(&["--configure", "a.toml", "b.toml"])).is_err());
    }
}
