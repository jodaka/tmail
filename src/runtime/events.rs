//! Crossterm event stream + periodic ticks, delivered as a typed stream
//! (plan §19 Phase 1: `runtime/events.rs`).

use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, KeyEvent, KeyEventKind, MouseEvent};
use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Tick cadence. The clock only shows minutes; 250 ms keeps future spinner
/// animation smooth without busy-waiting.
pub const TICK_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    /// Click or wheel event (Phase 10, plan §10). Arrives only while mouse
    /// capture is enabled (`[post].mouse`).
    Mouse(MouseEvent),
    Resize {
        width: u16,
        height: u16,
    },
    Tick,
}

/// Control commands for the running event loop.
enum Control {
    /// Stop reading input events and ticks: the terminal is about to be
    /// handed to a child program (the external editor, plan §14 Phase
    /// 11.2), and a concurrent reader would steal its keystrokes.
    Pause,
    /// Resume normal event delivery.
    Resume,
}

/// Handle over the running event loop's lifecycle (Phase 11.5): pause
/// while the external editor owns the terminal, resume after it exits.
#[derive(Clone)]
pub struct EventControl {
    tx: UnboundedSender<Control>,
}

impl EventControl {
    pub fn pause(&self) {
        let _ = self.tx.send(Control::Pause);
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Control::Resume);
    }
}

/// Spawn the event reader task; returns the receiving end together with
/// the loop's control handle. The task ends when the sender is dropped
/// (i.e. when this receiver goes away).
pub fn spawn() -> (UnboundedReceiver<Event>, EventControl) {
    let (tx, rx) = unbounded_channel();
    let (control_tx, control_rx) = unbounded_channel();
    tokio::spawn(event_loop(tx, control_rx));
    (rx, EventControl { tx: control_tx })
}

async fn event_loop(tx: UnboundedSender<Event>, mut control: UnboundedReceiver<Control>) {
    let mut reader = crossterm::event::EventStream::new();
    let mut tick = tokio::time::interval(TICK_INTERVAL);
    // While paused (external editor owns the terminal) ticks are not
    // consumed; delay mode prevents a burst firing on resume.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut paused = false;
    loop {
        tokio::select! {
            command = control.recv() => match command {
                Some(Control::Pause) => paused = true,
                Some(Control::Resume) => paused = false,
                None => break,
            },
            // Both input arms are disabled while paused: the child program
            // owns stdin, and its keystrokes must reach it untouched.
            _ = tick.tick(), if !paused => {
                if tx.send(Event::Tick).is_err() {
                    break;
                }
            }
            event = reader.next(), if !paused => {
                match event {
                    Some(Ok(CrosstermEvent::Key(key))) => {
                        // Only real presses; repeats/releases would double-fire.
                        if key.kind == KeyEventKind::Press && tx.send(Event::Key(key)).is_err() {
                            break;
                        }
                    }
                    Some(Ok(CrosstermEvent::Resize(width, height))) => {
                        if tx
                            .send(Event::Resize { width, height })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(CrosstermEvent::Mouse(mouse))) => {
                        // Phase 10: clicks and wheel scroll forward into
                        // the same action vocabulary; motion/drag events
                        // are dropped (v1 has no hover or selection).
                        if matches!(
                            mouse.kind,
                            crossterm::event::MouseEventKind::Down(_)
                                | crossterm::event::MouseEventKind::Up(_)
                                | crossterm::event::MouseEventKind::ScrollUp
                                | crossterm::event::MouseEventKind::ScrollDown
                        ) && tx.send(Event::Mouse(mouse)).is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // Focus changes and gestures have no v1 behavior.
                    }
                    Some(Err(err)) => {
                        tracing::warn!(%err, "crossterm event stream error");
                        break;
                    }
                    None => break,
                }
            }
        }
    }
}
