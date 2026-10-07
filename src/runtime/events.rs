//! Crossterm event stream + periodic ticks, delivered as a typed stream
//! (plan §19 Phase 1: `runtime/events.rs`).

use std::time::Duration;

use chrono::Local;
use crossterm::event::{
    Event as CrosstermEvent, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::app::{Action, AppState};
use crate::input::{keyboard, mouse};

/// Tick cadence while the app is idle (the clock only shows minutes; a
/// slow heartbeat costs nothing).
pub const SLOW_TICK_INTERVAL: Duration = Duration::from_millis(250);
/// Tick cadence while foreground work animates the Knight Rider scanner:
/// ~20 frames/second feeds the render loop fast enough for the 40 ms
/// scanner frames.
pub const FAST_TICK_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    /// Click or wheel event (Phase 10, plan §10). Arrives only while mouse
    /// capture is enabled (`[tmail].mouse`).
    Mouse(MouseEvent),
    /// The terminal window gained (`true`) or lost (`false`) focus (CSI
    /// 1004 focus events, ticket b28p). Terminals without focus reporting
    /// simply never send one.
    Focus(bool),
    Resize {
        width: u16,
        height: u16,
    },
    Tick,
}

/// Control commands to the running event loop.
enum Control {
    /// Switch the tick cadence: fast (`true`) while foreground work keeps
    /// the loader animating, slow otherwise, so idle sessions don't burn
    /// the render loop at 20 Hz for nothing.
    Pace(bool),
    /// Stop reading input events and ticks *and drop the crossterm
    /// reader*, then acknowledge through the one-shot. The terminal is
    /// about to be handed to a child program (the external editor, plan
    /// §14 Phase 11), and a concurrent reader would steal its
    /// keystrokes. Dropping is the load-bearing half (ticket a8n9): the
    /// stream's background thread sits in a blocking `poll_internal`
    /// on the tty and neither sleeps nor cancels just because the
    /// select arm is disabled — only `EventStream`'s `Drop` tells it to
    /// leave.
    Pause(oneshot::Sender<()>),
    /// Re-arm a fresh reader and resume normal event delivery.
    Resume,
}

/// Handle over the running event loop's lifecycle (Phase 11.5): pause
/// while the external editor owns the terminal, resume after it exits,
/// and match the tick pace to the loader's animation demands.
#[derive(Clone)]
pub struct EventControl {
    tx: UnboundedSender<Control>,
}

impl EventControl {
    /// Pause input delivery and wait until the event loop confirms the
    /// crossterm reader is torn down. The external editor must only take
    /// the terminal after this returns — a keystroke typed into the
    /// editor before the old reader dies can be consumed by its
    /// background thread and replayed into tmail after resume (ticket
    /// a8n9). Resolves even when the loop is already gone (ack error):
    /// the terminal is being dropped in that case too, so the editor
    /// flow never hangs on a vanished reader.
    pub async fn pause(&self) {
        let (ack, wait) = oneshot::channel();
        if self.tx.send(Control::Pause(ack)).is_err() {
            return;
        }
        let _ = wait.await;
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Control::Resume);
    }

    /// `true`: ticks arrive at [`FAST_TICK_INTERVAL`] (the scanner is
    /// animating); `false`: back to [`SLOW_TICK_INTERVAL`].
    pub fn set_fast_ticks(&self, fast: bool) {
        let _ = self.tx.send(Control::Pace(fast));
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
    // The reader is optional while paused: `None` means the terminal
    // belongs to a child program (the external editor) and no tmail-owned
    // reader may touch the tty (ticket a8n9 — with a live but unselected
    // stream, crossterm's background thread stays blocked on the tty and
    // consumes the editor's first keystrokes).
    let mut reader: Option<crossterm::event::EventStream> =
        Some(crossterm::event::EventStream::new());
    let mut idle = tokio::time::interval(SLOW_TICK_INTERVAL);
    let mut fast = tokio::time::interval(FAST_TICK_INTERVAL);
    // While paused (external editor owns the terminal) ticks are not
    // consumed; delay mode prevents a burst firing on resume. The first
    // tick of a fresh interval fires immediately: consume both so the
    // select below waits for real ticks.
    idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    fast.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    idle.tick().await;
    fast.tick().await;
    let mut fast_ticks = false;
    let mut paused = false;
    loop {
        tokio::select! {
            biased;
            command = control.recv() => match command {
                Some(Control::Pace(fast)) => fast_ticks = fast,
                Some(Control::Pause(ack)) => {
                    // `EventStream::drop` is what wakes and releases
                    // crossterm's background poll thread; then the ack
                    // lets the editor flow proceed to the spawn (ticket
                    // a8n9). A second pause is idempotent.
                    reader = None;
                    paused = true;
                    let _ = ack.send(());
                }
                Some(Control::Resume) => {
                    if reader.is_none() {
                        reader = Some(crossterm::event::EventStream::new());
                    }
                    paused = false;
                }
                None => break,
            },
            // Only the active pace's timer is awaited; a pace switch takes
            // effect on the next loop turn (worst case one slow tick of
            // lag).
            _ = fast.tick(), if fast_ticks && !paused => {
                if tx.send(Event::Tick).is_err() {
                    break;
                }
            }
            _ = idle.tick(), if !fast_ticks && !paused => {
                if tx.send(Event::Tick).is_err() {
                    break;
                }
            }
            // While paused this arm parks forever instead of touching the
            // (dropped) reader; the pause flag keeps every other turn of
            // the select going until Resume re-arms a fresh stream.
            poll = poll_reader(&mut reader) => {
                match poll {
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
                    Some(Ok(CrosstermEvent::FocusGained)) => {
                        // New-mail notifications fire only while the
                        // window is unfocused (ticket b28p).
                        tracing::debug!("terminal focus gained");
                        if tx.send(Event::Focus(true)).is_err() {
                            break;
                        }
                    }
                    Some(Ok(CrosstermEvent::FocusLost)) => {
                        tracing::debug!("terminal focus lost");
                        if tx.send(Event::Focus(false)).is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // Paste and other gestures have no v1 behavior.
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

/// Poll the armed reader; while unloaded (paused), never resolve so the
/// other select arms keep the loop's control path alive. Never touches a
/// dropped stream, and never panics on the select construction order.
async fn poll_reader(
    reader: &mut Option<crossterm::event::EventStream>,
) -> Option<std::io::Result<CrosstermEvent>> {
    match reader {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
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
    // A tick already collapsed into the batch: every later one is a
    // duplicate heartbeat (a bool, not a scan — issue pbcn).
    let mut tick_seen = false;
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
            Event::Focus(focused) => Some(Action::SetTerminalFocus(focused)),
            Event::Resize { width, height } => Some(Action::Resize { width, height }),
            Event::Tick => {
                if tick_seen {
                    continue;
                }
                tick_seen = true;
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
    fn focus_changes_map_to_set_terminal_focus() {
        let state = mock_initial_state();
        let events = vec![Event::Focus(false), Event::Focus(true)];
        assert_eq!(
            coalesce(events, &mouse::HitMap::default(), &state),
            vec![
                Action::SetTerminalFocus(false),
                Action::SetTerminalFocus(true)
            ]
        );
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

    /// Ticket a8n9: the editor spawn path *awaits* `pause()`. Whether the
    /// loop is alive (it acks after dropping the reader) or already
    /// gone (the control receiver dropped with its task), the handshake
    /// must resolve — a hang here would freeze the terminal while the
    /// editor is supposed to take over. In a test environment the
    /// loop's crossterm source may fail immediately or keep polling:
    /// both outcomes are acceptable, no outcome may deadlock.
    #[tokio::test]
    async fn pause_resolves_whatever_the_loop_state() {
        let (_events, control) = spawn();
        let started = std::time::Instant::now();
        control.pause().await;
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "pause must not hang the editor flow"
        );
        // And after the receiver (and with it the loop) is gone.
        drop(_events);
        let started = std::time::Instant::now();
        control.pause().await;
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "pause on a dead loop must not hang either"
        );
        // Resume on a dead loop is a fire-and-forget send: nothing to
        // await, but it must not panic either.
        control.resume();
    }
}
