//! Search (plan §16/§19 Phase 9) and refresh: submit/leave search,
//! manual refresh, and the periodic auto-refresh tick.
use super::navigation::{request_visible_page, request_visible_page_background};
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::OperationKind;
use crate::app::route::{Route, SearchRoute};
use crate::app::state::{AppState, ListStash, Loadable};
use crate::domain::{Page, SearchRequest};

// ── Search (plan §16/§19 Phase 9) ────────────────────────────────────────

/// Enter in the search field: run the query against the current mailbox.
/// The query travels to the backend unchanged (Phase 9.2: no local parser).
/// The mailbox list context is stashed for an exact return (Phase 9.1), and
/// search results reuse the mailbox list's page/selection/scroll machinery.
/// Re-submitting while the search route is open re-runs the (edited) query
/// without disturbing the stashed context.
pub(crate) fn submit_search(state: &mut AppState) -> Vec<Effect> {
    // Submitting is the search field's Enter; dispatched elsewhere it is
    // inert (the field is the only submit affordance, plan §10).
    if state.session.focus != Focus::SearchField {
        return Vec::new();
    }
    let query = state.session.search_query.clone();
    if query.trim().is_empty() {
        state.set_status("Type something to search for");
        return Vec::new();
    }
    // The search runs against the mailbox currently displayed. Submitting
    // is inert while a reader/composer sits on top: the search field is a
    // mailbox-screen affordance.
    let mailbox_id = match state.active_route() {
        Some(Route::Mailbox(route)) => route.mailbox_id.clone(),
        Some(Route::Search(route)) => route.mailbox_id.clone(),
        _ => return Vec::new(),
    };
    let already_searching = matches!(state.active_route(), Some(Route::Search(_)));
    if already_searching {
        if let Some(Route::Search(route)) = state.session.routes.last_mut() {
            route.query = query.clone();
        }
    } else {
        state.session.search_return = Some(ListStash {
            page: state.messages.clone(),
            selection: state.selection,
            scroll: state.list_scroll,
        });
        state.session.routes.push(Route::Search(SearchRoute {
            query: query.clone(),
            mailbox_id: mailbox_id.clone(),
        }));
    }
    state.selection = 0;
    state.list_scroll = 0;
    state.messages = Page::empty(state.messages.limit);
    state.session.focus = Focus::MessageList;
    let limit = state.messages.limit.max(1);
    vec![
        state
            .session
            .operations
            .start(OperationKind::Search(SearchRequest {
                mailbox_id,
                query,
                offset: 0,
                limit,
            })),
    ]
}

/// Leave the search route: restore the stashed mailbox list context
/// (Phase 9.1) — page, selection, and scroll come back exactly as they
/// were, with no reload. The search field clears with it (ticket 32b3):
/// leaving the results means the query is done, so `/` opens an empty
/// field for the next search.
pub(crate) fn leave_search(state: &mut AppState) {
    if matches!(state.active_route(), Some(Route::Search(_))) {
        state.session.routes.pop();
        if let Some(stash) = state.session.search_return.take() {
            state.messages = stash.page;
            state.selection = stash.selection;
            state.list_scroll = stash.scroll;
        }
        state.session.focus = Focus::MessageList;
        state.session.search_query.clear();
        // Search selections do not follow the user back to the mailbox
        // list (ticket p0s3): the visible set changed entirely.
        state.selected.clear();
    }
}

/// Manual refresh (`Ctrl+R`): (re)load the mailbox listing while startup
/// has not completed, otherwise refresh the visible context — the mailbox
/// page or the open search results (Phase 9.5). Re-arms the periodic
/// timer, so a manual refresh never collides with an imminent auto one.
pub(crate) fn refresh(state: &mut AppState) -> Vec<Effect> {
    if !matches!(state.mailboxes, Loadable::Loaded(_)) {
        if state.session.operations.is_loading_mailboxes() {
            return Vec::new();
        }
        // Ticket haeb: the cached mailbox listing renders the sidebar
        // (and, through the page cache, the first page) instantly; the
        // fresh listing still loads and replaces it. The read runs
        // off-thread; its completion applies the listing and starts the
        // fresh background load.
        return vec![
            state
                .session
                .operations
                .start_background(OperationKind::CacheMailboxesLoad),
        ];
    }
    if state.active_route().is_none() {
        return Vec::new();
    }
    state.set_status("Refreshing…");
    state.session.last_refresh_at = state.session.clock;
    request_visible_page(state, state.messages.offset)
}

/// The periodic timer (Phase 9.4, plan §11): every
/// `refresh_interval_seconds` of injected clock time, refresh the visible
/// context in the background. The timer never interrupts conflicting work:
/// it stands down while a modal is open, the composer is on screen, or any
/// operation is in flight, and retries on the next tick once they clear.
/// The first tick arms the timer (the reducer has no clock before then).
pub(crate) fn auto_refresh_tick(
    state: &mut AppState,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Vec<Effect> {
    if state.settings.refresh_interval_seconds == 0 {
        return Vec::new();
    }
    let Some(last) = state.session.last_refresh_at else {
        state.session.last_refresh_at = Some(now);
        return Vec::new();
    };
    let elapsed = (now - last).num_seconds().max(0) as u64;
    if elapsed < state.settings.refresh_interval_seconds {
        return Vec::new();
    }
    // The timer never interrupts interactive work: it stands down while a
    // modal is open, the composer is on screen, or a *foreground* operation
    // is in flight (ticket wxtx: silent background preview fetches do not
    // block it), and retries on the next tick once they clear. The first
    // tick arms the timer (the reducer has no clock before then).
    fn conflicts(state: &AppState) -> bool {
        state.session.overlay.is_some()
            || matches!(state.active_route(), Some(Route::Composer))
            || state.session.operations.has_foreground()
    }
    if conflicts(state) {
        tracing::debug!("auto refresh stood down: conflicting work in flight");
        return Vec::new();
    }
    state.session.last_refresh_at = Some(now);
    tracing::debug!(elapsed, "auto refresh");
    request_visible_page_background(state, state.messages.offset)
}
