//! Crossterm event stream + periodic ticks, delivered as a typed stream
//! (plan §19 Phase 1: `runtime/events.rs`).

use std::time::Duration;

use chrono::Local;
use crossterm::event::{
    Event as CrosstermEvent, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::app::{Action, AppState};
use crate::input::{keyboard, mouse};

/// Tick cadence. The clock only shows minutes; 250 ms keeps future spinner
/// animation smooth without busy-waiting.
pub const TICK_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    /// Click or wheel event (Phase 10, plan §10). Arrives only while mouse
    /// capture is enabled (`[tmail].mouse`).
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

/// Upper bound on events dispatched per frame. A touchpad gesture arrives
/// as a burst of wheel events; draining what has already queued in one
/// frame keeps a keypress from waiting behind the burst (one draw per
/// batch, not one per event), and the cap bounds a single frame's work —
/// whatever is still queued drains on the following frames.
pub const MAX_BATCH: usize = 256;

/// Translate a drained batch of raw events into reducer actions, in
/// arrival order, with the burst-specific rules that keep input responsive
/// under touchpad momentum scrolling:
///
/// - **Wheel runs coalesce**: consecutive `ScrollUp`/`ScrollDown` events
///   (and arrow keys) fold into net movement, so a hundred-delta momentum
///   burst costs a hundred clamped `MoveDown`s and one frame — not one
///   frame each. Movement is clamped by the reducer per step, so a run
///   past the end of a document is cheap and harmless.
/// - **A key press cancels pending scroll**: wheel events after the first
///   key in the batch are dropped. Without this, momentum leftovers would
///   walk the selection of whatever the key revealed — e.g. the message
///   list after Esc closed the message.
/// - **Ticks collapse to one per batch**: the cadence is a heartbeat, not
///   a work queue (the event loop's own timer uses missed-tick delay).
///
/// Clicks and resizes pass through untouched.
pub fn coalesce(batch: Vec<Event>, hits: &mouse::HitMap, state: &AppState) -> Vec<Action> {
    let mut actions = Vec::with_capacity(batch.len());
    let mut key_seen = false;
    // Pending net movement of the trailing move-run (up is negative),
    // flushed as plain actions before any other action dispatches.
    let mut run: i64 = 0;
    for event in batch {
        let action = match event {
            Event::Key(key) => {
                key_seen = true;
                keyboard::to_action(&state.settings.keymap, key, state.session.focus)
            }
            Event::Mouse(mouse_event) => {
                if key_seen
                    && matches!(
                        mouse_event.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    )
                {
                    // Momentum leftover after a key: cancelled.
                    continue;
                }
                mouse::to_action(mouse_event, hits, state)
            }
            Event::Resize { width, height } => Some(Action::Resize { width, height }),
            Event::Tick => {
                if actions.iter().any(|a| matches!(a, Action::Tick { .. })) {
                    continue;
                }
                Some(Action::Tick {
                    now: Box::new(Local::now().fixed_offset()),
                })
            }
        };
        match action {
            Some(Action::MoveUp) => run -= 1,
            Some(Action::MoveDown) => run += 1,
            Some(other) => {
                flush_run(&mut actions, &mut run);
                actions.push(other);
            }
            None => {}
        }
    }
    flush_run(&mut actions, &mut run);
    actions
}

/// Emit the pending move-run as net movement (opposite deltas cancel) and
/// reset it.
fn flush_run(actions: &mut Vec<Action>, run: &mut i64) {
    let steps = run.unsigned_abs();
    let up = *run < 0;
    for _ in 0..steps {
        actions.push(if up { Action::MoveUp } else { Action::MoveDown });
    }
    *run = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::action::ClickTarget;
    use crate::app::mock::mock_initial_state;
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton};
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn wheel(kind: MouseEventKind) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn click() -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn scroll_down() -> MouseEventKind {
        MouseEventKind::ScrollDown
    }

    #[test]
    fn a_wheel_burst_coalesces_into_net_movement() {
        let state = mock_initial_state();
        let events = (0..8).map(|_| wheel(scroll_down())).collect();
        assert_eq!(
            coalesce(events, &mouse::HitMap::default(), &state),
            vec![Action::MoveDown; 8]
        );
    }

    #[test]
    fn opposite_deltas_cancel() {
        let state = mock_initial_state();
        let events = vec![
            wheel(scroll_down()),
            wheel(scroll_down()),
            wheel(MouseEventKind::ScrollUp),
        ];
        assert_eq!(
            coalesce(events, &mouse::HitMap::default(), &state),
            vec![Action::MoveDown]
        );
        // Fully cancelling runs emit nothing.
        let events = vec![wheel(scroll_down()), wheel(MouseEventKind::ScrollUp)];
        assert_eq!(coalesce(events, &mouse::HitMap::default(), &state), vec![]);
    }

    #[test]
    fn a_key_press_cancels_pending_scroll() {
        let state = mock_initial_state();
        // Scrolling, then Esc, then momentum leftovers: the leftovers must
        // not walk the message-list selection the Esc revealed.
        let events = vec![
            wheel(scroll_down()),
            wheel(scroll_down()),
            key(KeyCode::Esc),
            wheel(scroll_down()),
            wheel(scroll_down()),
        ];
        assert_eq!(
            coalesce(events, &mouse::HitMap::default(), &state),
            vec![Action::MoveDown, Action::MoveDown, Action::BackOrCancel]
        );
    }

    #[test]
    fn a_key_flushes_the_run_before_dispatching() {
        let state = mock_initial_state();
        // The pending run dispatches before the key's own action.
        let events = vec![wheel(scroll_down()), key(KeyCode::Enter)];
        assert_eq!(
            coalesce(events, &mouse::HitMap::default(), &state),
            vec![Action::MoveDown, Action::Activate]
        );
    }

    #[test]
    fn clicks_survive_a_key_and_open_a_run_of_their_own() {
        let mut state = mock_initial_state();
        state.session.focus = crate::app::focus::Focus::MessageList;
        let mut hits = mouse::HitMap::default();
        hits.push(Rect::new(0, 5, 40, 1), ClickTarget::MessageRow(3));
        let events = vec![key(KeyCode::Esc), click()];
        assert_eq!(
            coalesce(events, &hits, &state),
            vec![
                Action::BackOrCancel,
                Action::Click(ClickTarget::MessageRow(3))
            ]
        );
    }

    #[test]
    fn repeated_ticks_collapse_to_one() {
        let state = mock_initial_state();
        let events = vec![Event::Tick, Event::Tick, Event::Tick];
        assert_eq!(coalesce(events, &mouse::HitMap::default(), &state).len(), 1);
    }

    #[test]
    fn resizes_pass_through() {
        let state = mock_initial_state();
        let events = vec![Event::Resize {
            width: 80,
            height: 24,
        }];
        assert_eq!(
            coalesce(events, &mouse::HitMap::default(), &state),
            vec![Action::Resize {
                width: 80,
                height: 24
            }]
        );
    }
}
