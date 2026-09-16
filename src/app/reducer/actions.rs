//! Message actions (plan §19 Phase 4): archive/trash, read/unread,
//! star, bulk selection (ticket p0s3), and attachment save/open
//! (plan §15, Phase 8.4).
use super::message_results::selected_attachment;
use super::navigation::keep_selection_visible;
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::OperationKind;
use crate::app::route::Route;
use crate::app::state::{AppState, ReaderFocus};
use crate::domain::MessageLocator;
use crate::domain::sanitize::sanitize;

// ── Message actions (plan §19 Phase 4) ───────────────────────────────────

/// The message a list/reader shortcut acts on, when one is under focus.
/// Shortcuts are list/reader context (plan §10); they never fire from the
/// sidebar or search field.
pub(crate) fn message_target(state: &AppState) -> Option<MessageLocator> {
    match state.session.focus {
        Focus::MessageList | Focus::Reader => state.action_target(),
        _ => None,
    }
}

/// Locators for a bulk operation (ticket p0s3): only when the list holds
/// focus, selection mode is on, and the visible selection is non-empty.
/// Reader shortcuts keep acting on the open message even while a selection
/// exists — the selection belongs to the list behind the reader.
pub(crate) fn bulk_targets(state: &AppState) -> Option<Vec<MessageLocator>> {
    if state.session.focus != Focus::MessageList || !state.selection_active() {
        return None;
    }
    let locators = state.selected_locators();
    (!locators.is_empty()).then_some(locators)
}

pub(crate) fn archive_message(state: &mut AppState) -> Vec<Effect> {
    bulk_or_single(
        state,
        "Archiving {count} messages…",
        "Archiving…",
        false,
        OperationKind::Archive,
    )
}

pub(crate) fn trash_message(state: &mut AppState) -> Vec<Effect> {
    bulk_or_single(
        state,
        "Moving {count} messages to trash…",
        "Moving to trash…",
        false,
        OperationKind::Trash,
    )
}

/// Mark read (ticket p0s3): the whole selection in selection mode, else the
/// focused row (the list has no read shortcut today; the reader marks read
/// on open, so the single path stays list-only). The bulk path starts ONE
/// batched operation (ticket aavy): N per-message effects would spawn N
/// concurrent himalaya processes — the IMAP fanout that tripped the
/// server's throttling.
pub(crate) fn mark_read(state: &mut AppState) -> Vec<Effect> {
    if let Some(locators) = bulk_targets(state) {
        let count = locators.len();
        state.set_status(format!("Marking {count} messages read…"));
        return vec![state.session.operations.start(OperationKind::SetReadBulk {
            locators,
            read: true,
        })];
    }
    single_flag(state, true, "Marking read…", true)
}

/// The unread counterpart (ticket p0s3).
pub(crate) fn mark_unread(state: &mut AppState) -> Vec<Effect> {
    if let Some(locators) = bulk_targets(state) {
        let count = locators.len();
        state.set_status(format!("Marking {count} messages unread…"));
        return vec![state.session.operations.start(OperationKind::SetReadBulk {
            locators,
            read: false,
        })];
    }
    single_flag(state, false, "Marking unread…", false)
}

/// The focused-row read-flag change (the single path of mark read/unread).
/// `list_only` keeps the shortcut from firing over the reader when the
/// action is list-only (mark read: the reader marks read on open itself).
fn single_flag(state: &mut AppState, read: bool, status: &str, list_only: bool) -> Vec<Effect> {
    if list_only && state.session.focus != Focus::MessageList {
        return Vec::new();
    }
    match message_target(state) {
        Some(locator) => {
            state.set_status(status);
            vec![
                state
                    .session
                    .operations
                    .start(OperationKind::SetRead { locator, read }),
            ]
        }
        None => Vec::new(),
    }
}

/// The target summary carries the current state to invert; the UI only
/// flips once the backend confirms (plan §19 Phase 4 acceptance).
pub(crate) fn toggle_star(state: &mut AppState) -> Vec<Effect> {
    let Some(locator) = message_target(state) else {
        return Vec::new();
    };
    let starred = match state.session.focus {
        Focus::Reader => state.open_summary().is_some_and(|s| s.is_starred),
        _ => state.selected_message().is_some_and(|s| s.is_starred),
    };
    vec![state.session.operations.start(OperationKind::SetStarred {
        locator,
        starred: !starred,
    })]
}

/// The shared "bulk in selection mode, else the focused row" skeleton of
/// the move operations (ticket p0s3): builds one [`OperationKind`] per
/// target from `make`, and forms the status message — `bulk` templates one
/// `{count}` plural in bulk mode, `single` is the fixed phrase for one
/// message. `list_only` keeps the single path from firing over the reader
/// (archive/trash are list shortcuts today); the bulk path always implies
/// list focus. Bulk mark read/unread bypasses this skeleton: they start
/// one batched operation instead (ticket aavy).
pub(crate) fn bulk_or_single(
    state: &mut AppState,
    bulk: &str,
    single: &str,
    list_only: bool,
    make: impl Fn(MessageLocator) -> OperationKind,
) -> Vec<Effect> {
    if let Some(locators) = bulk_targets(state) {
        state.set_status(bulk.replace("{count}", &locators.len().to_string()));
        // start_unless_duplicate: archive/trash of the same message may
        // not run twice concurrently — a double-press coalesces into the
        // request already in flight (the registry keeps the first).
        return locators
            .into_iter()
            .filter_map(|locator| {
                state
                    .session
                    .operations
                    .start_unless_duplicate(make(locator))
            })
            .collect();
    }
    if list_only && state.session.focus != Focus::MessageList {
        return Vec::new();
    }
    match message_target(state) {
        Some(locator) => {
            state.set_status(single);
            state
                .session
                .operations
                .start_unless_duplicate(make(locator))
                .into_iter()
                .collect()
        }
        None => Vec::new(),
    }
}

// ── Bulk selection (ticket p0s3) ─────────────────────────────────────────

/// Space on a focused message row: toggle its bulk-selection mark, then
/// advance the cursor to the next row (ticket yy4m) so several messages
/// can be marked by pressing Space repeatedly. The mark rides the backend
/// id, so it survives paging and refreshes while the row stays listed.
pub(crate) fn toggle_selected(state: &mut AppState) -> Vec<Effect> {
    if state.session.focus != Focus::MessageList {
        return Vec::new();
    }
    let Some(summary) = state.selected_message() else {
        return Vec::new();
    };
    let id = summary.id.clone();
    if !state.selected.remove(&id) {
        state.selected.insert(id);
    }
    if state.selection + 1 < state.messages.items.len() {
        state.selection += 1;
        keep_selection_visible(state);
    }
    Vec::new()
}

/// Ctrl+A: select every visible message, or clear the selection when all
/// visible rows are already marked. Needs a visible list — it never fires
/// over the reader.
pub(crate) fn toggle_select_all(state: &mut AppState) -> Vec<Effect> {
    if !matches!(
        state.active_route(),
        Some(Route::Mailbox(_) | Route::Search(_))
    ) {
        return Vec::new();
    }
    if state.all_visible_selected() {
        state.selected.clear();
        state.set_status("Selection cleared");
    } else {
        let ids: Vec<_> = state.messages.items.iter().map(|m| m.id.clone()).collect();
        for id in ids {
            state.selected.insert(id);
        }
        let count = state.messages.items.len();
        state.set_status(format!("{count} messages selected"));
    }
    Vec::new()
}

// ── Attachment save (plan §15, Phase 8.4) ────────────────────────────────

/// Save the selected reader attachment (plan §15). Reader-only: the open
/// message supplies the locator and the part id, the chip cursor supplies
/// the attachment. `d` saves into the downloads directory; the backend
/// owns collision handling and returns the path actually written. `o`
/// (open) reuses a path saved this session or saves first, then chains
/// the platform opener (Phase 8.5).
pub(crate) fn save_selected_attachment(state: &mut AppState, open_after: bool) -> Vec<Effect> {
    if state.session.focus != Focus::Reader {
        tracing::debug!("save attachment ignored outside the reader");
        return Vec::new();
    }
    let Some(message) = state.open_message.as_loaded() else {
        return Vec::new();
    };
    let Some((_, attachment)) = selected_attachment(state) else {
        return Vec::new();
    };
    let request = crate::domain::AttachmentRequest {
        locator: MessageLocator {
            mailbox: message.mailbox_id.clone(),
            id: message.id.clone(),
            message_id: message.headers.message_id.clone(),
        },
        part_id: attachment.part_id,
        filename: attachment.name.clone(),
        dir: None,
    };
    state.set_status("Saving attachment…");
    vec![
        state
            .session
            .operations
            .start(OperationKind::SaveAttachment {
                request,
                open_after,
            }),
    ]
}

/// Open the selected reader attachment with the platform handler (plan
/// §15, Phase 8.5): a file already saved this session opens from where it
/// landed (no duplicate downloads); otherwise the attachment is saved
/// first and the opener chains on the confirmed path.
pub(crate) fn open_selected_attachment(state: &mut AppState) -> Vec<Effect> {
    if state.session.focus != Focus::Reader {
        tracing::debug!("open attachment ignored outside the reader");
        return Vec::new();
    }
    let Some(message) = state.open_message.as_loaded() else {
        return Vec::new();
    };
    let Some((_, attachment)) = selected_attachment(state) else {
        return Vec::new();
    };
    let key = (message.id.clone(), attachment.part_id);
    if let Some(path) = state.caches.saved_attachments.get(&key).cloned() {
        tracing::debug!(path = %path.display(), "opening previously saved attachment");
        return vec![
            state
                .session
                .operations
                .start(OperationKind::OpenPath { path }),
        ];
    }
    save_selected_attachment(state, true)
}

/// Enter on the reader's focused item (tickets 61qx, hc9n): a focused
/// link opens in the platform browser; otherwise the selected attachment
/// chip opens — the first chip by default, the v1 behavior.
pub(crate) fn activate_reader_item(state: &mut AppState) -> Vec<Effect> {
    if let Some(ReaderFocus::Link(index)) = state.reader_focus {
        let width = crate::view::layout::reader_width(state.session.size);
        return open_reader_link(state, width, index);
    }
    open_selected_attachment(state)
}

/// Open one link run with the platform opener (ticket hc9n). Email HTML
/// is untrusted input: links outside the opener's web-scheme policy are
/// refused with a status note, never handed to the OS dispatcher.
pub(crate) fn open_reader_link(state: &mut AppState, width: usize, index: usize) -> Vec<Effect> {
    let Some(url) = crate::app::reader::link_target(state, width, index) else {
        return Vec::new();
    };
    if !crate::domain::url::is_openable_url(&url) {
        state.set_status(format!("Cannot open link: {}", sanitize(&url)));
        return Vec::new();
    }
    tracing::debug!(url = %url, "opening link in the platform browser");
    state.set_status("Opening link…");
    vec![
        state
            .session
            .operations
            .start(OperationKind::OpenUrl { url }),
    ]
}

/// Apply a finished attachment save: record the path for `Open` reuse and
/// tell the user where the file actually landed (the backend may have
/// collision-renamed it — that path, never the requested one, is shown).
pub(crate) fn attachment_saved(state: &mut AppState, path: &std::path::Path) -> Vec<Effect> {
    let Some(message) = state.open_message.as_loaded() else {
        return Vec::new();
    };
    let Some((_, attachment)) = selected_attachment(state) else {
        return Vec::new();
    };
    state
        .caches
        .saved_attachments
        .insert((message.id.clone(), attachment.part_id), path.to_path_buf());
    state.set_status(format!("Saved to {}", path.display()));
    Vec::new()
}
