//! Crossterm event stream + periodic ticks, delivered as a typed stream
//! (plan §19 Phase 1: `runtime/events.rs`).

use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, KeyEvent, KeyEventKind};
use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Tick cadence. The clock only shows minutes; 250 ms keeps future spinner
/// animation smooth without busy-waiting.
pub const TICK_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Resize { width: u16, height: u16 },
    Tick,
}

/// Spawn the event reader task; returns the receiving end. The task ends
/// when the sender is dropped (i.e. when this receiver goes away).
pub fn spawn() -> UnboundedReceiver<Event> {
    let (tx, rx) = unbounded_channel();
    tokio::spawn(event_loop(tx));
    rx
}

async fn event_loop(tx: UnboundedSender<Event>) {
    let mut reader = crossterm::event::EventStream::new();
    let mut tick = tokio::time::interval(TICK_INTERVAL);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if tx.send(Event::Tick).is_err() {
                    break;
                }
            }
            event = reader.next() => {
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
                    Some(Ok(_)) => {
                        // Mouse arrives in Phase 10; other events ignored.
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
