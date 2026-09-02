//! Post — application entry point (Phase 1 shell).
//!
//! Wires the event stream into the reducer and the renderer. No Himalaya
//! calls here: Phase 1 runs on mock data (plan §19 Phase 1).

use std::process::ExitCode;

use chrono::Local;
use tmail::app::{mock, reducer};
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
    let mut guard = terminal::enable()?;
    let mut state = mock::mock_initial_state();
    if let Ok((width, height)) = crossterm::terminal::size() {
        state.size = (width, height);
    }
    tracing::info!(size = ?state.size, "shell started (mock data)");

    let mut events = events::spawn();
    let theme = Theme::default_dark();

    loop {
        let now = Local::now().fixed_offset();
        let ctx = RenderContext::new(now, format_clock(now));
        guard
            .terminal_mut()
            .draw(|frame| tmail::ui::render(frame, &state, &theme, &ctx))?;

        if state.quit_requested {
            tracing::info!("quit requested; leaving event loop");
            break;
        }

        // Test hook: `POST_INDUCE_PANIC=1` panics after the first draw to
        // prove panic-safe terminal restoration (plan §19 Phase 1).
        if std::env::var_os("POST_INDUCE_PANIC").is_some() {
            panic!("induced panic: POST_INDUCE_PANIC is set (restoration test)");
        }

        match events.recv().await {
            Some(events::Event::Key(key)) => {
                if let Some(action) = keyboard::to_action(key, state.focus) {
                    tracing::debug!(?action, "dispatch");
                    reducer::reduce(&mut state, &action);
                }
            }
            Some(events::Event::Resize { width, height }) => {
                reducer::reduce(&mut state, &tmail::app::Action::Resize { width, height });
            }
            Some(events::Event::Tick) => reducer::reduce(&mut state, &tmail::app::Action::Tick),
            None => {
                tracing::warn!("event stream closed");
                break;
            }
        }
    }
    Ok(())
}
