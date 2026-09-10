//! Reducer tests: status domain.

use super::*;

// ── Status-message timeout (ticket h1d7) ─────────────────────────────────

#[test]
fn status_timeout_clears_the_message_when_the_window_elapses() {
    let mut s = state();
    s.settings.status_timeout_seconds = 5;
    // Arm the injected clock, then set a status the way the reducer does:
    // the timer starts at the `shown_at` stamp taken from the clock.
    tick(&mut s, 0);
    s.set_status("Message sent");
    assert_eq!(
        s.session.status.shown_at, s.session.clock,
        "the timer arms at set time"
    );
    // Inside the window the message stays.
    tick(&mut s, 4);
    assert_eq!(s.session.status.message.as_deref(), Some("Message sent"));
    // One second past the window it is gone, stamp included.
    tick(&mut s, 5);
    assert_eq!(s.session.status.message, None);
    assert_eq!(s.session.status.shown_at, None);
}

#[test]
fn status_timeout_zero_keeps_the_message_indefinitely() {
    let mut s = state();
    tick(&mut s, 0);
    s.set_status("Mailboxes loaded");
    tick(&mut s, 3_600);
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Mailboxes loaded"),
        "the default (0) never expires"
    );
}

#[test]
fn a_new_status_rearms_the_timeout() {
    let mut s = state();
    s.settings.status_timeout_seconds = 5;
    tick(&mut s, 0);
    s.set_status("first");
    tick(&mut s, 4);
    s.set_status("second");
    assert_eq!(s.session.status.message.as_deref(), Some("second"));
    // Four seconds passed since the *first* message; the second restarted
    // the window, so it must still be visible one tick later.
    tick(&mut s, 5);
    assert_eq!(s.session.status.message.as_deref(), Some("second"));
    tick(&mut s, 9);
    assert_eq!(
        s.session.status.message, None,
        "expired five seconds after reset"
    );
}
