//! Message-shaped backend results (plan §11): flag application, reader
//! focus, the loaded/moved message application, the mailbox listing and
//! page application, and the list previews (ticket wxtx).
use super::navigation::{
    close_reader, keep_selection_visible, reader_scroll_bounds, request_page, request_visible_page,
};
use super::results::unexpected_payload;
use crate::app::effect::Effect;
use crate::app::operation::{OperationKind, OperationOutcome, OperationResult};
use crate::app::route::{MailboxRoute, Route};
use crate::app::state::{AppState, Loadable, ReaderFocus};
use crate::domain::{Mailbox, MailboxId, MailboxRole, Message, MessageLocator, Page};

/// Local flag application after a confirmed flag operation. The list row
/// and the reader's summary snapshot update together so the next `Esc` does
/// not resurrect stale metadata.
pub(crate) fn apply_flag(state: &mut AppState, locator: &MessageLocator, change: FlagChange) {
    let matches = |summary: &crate::domain::MessageSummary| {
        summary.id == locator.id
            || locator
                .message_id
                .as_ref()
                .is_some_and(|mid| summary.message_id.as_ref() == Some(mid))
    };
    if let Some(Route::Message(route)) = state.session.routes.last_mut()
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

pub(crate) enum FlagChange {
    Read(bool),
    Starred(bool),
}

/// Move the reader's focus cursor (Tab/Shift+Tab, plan §15, tickets
/// 1fnh/hc9n). The cycle walks the body's links in document order first,
/// then the attachment chips, wrapping at both ends. With nothing focused,
/// forward starts at the first item and backward at the last; with no
/// focusable item at all the cursor stays put.
pub(crate) fn cycle_reader_focus(state: &mut AppState, delta: i64) {
    let width = crate::view::layout::reader_width(state.session.size).max(10);
    let links = crate::app::reader::link_count(state, width);
    let attachments = state
        .open_message
        .as_loaded()
        .map(|message| message.attachments.len())
        .unwrap_or(0);
    let total = links + attachments;
    if total == 0 {
        return;
    }
    let current = match state.reader_focus {
        Some(ReaderFocus::Link(index)) if index < links => Some(index),
        Some(ReaderFocus::Attachment(index)) if index < attachments => Some(links + index),
        // A stale index (the document reflowed or the message changed
        // under it) restarts from the cycle's head.
        Some(_) | None => None,
    };
    let next = match current {
        Some(position) => (position as i64 + delta).rem_euclid(total as i64) as usize,
        None if delta > 0 => 0,
        None => total - 1,
    };
    state.reader_focus = Some(if next < links {
        ReaderFocus::Link(next)
    } else {
        ReaderFocus::Attachment(next - links)
    });
    reveal_reader_focus(state);
}

/// Scroll the reader so the focused item is visible (tickets hc9n/1fnh):
/// tabbing to a link or chip below the fold must bring it into view. The
/// bounds come from the same document the renderer draws.
pub(crate) fn reveal_reader_focus(state: &mut AppState) {
    let Some(focus) = state.reader_focus else {
        return;
    };
    let width = crate::view::layout::reader_width(state.session.size).max(10);
    let Some(line) = crate::app::reader::focus_line(state, width, focus) else {
        return;
    };
    let (viewport, total) = reader_scroll_bounds(state);
    let viewport = viewport.max(1) as usize;
    if line < state.reader_scroll {
        state.reader_scroll = line;
    } else if line >= state.reader_scroll + viewport {
        state.reader_scroll = line + 1 - viewport;
    }
    let max = (total - viewport as i64).max(0);
    state.reader_scroll = (state.reader_scroll as i64).clamp(0, max) as usize;
}

/// The attachment the reader's save/open keys act on, when the open
/// message carries any (plan §15). With no attachment focused (or a link
/// focused) the first chip is the target, as before.
pub(crate) fn selected_attachment(state: &AppState) -> Option<(usize, &crate::domain::Attachment)> {
    let message = state.open_message.as_loaded()?;
    let index = match state.reader_focus {
        Some(ReaderFocus::Attachment(index)) => index,
        _ => 0,
    }
    .min(message.attachments.len().saturating_sub(1));
    let attachment = message.attachments.get(index)?;
    Some((index, attachment))
}

/// Apply the fetched message: show it, fill the list snippet (the same
/// one-line body preview the background previews produce, ticket wxtx),
/// and mark unread mail read after successful load (plan §19 Phase 4) as a
/// separate, retryable flag operation whose confirmation updates the list.
pub(crate) fn message_loaded(state: &mut AppState, message: Message) -> Vec<Effect> {
    let snippet = crate::view::rich::preview_text(&message);
    let message_id = message.id.clone();
    let has_attachments = !message.attachments.is_empty();
    // The message was fully fetched this session: never re-requested for
    // a preview, even when its body carries no preview text.
    state.caches.preview_requested.insert(message_id.clone());
    // Ticket haeb: cache the viewed message (bounded by [tmail.cache]) —
    // as an effect, so the write never blocks the reducer.
    let mut effects = Vec::new();
    if let Some(Route::Message(route)) = state.active_route() {
        effects.push(
            state
                .session
                .operations
                .start_background(OperationKind::CacheMessageStore {
                    mailbox: route.mailbox_id.clone(),
                    id: message_id.0.clone(),
                    message: Box::new(message.clone()),
                }),
        );
    }
    state.open_message = Loadable::Loaded(message);
    // The parsed message knows attachments better than the envelope did
    // (ticket r84f: IMAP envelopes carry no body structure, so the flag
    // was false and the paperclip never rendered).
    sync_row_attachments(state, &message_id, has_attachments);
    if let Some(snippet) = snippet {
        // The session keeps the preview: page loads and refreshes restore
        // it instead of re-fetching the message (ticket wxtx).
        state
            .caches
            .previews
            .insert(message_id.clone(), snippet.clone());
        if let Some(Route::Message(route)) = state.session.routes.last_mut()
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
        return effects;
    };
    if route.summary.is_read {
        return effects;
    }
    let locator = route.summary.into_locator();
    effects.push(state.session.operations.start(OperationKind::SetRead {
        locator,
        read: true,
    }));
    effects
}

/// A confirmed move (archive/trash): drop the row from the displayed page,
/// keep the selection index on what took its place, close the reader if it
/// was showing the moved message, and re-sync the page in the background so
/// pagination stays truthful (maildir ids change on move, ADR 0001 finding
/// 4; the reload re-resolves the selection by identity).
pub(crate) fn message_moved(state: &mut AppState, locator: &MessageLocator) -> Vec<Effect> {
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
    // A moved row leaves the bulk selection with it (ticket p0s3): only
    // the moved ids are pruned, selections on other pages stay.
    let moved_ids: Vec<_> = state
        .messages
        .items
        .iter()
        .filter(|s| matches(s))
        .map(|s| s.id.clone())
        .collect();
    state.messages.items.retain(|summary| !matches(summary));
    for id in moved_ids {
        state.selected.remove(&id);
    }
    state.selection = state
        .selection
        .min(state.messages.items.len().saturating_sub(1));
    keep_selection_visible(state);
    state.set_status("Message moved");
    // The visible context could be a mailbox page or search results
    // (Phase 9); the re-sync follows whichever is open.
    request_visible_page(state, state.messages.offset)
}

/// Reconcile one list row's attachment flag with a full message (ticket
/// r84f): envelope listings may not carry the flag (IMAP accounts — the
/// envelope carries no body structure), but every parsed message —
/// fetched for a preview or a read — knows the truth.
pub(crate) fn sync_row_attachments(
    state: &mut AppState,
    id: &crate::domain::MessageId,
    has_attachments: bool,
) {
    if let Some(summary) = state.messages.items.iter_mut().find(|s| &s.id == id) {
        summary.has_attachments = has_attachments;
    }
}

/// Apply the fetched mailbox listing (plan §19 Phase 2). Cold start — no
/// mailbox displayed yet — picks the Inbox (or first usable), roots the
/// route stack there, and loads its first page. Warm start — a mailbox is
/// already displayed (the cached listing at startup, ticket haeb, or a
/// previous load) — updates the sidebar data in place only: routes, the
/// visible page, the list selection, and the scroll are never touched, so
/// a background listing can never kick the user out of the composer or
/// reset their cursor (ticket sazy).
pub(crate) fn mailboxes_loaded(state: &mut AppState, mailboxes: Vec<Mailbox>) -> Vec<Effect> {
    // Cold start: no mailbox context established yet (empty stack). Warm
    // start: the stack is rooted at a mailbox — possibly under a reader,
    // search, or composer the user opened while the fetch ran; those all
    // stay.
    if !matches!(state.session.routes.first(), Some(Route::Mailbox(_))) {
        state.mailboxes = Loadable::Loaded(mailboxes.clone());
        return apply_mailbox_listing(state, mailboxes);
    }
    refresh_sidebar_listing(state, mailboxes);
    Vec::new()
}

/// Warm-start sidebar refresh (ticket sazy): swap the listing while keeping
/// the sidebar cursor on the same mailbox identity — a fresh enumeration
/// may order folders differently than the cached or previous one. The
/// mailbox the cursor points at wins; the displayed mailbox is the
/// fallback; with neither present in the fresh list, the index is simply
/// kept inside it.
pub(crate) fn refresh_sidebar_listing(state: &mut AppState, mailboxes: Vec<Mailbox>) {
    let cursor_id = state
        .mailboxes
        .as_loaded()
        .and_then(|list| list.get(state.mailbox_selection))
        .map(|m| m.id.clone());
    // The mailbox the UI is rooted at (the warm path guarantees one).
    let active_id = match state.session.routes.first() {
        Some(Route::Mailbox(route)) => Some(route.mailbox_id.clone()),
        _ => None,
    };
    let pointed = cursor_id
        .filter(|id| mailboxes.iter().any(|m| &m.id == id))
        .or_else(|| active_id.filter(|id| mailboxes.iter().any(|m| &m.id == id)));
    let len = mailboxes.len();
    state.mailboxes = Loadable::Loaded(mailboxes);
    state.mailbox_selection = match pointed {
        // The same mailbox, possibly at a new position.
        Some(id) => state
            .mailboxes
            .as_loaded()
            .and_then(|list| list.iter().position(|m| m.id == id))
            .unwrap_or(0),
        // Neither the cursor's nor the displayed mailbox exists anymore:
        // keep the index inside the fresh list.
        None => state.mailbox_selection.min(len.saturating_sub(1)),
    };
}

/// Shared body of the mailbox application (plan §19 Phase 2): pick the
/// inbox (or first usable), root the route stack there, and request its
/// first page. Used by the fresh listing and by the cached listing at
/// cold start (ticket haeb).
pub(crate) fn apply_mailbox_listing(state: &mut AppState, mailboxes: Vec<Mailbox>) -> Vec<Effect> {
    let chosen = mailboxes
        .iter()
        .position(|m| m.role == Some(MailboxRole::Inbox))
        .or_else(|| (!mailboxes.is_empty()).then_some(0));
    match chosen {
        Some(index) => {
            let mailbox_id = mailboxes[index].id.clone();
            state.session.routes = vec![Route::Mailbox(MailboxRoute { mailbox_id })];
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
            state.session.routes.clear();
            state.messages = Page::empty(state.messages.limit);
            Vec::new()
        }
    }
}

/// The mailbox whose page the visible list currently shows, when it shows
/// one: the nearest mailbox route on the stack. Reader and composer routes
/// are overlays over the list beneath them (the walk continues past them),
/// so a page load finishing behind them still applies (ticket sazy). A
/// search route owns the visible list with query results — a mailbox page
/// must never land there, so the walk stops.
pub(crate) fn visible_mailbox_page(state: &AppState) -> Option<&MailboxId> {
    for route in state.session.routes.iter().rev() {
        match route {
            Route::Mailbox(route) => return Some(&route.mailbox_id),
            Route::Search(_) => return None,
            Route::Message(_) | Route::Composer | Route::Wizard => {}
        }
    }
    None
}

/// Apply a page result for the active mailbox. The selection is
/// re-resolved by `Message-ID` first, then backend id (ADR 0001 finding 4:
/// only the Message-ID is stable across moves). Rows without a body
/// preview start their background preview fetches (ticket wxtx).
pub(crate) fn apply_page(
    state: &mut AppState,
    page: Page<crate::domain::MessageSummary>,
) -> Vec<Effect> {
    // Ticket sazy: an identical page changes nothing observable — keep the
    // user's selection, scroll, and preview bookkeeping exactly as they
    // are instead of re-resolving over the same rows. Fresh envelope
    // listings carry no snippets, so a page the session already decorated
    // never compares equal here; equality means genuinely unchanged data.
    if page == state.messages {
        return Vec::new();
    }
    let previous = state.selected_message();
    let previous_message_id = previous.and_then(|m| m.message_id.clone());
    let previous_id = previous.map(|m| m.id.clone());
    // The envelope's attachment flag is absent on IMAP listings (ticket
    // r84f): a fresh page must not wipe the flag the fetched messages
    // established this session (previews, opens), or the paperclip would
    // vanish on every background refresh.
    let reconciled_flags: std::collections::HashMap<crate::domain::MessageId, bool> = state
        .messages
        .items
        .iter()
        .map(|s| (s.id.clone(), s.has_attachments))
        .collect();
    state.messages = page;
    for item in &mut state.messages.items {
        if !item.has_attachments
            && let Some(previous) = reconciled_flags.get(&item.id)
        {
            item.has_attachments = *previous;
        }
    }
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
    // A fresh envelope listing carries no snippets (ADR 0001 finding 2),
    // so restore what this session already previewed: a periodic refresh
    // or a page change must render the same previews it replaces, never
    // clear them (ticket wxtx).
    for item in &mut state.messages.items {
        if item.snippet.is_none()
            && let Some(text) = state.caches.previews.get(&item.id)
        {
            item.snippet = Some(text.clone());
        }
    }
    start_missing_previews(state)
}

// ── List previews (ticket wxtx) ──────────────────────────────────────────

/// How many preview fetches may run at once: a page can hold dozens of
/// rows, and each fetch spawns a himalaya child, so the window rolls —
/// queued rows start as in-flight fetches complete.
pub(crate) const MAX_IN_FLIGHT_PREVIEWS: usize = 6;

/// Satisfy the visible rows' previews (ticket wxtx): rows without a
/// snippet get one cache read each (off-thread, ticket haeb). A read that
/// hits serves the preview straight from the cached copy — no backend
/// work, and old cached messages are never re-fetched; a miss may start a
/// background fetch within the rolling window (decided at completion
/// time, against the live in-flight count).
pub(crate) fn start_missing_previews(state: &mut AppState) -> Vec<Effect> {
    state
        .messages
        .items
        .iter()
        .filter(|s| s.snippet.is_none() && !state.caches.preview_requested.contains(&s.id))
        .map(|summary| {
            state
                .session
                .operations
                .start_background(OperationKind::CachePreviewLoad {
                    locator: summary.into_locator(),
                })
        })
        .collect()
}

/// Apply a served preview-cache read (ticket wxtx): a hit fills the row's
/// snippet and attachment flag from the cached copy; a miss starts a
/// background fetch when the rolling window has room. A result for a row
/// no longer listed is dropped.
pub(crate) fn complete_cache_preview_load(
    state: &mut AppState,
    locator: &MessageLocator,
    result: OperationResult,
) -> Vec<Effect> {
    let Some(summary) = state
        .messages
        .items
        .iter()
        .find(|s| s.id == locator.id)
        .cloned()
    else {
        tracing::debug!(id = %locator.id.0, "preview cache read for an unlisted row");
        return Vec::new();
    };
    match result.outcome {
        Ok(OperationOutcome::CachedMessage(message)) => {
            state.caches.preview_requested.insert(summary.id.clone());
            match crate::view::rich::preview_text(&message) {
                Some(text) => {
                    state
                        .caches
                        .previews
                        .insert(summary.id.clone(), text.clone());
                    if let Some(item) = state.messages.items.iter_mut().find(|s| s.id == summary.id)
                    {
                        item.snippet = Some(text);
                        item.has_attachments = !message.attachments.is_empty();
                    }
                }
                None => {
                    // The cached body carries no preview text (an empty
                    // body): satisfied, nothing to fetch. The attachment
                    // flag still reconciles from the cached copy.
                    sync_row_attachments(state, &summary.id, !message.attachments.is_empty());
                }
            }
            Vec::new()
        }
        Ok(OperationOutcome::CacheMiss) => {
            // Genuinely unknown: fetch in the background, once, within
            // the rolling window (the live count, so concurrent
            // completions cannot overshoot).
            if state.session.operations.previews_in_flight() >= MAX_IN_FLIGHT_PREVIEWS {
                return Vec::new();
            }
            state.caches.preview_requested.insert(summary.id.clone());
            vec![
                state
                    .session
                    .operations
                    .start_background(OperationKind::Preview(summary.into_locator())),
            ]
        }
        Ok(_) => unexpected_payload(result.id, "cached message"),
        Err(_) => {
            // Cache reads never fail (a broken cache is a miss); this arm
            // only keeps the match total.
            Vec::new()
        }
    }
}

/// Apply a fetched preview (ticket wxtx): convert the body to one line of
/// plain text, remember it for the session, fill the list row's snippet,
/// cache the message for an instant open, and roll the fetch window. A
/// result for a message no longer listed (mailbox switched, row moved) is
/// dropped — but still cached, so it helps if the message returns.
pub(crate) fn preview_loaded(state: &mut AppState, message: Message) -> Vec<Effect> {
    // The message was fully fetched this session: never re-requested for
    // a preview, even when its body carries no preview text (marking here
    // also closes the race with the store effect below — a cache read
    // that ran before the write would otherwise re-fetch it).
    state.caches.preview_requested.insert(message.id.clone());
    // Ticket haeb: cache the fetched message (bounded by [tmail.cache]) —
    // as an effect, so the write never blocks the reducer.
    let mut effects =
        vec![
            state
                .session
                .operations
                .start_background(OperationKind::CacheMessageStore {
                    mailbox: message.mailbox_id.clone(),
                    id: message.id.0.clone(),
                    message: Box::new(message.clone()),
                }),
        ];
    let message_id = message.id.clone();
    if let Some(text) = crate::view::rich::preview_text(&message) {
        state
            .caches
            .previews
            .insert(message_id.clone(), text.clone());
    }
    // The parsed message knows attachments better than the envelope did
    // (ticket r84f: IMAP envelopes carry no body structure, so the flag
    // was false and the paperclip never rendered). The row's snippet,
    // when still missing, fills from the preview computed above.
    let has_attachments = !message.attachments.is_empty();
    if let Some(summary) = state.messages.items.iter_mut().find(|s| s.id == message_id) {
        summary.has_attachments = has_attachments;
        if summary.snippet.is_none() {
            summary.snippet = state.caches.previews.get(&message_id).cloned();
        }
    }
    effects.extend(start_missing_previews(state));
    effects
}
