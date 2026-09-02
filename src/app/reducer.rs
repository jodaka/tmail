//! Deterministic, I/O-free reducer (plan §5/§9).
//!
//! `reduce` is the only writer of `AppState`. State transitions that need
//! backend data start an operation in the registry and return an
//! [`Effect`] for the runtime to execute; results come back as
//! `Action::BackendCompleted` and apply only while their operation is still
//! registered, so stale, cancelled, or superseded results never win
//! (plan §11). Reducers never perform I/O themselves.

use crate::app::action::{Action, SearchEdit};
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::{OperationFailure, OperationKind, OperationOutcome, OperationResult};
use crate::app::overlay::{ErrorDialog, ModalButton, Overlay};
use crate::app::route::{MailboxRoute, MessageRoute, Route};
use crate::app::state::{AppState, Loadable};
use crate::domain::{Mailbox, MailboxId, MailboxRole, Message, MessageLocator, Page, PageRequest};

/// Apply `action` to `state`, returning backend work to spawn. Never
/// performs I/O, never panics on odd input.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    // A modal overlay intercepts all input while it is open (plan §9).
    if let Some(effects) = modal_reduce(state, action) {
        return effects;
    }
    match action {
        Action::MoveUp => move_selection(state, -1),
        Action::MoveDown => move_selection(state, 1),
        Action::PagePrevious => page_step(state, -1),
        Action::PageNext => page_step(state, 1),
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
        Action::Archive => archive_message(state),
        Action::Trash => trash_message(state),
        Action::ToggleStar => toggle_star(state),
        Action::MarkUnread => mark_unread(state),
        Action::Compose
        | Action::Reply
        | Action::ReplyAll
        | Action::Forward
        | Action::Send
        | Action::LeaveComposer
        | Action::DiscardDraft => {
            // Vocabulary is complete (plan §9); the composer phases own
            // these actions. No-op, never a crash.
            tracing::debug!(?action, "action not yet implemented");
            Vec::new()
        }
        Action::RetryError | Action::DismissError => {
            // Only meaningful with the error overlay open (handled above).
            Vec::new()
        }
        Action::BackendCompleted(result) => backend_completed(state, result),
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

// ── Modal overlay (plan §9/§12) ──────────────────────────────────────────

/// Handle `action` while the error modal is open. Returns `None` when no
/// modal is open (the caller falls through to normal handling).
fn modal_reduce(state: &mut AppState, action: &Action) -> Option<Vec<Effect>> {
    state.overlay.as_ref()?;
    // Scroll budget from the same layout math the renderer uses, so the
    // reducer and the drawn modal always agree on the clamp.
    let (max_scroll, viewport) = match &state.overlay {
        Some(Overlay::Error(dialog)) => {
            let layout = crate::ui::components::error_modal::layout(
                state.size,
                dialog.code,
                dialog.ambiguous,
            );
            (
                crate::ui::components::error_modal::max_scroll(dialog, state.size),
                layout.viewport_lines,
            )
        }
        None => (0, 1),
    };
    let Some(Overlay::Error(dialog)) = state.overlay.as_mut() else {
        return None;
    };
    match action {
        Action::MoveUp => dialog.scroll = dialog.scroll.saturating_sub(1),
        Action::MoveDown => dialog.scroll = (dialog.scroll + 1).min(max_scroll),
        Action::PagePrevious => dialog.scroll = dialog.scroll.saturating_sub(viewport.max(1)),
        Action::PageNext => dialog.scroll = (dialog.scroll + viewport.max(1)).min(max_scroll),
        Action::FocusNext => dialog.button = dialog.button.next(),
        Action::FocusPrevious => dialog.button = dialog.button.previous(),
        Action::BackOrCancel | Action::DismissError => {
            let focus = dialog.previous_focus;
            state.overlay = None;
            state.focus = focus;
        }
        Action::Activate | Action::RetryError => {
            // `RetryError` always retries; `Enter` acts on the focused
            // button (plan §12: Retry replays the intent, Dismiss closes).
            let wants_retry =
                matches!(action, Action::RetryError) || dialog.button == ModalButton::Retry;
            let retry = dialog.retry.clone().filter(|_| wants_retry);
            let focus = dialog.previous_focus;
            if let Some(spec) = retry {
                state.overlay = None;
                state.focus = focus;
                // Retrying replays the equivalent typed intent under a
                // *new* operation id (plan §12; acceptance: new id). The
                // loading placeholders reset so no stale failure text
                // lingers while the replay runs.
                match &spec.kind {
                    OperationKind::LoadMailboxes => state.mailboxes = Loadable::Loading,
                    OperationKind::LoadMessage(_) => {
                        state.open_message = Loadable::Loading;
                        state.reader_scroll = 0;
                    }
                    _ => {}
                }
                return Some(vec![state.operations.start(spec.kind)]);
            }
            if !wants_retry {
                // Enter on Dismiss: close without new work.
                state.overlay = None;
                state.focus = focus;
            }
            // `RetryError` on a non-retryable failure keeps the modal open.
            return Some(Vec::new());
        }
        // Everything else is swallowed while the modal is open.
        _ => {}
    }
    Some(Vec::new())
}

/// Open the Retry/Dismiss modal for a failed operation (plan §12).
fn open_error_modal(state: &mut AppState, failure: &OperationFailure) -> Vec<Effect> {
    tracing::warn!(code = ?failure.code, detail = %failure.detail, "operation failed");
    state.overlay = Some(Overlay::Error(ErrorDialog {
        code: failure.code,
        detail: failure.detail.clone(),
        retry: failure.retry.clone(),
        ambiguous: failure.ambiguous,
        scroll: 0,
        button: ModalButton::Dismiss,
        previous_focus: state.focus,
    }));
    state.focus = Focus::ErrorModal;
    state.set_status("Operation failed");
    Vec::new()
}

// ── Message actions (plan §19 Phase 4) ───────────────────────────────────

/// The message a list/reader shortcut acts on, when one is under focus.
/// Shortcuts are list/reader context (plan §10); they never fire from the
/// sidebar or search field.
fn message_target(state: &AppState) -> Option<MessageLocator> {
    match state.focus {
        Focus::MessageList | Focus::Reader => state.action_target(),
        _ => None,
    }
}

fn archive_message(state: &mut AppState) -> Vec<Effect> {
    match message_target(state) {
        Some(locator) => {
            state.set_status("Archiving…");
            vec![state.operations.start(OperationKind::Archive(locator))]
        }
        None => Vec::new(),
    }
}

fn trash_message(state: &mut AppState) -> Vec<Effect> {
    match message_target(state) {
        Some(locator) => {
            state.set_status("Moving to trash…");
            vec![state.operations.start(OperationKind::Trash(locator))]
        }
        None => Vec::new(),
    }
}

fn toggle_star(state: &mut AppState) -> Vec<Effect> {
    // The target summary carries the current state to invert; the UI only
    // flips once the backend confirms (plan §19 Phase 4 acceptance).
    let Some(locator) = message_target(state) else {
        return Vec::new();
    };
    let starred = match state.focus {
        Focus::Reader => state.open_summary().is_some_and(|s| s.is_starred),
        _ => state.selected_message().is_some_and(|s| s.is_starred),
    };
    vec![state.operations.start(OperationKind::SetStarred {
        locator,
        starred: !starred,
    })]
}

fn mark_unread(state: &mut AppState) -> Vec<Effect> {
    match message_target(state) {
        Some(locator) => vec![state.operations.start(OperationKind::SetRead {
            locator,
            read: false,
        })],
        None => Vec::new(),
    }
}

// ── Backend results (plan §11) ───────────────────────────────────────────

/// Apply a backend result. Results for unknown, cancelled, or superseded
/// operation ids never mutate state: `finish` removes the operation, and a
/// superseded operation was already cancelled and removed when its
/// replacement started.
fn backend_completed(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    let Some(kind) = state.operations.get(result.id).map(|op| op.kind.clone()) else {
        tracing::debug!(id = %result.id, "dropping result for unknown or cancelled operation");
        return Vec::new();
    };
    match &kind {
        OperationKind::LoadMailboxes => {
            state.operations.finish(result.id);
            match &result.outcome {
                Ok(OperationOutcome::Mailboxes(mailboxes)) => {
                    mailboxes_loaded(state, mailboxes.clone())
                }
                Ok(OperationOutcome::Page(_)) => {
                    tracing::warn!(id = %result.id, "page payload for a mailbox operation");
                    Vec::new()
                }
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a mailbox operation");
                    Vec::new()
                }
                Err(failure) => {
                    // The sidebar keeps a dim failed note; the modal carries
                    // the full sanitized detail and the retry intent.
                    state.mailboxes = Loadable::Failed(failure.detail.clone());
                    open_error_modal(state, failure)
                }
            }
        }
        OperationKind::LoadPage(request) => {
            // Currency check: the request must still target the active
            // mailbox. A newer request for the same mailbox superseded this
            // operation, so its id would already be unknown above; this
            // guard drops results that raced a mailbox switch.
            let current = state
                .active_route()
                .and_then(Route::mailbox_id)
                .is_some_and(|id| *id == request.mailbox_id);
            state.operations.finish(result.id);
            if !current {
                tracing::debug!(
                    mailbox = %request.mailbox_id.0,
                    offset = request.offset,
                    "dropping page result for inactive mailbox"
                );
                return Vec::new();
            }
            match &result.outcome {
                Ok(OperationOutcome::Page(page)) => {
                    apply_page(state, page.clone());
                    Vec::new()
                }
                Ok(OperationOutcome::Mailboxes(_)) => {
                    tracing::warn!(id = %result.id, "mailbox payload for a page operation");
                    Vec::new()
                }
                Err(failure) => {
                    // The last coherent page stays visible; the modal offers
                    // Retry/Dismiss (plan §12).
                    open_error_modal(state, failure)
                }
                _ => {
                    tracing::warn!(id = %result.id, "unexpected payload for a page operation");
                    Vec::new()
                }
            }
        }
        OperationKind::LoadMessage(locator) => {
            // Currency check: the reader must still show this message.
            let current = matches!(
                state.active_route(),
                Some(Route::Message(route))
                    if route.mailbox_id == locator.mailbox && route.summary.id == locator.id
            );
            state.operations.finish(result.id);
            if !current {
                tracing::debug!(
                    id = %result.id,
                    mailbox = %locator.mailbox.0,
                    "dropping message result for a closed reader"
                );
                return Vec::new();
            }
            match &result.outcome {
                Ok(OperationOutcome::Message(message)) => {
                    message_loaded(state, (**message).clone())
                }
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a message operation");
                    Vec::new()
                }
                Err(failure) => {
                    // The reader shows a failure placeholder; the modal
                    // carries Retry/Dismiss (plan §12). Coherent state.
                    state.open_message = Loadable::Failed(failure.detail.clone());
                    open_error_modal(state, failure)
                }
            }
        }
        OperationKind::SetRead { locator, read } => {
            state.operations.finish(result.id);
            match &result.outcome {
                // The flag commands echo affected flags, not resulting state
                // (ADR 0001 finding 6): the confirmed request is the state.
                Ok(OperationOutcome::Done) => {
                    apply_flag(state, locator, FlagChange::Read(*read));
                }
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a flag operation");
                }
                Err(failure) => {
                    open_error_modal(state, failure);
                }
            }
            Vec::new()
        }
        OperationKind::SetStarred { locator, starred } => {
            state.operations.finish(result.id);
            match &result.outcome {
                Ok(OperationOutcome::Done) => {
                    apply_flag(state, locator, FlagChange::Starred(*starred));
                }
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a flag operation");
                }
                Err(failure) => {
                    open_error_modal(state, failure);
                }
            }
            Vec::new()
        }
        OperationKind::Archive(locator) | OperationKind::Trash(locator) => {
            state.operations.finish(result.id);
            match &result.outcome {
                Ok(OperationOutcome::Done) => message_moved(state, locator),
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a move operation");
                    Vec::new()
                }
                Err(failure) => {
                    // Nothing was removed locally: the list/reader still
                    // show the message (plan §12 coherent failure state).
                    open_error_modal(state, failure)
                }
            }
        }
    }
}

/// Local flag application after a confirmed flag operation. The list row
/// and the reader's summary snapshot update together so the next `Esc` does
/// not resurrect stale metadata.
fn apply_flag(state: &mut AppState, locator: &MessageLocator, change: FlagChange) {
    let matches = |summary: &crate::domain::MessageSummary| {
        summary.id == locator.id
            || locator
                .message_id
                .as_ref()
                .is_some_and(|mid| summary.message_id.as_ref() == Some(mid))
    };
    if let Some(Route::Message(route)) = state.routes.last_mut()
        && matches(&route.summary)
    {
        match change {
            FlagChange::Read(read) => route.summary.is_read = read,
            FlagChange::Starred(starred) => route.summary.is_starred = starred,
        }
    }
    for summary in state.messages.items.iter_mut().filter(|s| matches(s)) {
        match change {
            FlagChange::Read(read) => summary.is_read = read,
            FlagChange::Starred(starred) => summary.is_starred = starred,
        }
    }
}

enum FlagChange {
    Read(bool),
    Starred(bool),
}

/// Apply the fetched message: show it, fill the list snippet (Post fills
/// snippets only from full fetches, see map.rs), and mark unread mail read
/// after successful load (plan §19 Phase 4) as a separate, retryable flag
/// operation whose confirmation updates the list.
fn message_loaded(state: &mut AppState, message: Message) -> Vec<Effect> {
    let snippet = message.snippet();
    let message_id = message.id.clone();
    state.open_message = Loadable::Loaded(message);
    if let Some(snippet) = snippet {
        if let Some(Route::Message(route)) = state.routes.last_mut()
            && route.summary.id == message_id
            && route.summary.snippet.is_none()
        {
            route.summary.snippet = Some(snippet.clone());
        }
        if let Some(summary) = state
            .messages
            .items
            .iter_mut()
            .find(|s| s.id == message_id && s.snippet.is_none())
        {
            summary.snippet = Some(snippet);
        }
    }
    // The summary in the route knows the read state; only an unread message
    // triggers the flag operation (plan §19 Phase 4: mark read after load).
    let Some(Route::Message(route)) = state.active_route() else {
        return Vec::new();
    };
    if route.summary.is_read {
        return Vec::new();
    }
    let locator = MessageLocator {
        mailbox: route.mailbox_id.clone(),
        id: route.summary.id.clone(),
        message_id: route.summary.message_id.clone(),
    };
    vec![state.operations.start(OperationKind::SetRead {
        locator,
        read: true,
    })]
}

/// A confirmed move (archive/trash): drop the row from the displayed page,
/// keep the selection index on what took its place, close the reader if it
/// was showing the moved message, and re-sync the page in the background so
/// pagination stays truthful (maildir ids change on move, ADR 0001 finding
/// 4; the reload re-resolves the selection by identity).
fn message_moved(state: &mut AppState, locator: &MessageLocator) -> Vec<Effect> {
    let matches = |summary: &crate::domain::MessageSummary| {
        summary.id == locator.id
            || locator
                .message_id
                .as_ref()
                .is_some_and(|mid| summary.message_id.as_ref() == Some(mid))
    };
    // Close the reader when it was showing the moved message.
    if matches!(state.active_route(), Some(Route::Message(route)) if matches(&route.summary)) {
        close_reader(state);
    }
    state.messages.items.retain(|summary| !matches(summary));
    state.selection = state
        .selection
        .min(state.messages.items.len().saturating_sub(1));
    keep_selection_visible(state);
    state.set_status("Message moved");
    request_page(state, state.messages.offset)
}

/// Apply the mailbox listing: select the Inbox, or the first mailbox when
/// no Inbox exists, and load its first page.
fn mailboxes_loaded(state: &mut AppState, mailboxes: Vec<Mailbox>) -> Vec<Effect> {
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
            Vec::new()
        }
    }
}

/// Apply a page result for the active mailbox. The selection is
/// re-resolved by `Message-ID` first, then backend id (ADR 0001 finding 4:
/// only the Message-ID is stable across moves).
fn apply_page(state: &mut AppState, page: Page<crate::domain::MessageSummary>) -> Vec<Effect> {
    let previous = state.selected_message();
    let previous_message_id = previous.and_then(|m| m.message_id.clone());
    let previous_id = previous.map(|m| m.id.clone());
    state.messages = page;
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
    Vec::new()
}

// ── Navigation and input ─────────────────────────────────────────────────

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
        Focus::Reader => scroll_reader(state, delta),
        Focus::ErrorModal => {}
    }
    Vec::new()
}

/// Left/Right: pages in the message list, viewport steps in the reader.
fn page_step(state: &mut AppState, delta: i64) -> Vec<Effect> {
    match state.focus {
        Focus::Reader => {
            let viewport = crate::ui::layout::reader_rows_visible(state.size).max(1) as i64;
            scroll_reader(state, delta * viewport);
            Vec::new()
        }
        _ => change_page(state, delta),
    }
}

/// Scroll the reader document (Up/Down in reader focus, plan §10: "scroll
/// focused area"). The line budget comes from the same pure content
/// function the renderer draws, so the reducer's clamp always matches the
/// frame.
fn scroll_reader(state: &mut AppState, delta: i64) {
    let viewport = crate::ui::layout::reader_rows_visible(state.size).max(1) as i64;
    let total =
        crate::ui::screens::reader::content_line_count(state, state.size.0.max(1) as usize) as i64;
    let max = (total - viewport).max(0);
    let next = (state.reader_scroll as i64 + delta).clamp(0, max);
    state.reader_scroll = next as usize;
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
    let base = match state.active_route().and_then(Route::mailbox_id) {
        Some(id) => state
            .operations
            .page_in_flight(id)
            .map(|pending| pending.offset as i64),
        None => None,
    }
    .unwrap_or(state.messages.offset as i64);
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

/// Start a page load for the active route's mailbox and return its effect.
/// The registry supersedes any page request still in flight for the same
/// mailbox (and cancels it), so only the newest result can win.
fn request_page(state: &mut AppState, offset: usize) -> Vec<Effect> {
    let Some(mailbox_id) = state.active_route().and_then(Route::mailbox_id).cloned() else {
        return Vec::new();
    };
    let request = PageRequest {
        mailbox_id,
        offset,
        limit: state.messages.limit.max(1),
    };
    vec![state.operations.start(OperationKind::LoadPage(request))]
}

fn activate(state: &mut AppState) -> Vec<Effect> {
    match state.focus {
        Focus::Sidebar => match state.selected_mailbox().cloned() {
            Some(mailbox) => switch_mailbox(state, &mailbox.id),
            None => Vec::new(),
        },
        Focus::MessageList => match state.selected_message().cloned() {
            Some(summary) => open_message(state, summary),
            None => Vec::new(),
        },
        Focus::Reader | Focus::SearchField | Focus::ErrorModal => {
            if state.focus == Focus::SearchField {
                reduce(state, &Action::SubmitSearch)
            } else {
                Vec::new()
            }
        }
    }
}

/// Open the selected message: push the reader route, snapshot the summary,
/// and fetch the full message. The list page, selection, and scroll stay
/// untouched so `Esc` restores them exactly (plan §19 Phase 4).
fn open_message(state: &mut AppState, summary: crate::domain::MessageSummary) -> Vec<Effect> {
    let mailbox_id = summary.mailbox_id.clone();
    let locator = MessageLocator {
        mailbox: mailbox_id.clone(),
        id: summary.id.clone(),
        message_id: summary.message_id.clone(),
    };
    state.routes.push(Route::Message(MessageRoute {
        mailbox_id,
        summary,
    }));
    state.focus = Focus::Reader;
    state.open_message = Loadable::Loading;
    state.reader_scroll = 0;
    vec![state.operations.start(OperationKind::LoadMessage(locator))]
}

/// Close the reader if it is open: pop its route and drop its data. The
/// mailbox route underneath was never mutated, so page, selection, focus,
/// and scroll are restored by construction.
fn close_reader(state: &mut AppState) {
    if matches!(state.active_route(), Some(Route::Message(_))) {
        state.routes.pop();
        state.open_message = Loadable::Idle;
        state.reader_scroll = 0;
        state.focus = Focus::MessageList;
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
    // A reader (or composer, later) open on top is replaced by the new
    // mailbox; its data must not linger.
    if state.routes.pop().is_some() {
        state.open_message = Loadable::Idle;
        state.reader_scroll = 0;
    }
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

/// `Esc`: cancel foreground work, close an overlay, or go back — in that
/// order (plan §10). Cancelling returns to the prior stable state: the
/// displayed page/list was never cleared while the request ran.
fn back_or_cancel(state: &mut AppState) {
    if let Some(op) = state.operations.cancel_foreground() {
        tracing::info!(id = %op.id, kind = ?op.kind, "cancelled foreground operation");
        state.set_status(format!("{} — cancelled", op.kind.summary()));
        return;
    }
    if let Some(Overlay::Error(dialog)) = state.overlay.take() {
        state.focus = dialog.previous_focus;
        return;
    }
    if state.focus == Focus::SearchField {
        state.focus = Focus::MessageList;
        return;
    }
    if state.routes.len() > 1 {
        // Pop the reader (or a later screen): the mailbox route underneath
        // still holds the exact page, selection, and scroll.
        state.routes.pop();
        state.open_message = Loadable::Idle;
        state.reader_scroll = 0;
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

/// Manual refresh (`Ctrl+R`): (re)load the mailbox listing while startup
/// has not completed, otherwise refresh the visible page.
fn refresh(state: &mut AppState) -> Vec<Effect> {
    if !matches!(state.mailboxes, Loadable::Loaded(_)) {
        if state.operations.is_loading_mailboxes() {
            return Vec::new();
        }
        return vec![state.operations.start(OperationKind::LoadMailboxes)];
    }
    if state.active_route().is_none() {
        return Vec::new();
    }
    state.set_status("Refreshing…");
    request_page(state, state.messages.offset)
}

#[cfg(test)]
#[path = "reducer_tests.rs"]
mod tests;
