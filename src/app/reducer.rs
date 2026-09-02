//! Deterministic, I/O-free reducer (plan §5/§9).
//!
//! `reduce` is the only writer of `AppState`. State transitions that need
//! backend data return [`Effect`]s for the runtime to execute; results come
//! back as `Action::MailboxesLoaded` / `Action::PageLoaded` and are applied
//! here under a pending-request guard so stale results never win. Reducers
//! never perform I/O themselves.

use crate::app::action::{Action, SearchEdit};
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::route::{MailboxRoute, Route};
use crate::app::state::{AppState, Loadable};
use crate::domain::{Mailbox, MailboxId, MailboxRole, MessageSummary, Page, PageRequest};

/// Apply `action` to `state`, returning backend work to spawn. Never
/// performs I/O, never panics on odd input.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    match action {
        Action::MoveUp => move_selection(state, -1),
        Action::MoveDown => move_selection(state, 1),
        Action::PagePrevious => change_page(state, -1),
        Action::PageNext => change_page(state, 1),
        Action::Activate => activate(state),
        Action::BackOrCancel => {
            back_or_cancel(state);
            Vec::new()
        }
        Action::FocusNext => {
            state.focus = state.focus.next();
            Vec::new()
        }
        Action::FocusPrevious => {
            state.focus = state.focus.previous();
            Vec::new()
        }
        Action::OpenSearch => {
            state.focus = Focus::SearchField;
            Vec::new()
        }
        Action::SearchEdit(edit) => {
            search_edit(state, edit);
            Vec::new()
        }
        Action::SubmitSearch => {
            // Real search arrives in Phase 9 (plan §16); the field and its
            // focus/edit interactions already work.
            tracing::debug!(query = %state.search_query, "submit search (noop until phase 9)");
            Vec::new()
        }
        Action::Compose
        | Action::Reply
        | Action::ReplyAll
        | Action::Forward
        | Action::Archive
        | Action::Trash
        | Action::ToggleStar
        | Action::MarkUnread
        | Action::Send
        | Action::LeaveComposer
        | Action::DiscardDraft
        | Action::RetryError
        | Action::DismissError => {
            // Vocabulary is complete (plan §9); the screens owning these
            // actions arrive in later phases. No-op, never a crash.
            tracing::debug!(?action, "action not yet implemented");
            Vec::new()
        }
        Action::MailboxesLoaded(result) => mailboxes_loaded(state, result),
        Action::PageLoaded { request, result } => apply_page(state, request, result),
        Action::Refresh => refresh(state),
        Action::Tick => {
            state.ticks += 1;
            Vec::new()
        }
        Action::Resize { width, height } => {
            state.size = (*width, *height);
            // A smaller window may have pushed the selection off screen.
            keep_selection_visible(state);
            Vec::new()
        }
        Action::Quit => {
            state.quit_requested = true;
            Vec::new()
        }
    }
}

fn move_selection(state: &mut AppState, delta: i64) -> Vec<Effect> {
    match state.focus {
        Focus::MessageList => {
            let len = state.messages.items.len();
            if len == 0 {
                return Vec::new();
            }
            let next = state.selection as i64 + delta;
            state.selection = next.clamp(0, len as i64 - 1) as usize;
            keep_selection_visible(state);
        }
        Focus::Sidebar => {
            let len = state.mailboxes.as_loaded().map(Vec::len).unwrap_or(0);
            if len == 0 {
                return Vec::new();
            }
            let next = state.mailbox_selection as i64 + delta;
            state.mailbox_selection = next.clamp(0, len as i64 - 1) as usize;
        }
        Focus::SearchField => {
            // Cursor movement inside the field is a render concern for now;
            // the query is edited append/backspace only (Phase 1).
        }
    }
    Vec::new()
}

/// Shift `list_scroll` so the selection stays on screen. Row geometry comes
/// from the same pure layout math the renderer uses (`message_rows_visible`),
/// keeping the bookkeeping consistent with what is actually drawn.
fn keep_selection_visible(state: &mut AppState) {
    let visible = crate::ui::layout::message_rows_visible(state.size).max(1);
    if state.selection < state.list_scroll {
        state.list_scroll = state.selection;
    } else if state.selection >= state.list_scroll + visible {
        state.list_scroll = state.selection + 1 - visible;
    }
    // A shrunken or replaced page must never leave the window past the end.
    state.list_scroll = state
        .list_scroll
        .min(state.messages.items.len().saturating_sub(1));
}

fn change_page(state: &mut AppState, delta: i64) -> Vec<Effect> {
    if state.focus != Focus::MessageList {
        return Vec::new();
    }
    let limit = state.messages.limit.max(1) as i64;
    // Base on the in-flight request when there is one, so repeated keys
    // before results arrive keep advancing instead of re-requesting the
    // same next page.
    let base = match state.pending_page.as_ref() {
        Some(pending)
            if state
                .active_route()
                .and_then(Route::mailbox_id)
                .is_some_and(|id| *id == pending.mailbox_id) =>
        {
            pending.offset as i64
        }
        _ => state.messages.offset as i64,
    };
    let target = base + delta * limit;
    if target < 0 {
        return Vec::new();
    }
    // Known total: never issue a request past the end (plan §16/§19).
    if state
        .messages
        .total
        .is_some_and(|total| target >= total as i64)
    {
        return Vec::new();
    }
    // Unknown total (maildir): a short page is the last one, so there is
    // nothing valid to request.
    if state.messages.total.is_none() && state.messages.items.len() < state.messages.limit {
        return Vec::new();
    }
    // The selection keeps pointing at the current page until the result
    // applies; `apply_page` re-resolves it by identity.
    request_page(state, target as usize)
}

/// Record `offset` as the pending page request and emit the effect to fetch
/// it for the active route's mailbox.
fn request_page(state: &mut AppState, offset: usize) -> Vec<Effect> {
    let Some(Route::Mailbox(route)) = state.active_route().cloned() else {
        return Vec::new();
    };
    let request = PageRequest {
        mailbox_id: route.mailbox_id,
        offset,
        limit: state.messages.limit.max(1),
    };
    state.pending_page = Some(request.clone());
    vec![Effect::LoadPage(request)]
}

/// Apply a page result when it still matches the pending request. The
/// selection is re-resolved by `Message-ID` first, then backend id
/// (ADR 0001 finding 4: only the Message-ID is stable across moves).
fn apply_page(
    state: &mut AppState,
    request: &PageRequest,
    result: &Result<Page<MessageSummary>, String>,
) -> Vec<Effect> {
    let still_pending = state
        .pending_page
        .as_ref()
        .is_some_and(|pending| pending == request);
    if !still_pending {
        tracing::debug!(mailbox = %request.mailbox_id.0, offset = request.offset, "dropping stale page result");
        return Vec::new();
    }
    state.pending_page = None;
    match result {
        Ok(page) => {
            let previous = state.selected_message();
            let previous_message_id = previous.and_then(|m| m.message_id.clone());
            let previous_id = previous.map(|m| m.id.clone());
            state.messages = page.clone();
            let len = state.messages.items.len();
            state.selection = state
                .messages
                .items
                .iter()
                .position(|m| {
                    previous_message_id
                        .as_ref()
                        .is_some_and(|id| m.message_id.as_ref() == Some(id))
                        || previous_id.as_ref().is_some_and(|id| &m.id == id)
                })
                .unwrap_or(0)
                .min(len.saturating_sub(1));
            state.list_scroll = state.list_scroll.min(len.saturating_sub(1));
            keep_selection_visible(state);
        }
        Err(err) => {
            // Phase 3 replaces this with the Retry/Dismiss modal; for now
            // the last coherent page stays visible and the failure is
            // reported in the status area.
            state.set_status(format!("Could not load messages: {err}"));
        }
    }
    Vec::new()
}

/// Apply the mailbox listing: select the Inbox, or the first mailbox when
/// no Inbox exists, and load its first page.
fn mailboxes_loaded(state: &mut AppState, result: &Result<Vec<Mailbox>, String>) -> Vec<Effect> {
    match result {
        Ok(mailboxes) => {
            state.mailboxes = Loadable::Loaded(mailboxes.clone());
            let chosen = mailboxes
                .iter()
                .position(|m| m.role == Some(MailboxRole::Inbox))
                .or_else(|| (!mailboxes.is_empty()).then_some(0));
            match chosen {
                Some(index) => {
                    let mailbox_id = mailboxes[index].id.clone();
                    state.routes = vec![Route::Mailbox(MailboxRoute { mailbox_id })];
                    state.mailbox_selection = index;
                    state.selection = 0;
                    state.list_scroll = 0;
                    state.messages = Page::empty(state.messages.limit);
                    state.set_status("Mailboxes loaded");
                    request_page(state, 0)
                }
                None => {
                    // The account genuinely has no mailboxes; an empty list
                    // is a valid state, not an error (plan §16).
                    state.routes.clear();
                    state.messages = Page::empty(state.messages.limit);
                    state.pending_page = None;
                    Vec::new()
                }
            }
        }
        Err(err) => {
            state.mailboxes = Loadable::Failed(err.clone());
            state.set_status(format!("Could not load mailboxes: {err}"));
            Vec::new()
        }
    }
}

fn activate(state: &mut AppState) -> Vec<Effect> {
    match state.focus {
        Focus::Sidebar => match state.selected_mailbox().cloned() {
            Some(mailbox) => switch_mailbox(state, &mailbox.id),
            None => Vec::new(),
        },
        Focus::MessageList => {
            // The reader screen arrives in Phase 4; opening a message is a
            // no-op until then.
            tracing::debug!("activate on message list (reader arrives in phase 4)");
            Vec::new()
        }
        Focus::SearchField => reduce(state, &Action::SubmitSearch),
    }
}

fn switch_mailbox(state: &mut AppState, mailbox_id: &MailboxId) -> Vec<Effect> {
    if state.active_route()
        == Some(&Route::Mailbox(MailboxRoute {
            mailbox_id: mailbox_id.clone(),
        }))
    {
        return Vec::new();
    }
    state.routes.pop();
    state.routes.push(Route::Mailbox(MailboxRoute {
        mailbox_id: mailbox_id.clone(),
    }));
    state.mailbox_selection = state
        .mailboxes
        .as_loaded()
        .and_then(|ms| ms.iter().position(|m| &m.id == mailbox_id))
        .unwrap_or(0);
    state.selection = 0;
    state.list_scroll = 0;
    state.focus = Focus::MessageList;
    // Rows of the previous mailbox must not linger while the new one loads.
    state.messages = Page::empty(state.messages.limit);
    request_page(state, 0)
}

fn back_or_cancel(state: &mut AppState) {
    // Order per plan §10: cancel work, close overlay, go back. Neither
    // cancellable work nor overlays exist until Phase 3, so Phase 2 has:
    // leave the search field first, otherwise back/quit.
    if state.focus == Focus::SearchField {
        state.focus = Focus::MessageList;
        return;
    }
    if state.routes.len() > 1 {
        state.routes.pop();
        state.focus = Focus::MessageList;
        return;
    }
    // Root route with nothing to cancel or close: exit cleanly (plan §19
    // Phase 1 acceptance: app exits with Esc/quit).
    state.quit_requested = true;
}

fn search_edit(state: &mut AppState, edit: &SearchEdit) {
    if state.focus != Focus::SearchField {
        return;
    }
    match edit {
        SearchEdit::Char(c) => state.search_query.push(*c),
        SearchEdit::Backspace => {
            state.search_query.pop();
        }
    }
}

fn refresh(state: &mut AppState) -> Vec<Effect> {
    if !matches!(state.mailboxes, Loadable::Loaded(_)) || state.active_route().is_none() {
        return Vec::new();
    }
    state.set_status("Refreshing…");
    request_page(state, state.messages.offset)
}

#[cfg(test)]
#[path = "reducer_tests.rs"]
mod tests;
