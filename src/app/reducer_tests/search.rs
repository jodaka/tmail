//! Reducer tests: search domain.

use super::*;

#[test]
fn submit_search_stashes_mailbox_context_and_runs_search() {
    let mut s = state();
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::MoveDown);
    let (page, selection, scroll) = (s.messages.clone(), s.selection, s.list_scroll);

    let effects = search(&mut s, "quote");
    let (id, request) = expect_search(&effects);
    // The query travels unchanged (Phase 9.2), scoped to the current
    // mailbox, first page, list page size.
    assert_eq!(request.query, "quote");
    assert_eq!(request.mailbox_id, inbox_id());
    assert_eq!(request.offset, 0);
    assert_eq!(request.limit, 20);
    assert!(s.operations.get(id).is_some());
    // The search route sits above the mailbox route…
    assert_eq!(s.routes.len(), 2);
    assert_eq!(
        s.active_route(),
        Some(&Route::Search(crate::app::route::SearchRoute {
            query: String::from("quote"),
            mailbox_id: inbox_id(),
        }))
    );
    // …and the mailbox list context is stashed for an exact return.
    let stash = s.search_return.as_ref().expect("stash");
    assert_eq!(stash.page, page);
    assert_eq!(stash.selection, selection);
    assert_eq!(stash.scroll, scroll);
    // The visible list is emptied while the search runs; focus moves to it.
    assert!(s.messages.items.is_empty());
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn submit_search_with_empty_query_is_inert() {
    let mut s = state();
    let before = s.clone();
    s.focus = Focus::SearchField;
    let effects = reduce(&mut s, &Action::SubmitSearch);
    no_effects(&effects);
    assert_eq!(s.routes, before.routes);
    assert!(s.search_return.is_none());
    assert!(s.status.message.is_some(), "guidance is shown");
}

#[test]
fn submit_search_from_the_reader_is_inert() {
    let mut s = state();
    reduce(&mut s, &Action::Activate); // open reader
    let before = s.routes.clone();
    s.focus = Focus::SearchField;
    s.search_query = String::from("quote");
    no_effects(&reduce(&mut s, &Action::SubmitSearch));
    assert_eq!(s.routes, before, "no search above the reader");
    assert!(s.search_return.is_none());
}

#[test]
fn search_results_apply_to_the_shared_list() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    // Results carry the searched mailbox as their context (Phase 9.1).
    let mut page = mock::mock_page(&inbox_id(), 0, 20);
    for summary in &mut page.items {
        summary.subject = format!("quote match: {}", summary.subject);
    }
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
    assert_eq!(s.messages.items.len(), 20);
    assert_eq!(s.messages.items[0].mailbox_id, inbox_id());
    assert!(s.messages.items[0].subject.starts_with("quote match:"));
    assert_eq!(s.selection, 0);
}

#[test]
fn search_results_superseded_by_a_navigation_race_are_dropped() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    // The user leaves the search before the result arrives. Esc first
    // cancels the in-flight search (plan §10), then leaves the route.
    reduce(&mut s, &Action::BackOrCancel);
    reduce(&mut s, &Action::BackOrCancel);
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    // The cancelled result must not clobber the restored mailbox context.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(&inbox_id(), 0, 20))),
        }),
    );
    assert_eq!(s.messages, mock::mock_page(&inbox_id(), 0, 20));
}

#[test]
fn reader_from_search_returns_to_the_same_results() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (search_id, _) = expect_search(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: search_id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(&inbox_id(), 0, 20))),
        }),
    );
    let selected = s.selected_message().cloned().expect("a result row");
    let effects = reduce(&mut s, &Action::Activate); // open reader
    assert!(matches!(s.active_route(), Some(Route::Message(_))));
    // The fetch completes; Esc then pops the reader route itself.
    let load_id = match effects.as_slice() {
        [effect] => {
            assert!(
                matches!(effect.kind, OperationKind::LoadMessage(_)),
                "expected LoadMessage"
            );
            effect.id
        }
        other => panic!("expected one effect, got {other:?}"),
    };
    // Opening an unread message also starts a SetRead; complete it so the
    // registry is idle and the next Esc reaches the route stack.
    let read_effects = reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: load_id,
            outcome: Ok(OperationOutcome::Message(Box::new(mock::mock_message(
                &selected,
            )))),
        }),
    );
    match read_effects.as_slice() {
        [effect] => {
            assert!(
                matches!(effect.kind, OperationKind::SetRead { read: true, .. }),
                "expected the mark-read follow-up"
            );
            reduce(
                &mut s,
                &Action::BackendCompleted(OperationResult {
                    id: effect.id,
                    outcome: Ok(OperationOutcome::Done),
                }),
            );
        }
        other => no_effects(other),
    }
    // Esc returns to the search, whose results were never disturbed.
    reduce(&mut s, &Action::BackOrCancel);
    assert!(matches!(s.active_route(), Some(Route::Search(_))));
    assert_eq!(s.messages.items.len(), 20);
    assert_eq!(
        s.selected_message().map(|m| m.id.clone()),
        Some(selected.id)
    );
}

#[test]
fn leaving_search_restores_the_mailbox_context_exactly() {
    let mut s = state();
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::MoveDown);
    let before = (s.messages.clone(), s.selection, s.list_scroll);
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(&inbox_id(), 0, 20))),
        }),
    );
    // Esc from the search route restores page, selection, and scroll.
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.routes.len(), 1);
    assert!(s.search_return.is_none());
    assert_eq!(s.messages, before.0, "mailbox page restored");
    assert_eq!(s.selection, before.1);
    assert_eq!(s.list_scroll, before.2);
    assert_eq!(s.focus, Focus::MessageList);
    // The search field clears with the results (ticket 32b3): `/` opens
    // an empty field for the next search.
    assert_eq!(s.search_query, "");
}

#[test]
fn resubmitting_edits_the_query_without_disturbing_the_stash() {
    let mut s = state();
    let effects = search(&mut s, "quo");
    let (first, _) = expect_search(&effects);
    let stash = s.search_return.clone().expect("stash");
    // Edit and re-submit while the search route is open.
    let effects = search(&mut s, "te");
    let (second, request) = expect_search(&effects);
    assert_ne!(first, second, "a new operation");
    assert_eq!(request.query, "quote");
    assert_eq!(s.routes.len(), 2, "no second search route");
    assert_eq!(s.search_return.as_ref(), Some(&stash), "stash untouched");
}

#[test]
fn search_pagination_issues_search_requests() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    let mut page = mock::mock_page(&inbox_id(), 0, 20);
    for summary in &mut page.items {
        summary.subject = format!("quote match: {}", summary.subject);
    }
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
    // Right: next results page as a Search request.
    let effects = reduce(&mut s, &Action::PageNext);
    let (_, request) = expect_search(&effects);
    assert_eq!(request.offset, 20);
    assert_eq!(request.query, "quote");
}

#[test]
fn search_operation_supersedes_the_previous_one() {
    let mut s = state();
    let effects = search(&mut s, "quo");
    let (first, _) = expect_search(&effects);
    let effects = search(&mut s, "te");
    let (second, _) = expect_search(&effects);
    assert!(s.operations.get(first).is_none(), "older search cancelled");
    assert!(s.operations.get(second).is_some());
}

#[test]
fn switching_mailbox_from_search_rebuilds_the_route_stack() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(&inbox_id(), 0, 20))),
        }),
    );
    // Pick another mailbox in the sidebar and activate it.
    s.focus = Focus::Sidebar;
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::Activate);
    assert_eq!(s.routes.len(), 1);
    assert_eq!(
        s.active_route().unwrap().mailbox_id().unwrap().0,
        "sent",
        "the new mailbox replaces search and mailbox alike"
    );
    assert!(s.search_return.is_none(), "stash dropped with the search");
}

#[test]
fn auto_refresh_arms_on_first_tick_and_fires_after_the_interval() {
    let mut s = state();
    timer(&mut s);
    // First tick arms the timer; nothing fires.
    no_effects(&tick(&mut s, 0));
    // Halfway: still nothing.
    no_effects(&tick(&mut s, 30));
    // The interval has elapsed on the injected clock: a *background* page
    // refresh for the visible mailbox (Phase 9.4).
    let effects = tick(&mut s, 60);
    let (id, req) = expect_page(&effects);
    assert_eq!(req.mailbox_id, inbox_id());
    assert_eq!(req.offset, 0);
    assert_eq!(
        s.operations.get(id).map(|op| op.origin),
        Some(OperationOrigin::Background)
    );
    // Complete the background refresh; the arm point moved, so the next
    // fire needs another full interval.
    complete_page_ok(&mut s, id, &req, 0);
    let effects = tick(&mut s, 90);
    assert!(effects.is_empty());
    let effects = tick(&mut s, 120);
    expect_page(&effects);
}

#[test]
fn auto_refresh_is_disabled_without_an_interval() {
    let mut s = state();
    // refresh_interval_seconds defaults to 0 in state; the config opts in.
    no_effects(&tick(&mut s, 0));
    no_effects(&tick(&mut s, 3600));
}

#[test]
fn auto_refresh_stands_down_while_conflicting_work_is_in_flight() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    // A foreground operation (the startup page load) is in flight when the
    // interval elapses: the timer stands down and retries later.
    let (manual, req) = expect_page(&reduce(&mut s, &Action::Refresh));
    no_effects(&tick(&mut s, 60));
    complete_page_ok(&mut s, manual, &req, 0);
    // Conflicts cleared; the manual refresh re-armed the timer to its own
    // clock, so the next automatic fire is one full interval later.
    no_effects(&tick(&mut s, 50));
    let effects = tick(&mut s, 60);
    let (id, _) = expect_page(&effects);
    assert_eq!(
        s.operations.get(id).map(|op| op.origin),
        Some(OperationOrigin::Background)
    );
}

#[test]
fn auto_refresh_stands_down_while_composing() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    reduce(&mut s, &Action::Compose);
    no_effects(&tick(&mut s, 60));
    // Leaving the composer unblocks the next tick.
    reduce(&mut s, &Action::LeaveComposer);
    let effects = tick(&mut s, 120);
    expect_page(&effects);
}

#[test]
fn auto_refresh_targets_the_open_search_context() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    let mut page = mock::mock_page(&inbox_id(), 0, 20);
    for summary in &mut page.items {
        summary.subject = format!("quote match: {}", summary.subject);
    }
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
    // The timer refreshes the search at the current offset (Phase 9.5:
    // refresh only the visible context).
    let effects = tick(&mut s, 60);
    let (_, request) = expect_search(&effects);
    assert_eq!(request.query, "quote");
    assert_eq!(request.offset, 0);
}

#[test]
fn background_refresh_failure_never_opens_the_modal() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    let effects = tick(&mut s, 60);
    let (id, _) = expect_page(&effects);
    reduce(
        &mut s,
        &failure(
            id,
            &OperationKind::LoadPage(PageRequest {
                mailbox_id: inbox_id(),
                offset: 0,
                limit: 20,
            }),
            "connect refused",
        ),
    );
    // No Retry/Dismiss modal for background work; the status line carries
    // the failure and the record suppresses repeats (Phase 9.6).
    assert!(s.overlay.is_none());
    assert_eq!(s.last_background_error.as_deref(), Some("connect refused"));
    assert_eq!(
        s.status.message.as_deref(),
        Some("Refresh failed — the timer will retry")
    );
}

#[test]
fn background_failure_record_clears_on_success_and_on_manual_refresh() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    let effects = tick(&mut s, 60);
    let (id, req) = expect_page(&effects);
    reduce(
        &mut s,
        &failure(id, &OperationKind::LoadPage(req.clone()), "connect refused"),
    );
    assert!(s.last_background_error.is_some());
    // A success clears the record: a later failure is reported again.
    let effects = tick(&mut s, 120);
    let (id2, req2) = expect_page(&effects);
    complete_page_ok(&mut s, id2, &req2, 0);
    assert!(s.last_background_error.is_none());
    // A manual refresh that fails is foreground work: it opens the modal.
    reduce(&mut s, &Action::Refresh);
    // (its operation is in flight; complete it with a failure)
    let last = s
        .operations
        .foreground()
        .map(|op| op.id)
        .expect("manual refresh");
    let kind = s.operations.get(last).unwrap().kind.clone();
    reduce(&mut s, &failure(last, &kind, "also refused"));
    assert!(
        matches!(s.overlay, Some(Overlay::Error(_))),
        "foreground failure modals"
    );
}

#[test]
fn manual_refresh_failure_still_opens_the_modal() {
    let mut s = state();
    reduce(&mut s, &Action::Refresh);
    let last = s.operations.foreground().map(|op| op.id).expect("refresh");
    let kind = s.operations.get(last).unwrap().kind.clone();
    reduce(&mut s, &failure(last, &kind, "connect refused"));
    assert!(matches!(s.overlay, Some(Overlay::Error(_))));
    // Manual refresh remains available after dismissing (acceptance).
    reduce(&mut s, &Action::DismissError);
    let effects = reduce(&mut s, &Action::Refresh);
    expect_page(&effects);
}
