//! New-mail notifications (`[tmail].notifications`, ticket b28p): the
//! background refresh marks its boundaries clean, and the leading run of
//! ids a finished page adds notifies — but only while the terminal window
//! is unfocused and a mode other than `off` is configured.

use super::*;
use crate::app::operation::NotifyRequest;
use crate::config::Notifications;
use crate::domain::Page;

/// A state with notifications configured and the terminal unfocused (the
/// "user is elsewhere" situation notifications exist for).
fn notified_state(mode: Notifications) -> AppState {
    let mut s = state();
    s.settings.notifications = mode;
    s.session.terminal_focused = false;
    s
}

/// A brand-new summary with recognizable sender and subject, derived from
/// the mock inbox row (so it carries a snippet and no preview fires).
fn new_summary(index: usize) -> MessageSummary {
    let mut summary = mock::mock_page(&inbox_id(), 0, 1).items.remove(0);
    summary.id = MessageId(format!("new-{index}"));
    summary.message_id = Some(format!("<new-{index}@tmail.local>"));
    summary.from = vec![Address {
        name: Some(format!("Sender {index}")),
        email: format!("sender{index}@example.org"),
    }];
    summary.subject = format!("New message {index}");
    summary
}

/// The current page with `prefix` brand-new rows prepended (newest first,
/// the way `envelope list` orders mail).
fn page_with_new(prefix: usize) -> Page<MessageSummary> {
    let all = mock::mock_page(&inbox_id(), 0, crate::app::mock::PAGE_SIZE);
    let mut items: Vec<MessageSummary> = (0..prefix).rev().map(new_summary).collect();
    items.extend(all.items);
    Page {
        items,
        offset: all.offset,
        limit: all.limit,
        total: all.total.map(|total| total + prefix),
    }
}

/// Arm the timer on the first tick (no fire).
fn arm_timer(s: &mut AppState) {
    timer(s);
    no_effects(&tick(s, 0));
}

/// Fire the background refresh due `at` seconds on the mock clock;
/// returns the in-flight `(id, request)`.
fn background_refresh_at(s: &mut AppState, at: i64) -> (OperationId, PageRequest) {
    let effects = tick(s, at);
    let (id, req) = expect_page(&effects);
    assert_eq!(
        s.session.operations.get(id).map(|op| op.origin),
        Some(OperationOrigin::Background)
    );
    (id, req)
}

/// Complete the in-flight background page load with `page`.
fn complete_background(
    s: &mut AppState,
    id: OperationId,
    page: Page<MessageSummary>,
) -> Vec<Effect> {
    let effects = reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
    settle_cache_stores(s, effects)
}

/// The single `Notify` effect's request.
fn expect_notify(effects: &[Effect]) -> NotifyRequest {
    let (_, kind) = effect_parts(effects);
    let OperationKind::Notify { request } = kind else {
        panic!("expected a Notify effect, got {kind:?}");
    };
    request
}

#[test]
fn bell_notifies_the_leading_new_run_when_unfocused() {
    let mut s = notified_state(Notifications::Bell);
    arm_timer(&mut s);
    let (id, _) = background_refresh_at(&mut s, 60);
    let effects = complete_background(&mut s, id, page_with_new(2));
    assert_eq!(expect_notify(&effects), NotifyRequest::Bell);
}

#[test]
fn notifications_never_take_the_foreground_slot() {
    let mut s = notified_state(Notifications::Bell);
    arm_timer(&mut s);
    let (id, _) = background_refresh_at(&mut s, 60);
    let effects = complete_background(&mut s, id, page_with_new(2));
    let (notify_id, _) = effect_parts(&effects);
    assert_eq!(
        s.session.operations.get(notify_id).map(|op| op.origin),
        Some(OperationOrigin::Background)
    );
}

#[test]
fn focused_terminal_stays_silent() {
    let mut s = notified_state(Notifications::Bell);
    s.session.terminal_focused = true;
    arm_timer(&mut s);
    let (id, _) = background_refresh_at(&mut s, 60);
    let effects = complete_background(&mut s, id, page_with_new(2));
    no_effects(&effects);
}

#[test]
fn off_never_notifies_even_when_unfocused() {
    let mut s = notified_state(Notifications::Off);
    arm_timer(&mut s);
    let (id, _) = background_refresh_at(&mut s, 60);
    let effects = complete_background(&mut s, id, page_with_new(2));
    no_effects(&effects);
}

#[test]
fn a_single_new_message_names_its_sender_and_subject() {
    let mut s = notified_state(Notifications::On);
    arm_timer(&mut s);
    let (id, _) = background_refresh_at(&mut s, 60);
    let effects = complete_background(&mut s, id, page_with_new(1));
    assert_eq!(
        expect_notify(&effects),
        NotifyRequest::Desktop {
            summary: String::from("Sender 0"),
            body: String::from("New message 0"),
        }
    );
}

#[test]
fn several_new_messages_report_the_count() {
    let mut s = notified_state(Notifications::On);
    arm_timer(&mut s);
    let (id, _) = background_refresh_at(&mut s, 60);
    let effects = complete_background(&mut s, id, page_with_new(3));
    assert_eq!(
        expect_notify(&effects),
        NotifyRequest::Desktop {
            summary: String::from("Tmail"),
            body: String::from("3 new messages"),
        }
    );
}

#[test]
fn known_mail_deeper_in_the_page_never_notifies() {
    // The first row was already clean; an unknown id below it is not a new
    // arrival (a widened page, a moved row), so the leading run is empty.
    let mut s = notified_state(Notifications::Bell);
    arm_timer(&mut s);
    let (id, _) = background_refresh_at(&mut s, 60);
    let mut page = page_with_new(0);
    page.items.insert(1, new_summary(9));
    let effects = complete_background(&mut s, id, page);
    no_effects(&effects);
}

#[test]
fn the_finish_boundary_advances_the_clean_state() {
    let mut s = notified_state(Notifications::Bell);
    arm_timer(&mut s);
    let (id, _) = background_refresh_at(&mut s, 60);
    let effects = complete_background(&mut s, id, page_with_new(2));
    expect_notify(&effects);
    // The next refresh of the same page finds nothing new: the finish
    // boundary marked the arrivals clean.
    let (id, _) = background_refresh_at(&mut s, 120);
    let effects = complete_background(&mut s, id, page_with_new(2));
    no_effects(&effects);
}

#[test]
fn a_manual_refresh_between_updates_calms_the_notification() {
    // The user already saw the new mail via Ctrl+R: the next background
    // update must not notify about it again (the start boundary marks the
    // visible page clean).
    let mut s = notified_state(Notifications::Bell);
    arm_timer(&mut s);
    let (id, _) = expect_page(&reduce(&mut s, Action::Refresh));
    let effects = complete_background(&mut s, id, page_with_new(1));
    no_effects(&effects);
    let (id, _) = background_refresh_at(&mut s, 60);
    let effects = complete_background(&mut s, id, page_with_new(1));
    no_effects(&effects);
}

#[test]
fn foreground_page_loads_never_notify() {
    let mut s = notified_state(Notifications::Bell);
    let (id, _) = expect_page(&reduce(&mut s, Action::Refresh));
    let effects = complete_background(&mut s, id, page_with_new(2));
    no_effects(&effects);
}

#[test]
fn terminal_focus_actions_track_the_window_state() {
    let mut s = state();
    assert!(s.session.terminal_focused, "starts focused");
    no_effects(&reduce(&mut s, Action::SetTerminalFocus(false)));
    assert!(!s.session.terminal_focused);
    no_effects(&reduce(&mut s, Action::SetTerminalFocus(true)));
    assert!(s.session.terminal_focused);
}
