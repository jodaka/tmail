//! Deterministic, I/O-free reducer (plan §5/§9).
//!
//! `reduce` is the only writer of `AppState`. In Phase 1 the "backend" is
//! the pure mock store in [`crate::app::mock`], so page loads happen inline;
//! when the real backend lands (Phase 2/3) these loads become effects and
//! results flow back through a `BackendCompleted`-style action. The state
//! transitions below stay exactly as they are.

use crate::app::action::{Action, SearchEdit};
use crate::app::focus::Focus;
use crate::app::mock;
use crate::app::route::{MailboxRoute, Route};
use crate::app::state::{AppState, Loadable};
use crate::domain::{MailboxId, Page};

/// Apply `action` to `state`. Never performs I/O, never panics on odd input.
pub fn reduce(state: &mut AppState, action: &Action) {
    match action {
        Action::MoveUp => move_selection(state, -1),
        Action::MoveDown => move_selection(state, 1),
        Action::PagePrevious => change_page(state, -1),
        Action::PageNext => change_page(state, 1),
        Action::Activate => activate(state),
        Action::BackOrCancel => back_or_cancel(state),
        Action::FocusNext => {
            state.focus = state.focus.next();
        }
        Action::FocusPrevious => {
            state.focus = state.focus.previous();
        }
        Action::OpenSearch => state.focus = Focus::SearchField,
        Action::SearchEdit(edit) => search_edit(state, edit),
        Action::SubmitSearch => {
            // Real search arrives in Phase 9 (plan §16); the field and its
            // focus/edit interactions already work.
            tracing::debug!(query = %state.search_query, "submit search (noop in phase 1)");
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
        }
        Action::Refresh => refresh(state),
        Action::Tick => state.ticks += 1,
        Action::Resize { width, height } => state.size = (*width, *height),
        Action::Quit => state.quit_requested = true,
    }
}

fn move_selection(state: &mut AppState, delta: i64) {
    match state.focus {
        Focus::MessageList => {
            let len = state.messages.items.len();
            if len == 0 {
                return;
            }
            let next = state.selection as i64 + delta;
            state.selection = next.clamp(0, len as i64 - 1) as usize;
        }
        Focus::Sidebar => {
            let len = state.mailboxes.as_loaded().map(Vec::len).unwrap_or(0);
            if len == 0 {
                return;
            }
            let next = state.mailbox_selection as i64 + delta;
            state.mailbox_selection = next.clamp(0, len as i64 - 1) as usize;
        }
        Focus::SearchField => {
            // Cursor movement inside the field is a render concern for now;
            // the query is edited append/backspace only (Phase 1).
        }
    }
}

fn change_page(state: &mut AppState, delta: i64) {
    if state.focus != Focus::MessageList {
        return;
    }
    let (offset, limit) = (state.messages.offset, state.messages.limit);
    let target = offset as i64 + delta * limit as i64;
    if target < 0 || target == offset as i64 {
        return;
    }
    // Never issue a request past the known total (plan §16/§19: page
    // boundaries stay valid).
    if state
        .messages
        .total
        .is_some_and(|total| target >= total as i64)
    {
        return;
    }
    load_page(state, target as usize);
    state.selection = 0;
}

/// Load the page for the active route's mailbox at `offset`, preserving the
/// selected message identity when it survives the load.
fn load_page(state: &mut AppState, offset: usize) {
    let Some(Route::Mailbox(route)) = state.active_route().cloned() else {
        return;
    };
    let selected_id = state.selected_message().map(|m| m.id.clone());
    let page: Page<_> = mock::mock_page(&route.mailbox_id, offset, mock::PAGE_SIZE);
    state.messages = page;
    state.selection = selected_id
        .and_then(|id| state.messages.items.iter().position(|m| m.id == id))
        .unwrap_or(0);
}

fn activate(state: &mut AppState) {
    match state.focus {
        Focus::Sidebar => {
            if let Some(mailbox) = state.selected_mailbox().cloned() {
                switch_mailbox(state, &mailbox.id);
            }
        }
        Focus::MessageList => {
            // The reader screen arrives in Phase 4; opening a message is a
            // no-op until then.
            tracing::debug!("activate on message list (reader arrives in phase 4)");
        }
        Focus::SearchField => {
            reduce(state, &Action::SubmitSearch);
        }
    }
}

fn switch_mailbox(state: &mut AppState, mailbox_id: &MailboxId) {
    if state.active_route()
        == Some(&Route::Mailbox(MailboxRoute {
            mailbox_id: mailbox_id.clone(),
        }))
    {
        return;
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
    load_page(state, 0);
    state.selection = 0;
    state.focus = Focus::MessageList;
}

fn back_or_cancel(state: &mut AppState) {
    // Order per plan §10: cancel work, close overlay, go back. Neither
    // cancellable work nor overlays exist until Phase 3, so Phase 1 has:
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

fn refresh(state: &mut AppState) {
    let offset = state.messages.offset;
    match &state.mailboxes {
        Loadable::Loaded(_) => load_page(state, offset),
        Loadable::Loading => return,
        Loadable::Failed(_) => return,
    }
    state.set_status("Refreshed");
}

#[cfg(test)]
#[path = "reducer_tests.rs"]
mod tests;
