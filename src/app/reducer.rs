//! Deterministic, I/O-free reducer (plan §5/§9).
//!
//! `reduce` is the only writer of `AppState`. State transitions that need
//! backend data start an operation in the registry and return an
//! [`Effect`] for the runtime to execute; results come back as
//! `Action::BackendCompleted` and apply only while their operation is still
//! registered, so stale, cancelled, or superseded results never win
//! (plan §11). Reducers never perform I/O themselves.

use crate::app::action::{Action, ClickTarget, DialogEdit, ReaderAction, SearchEdit};
use crate::app::composer::{ComposerField, ComposerState};
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::{
    DraftRemovalReason, OperationFailure, OperationKind, OperationOrigin, OperationOutcome,
    OperationResult,
};
use crate::app::overlay::{
    AttachmentPathDialog, ConfirmButton, DiscardDialog, ErrorDialog, ModalButton, Overlay,
};
use crate::app::route::{MailboxRoute, MessageRoute, Route, SearchRoute};
use crate::app::sanitize::sanitize;
use crate::app::state::{AppState, ListStash, Loadable};
use crate::domain::{
    DraftSaveState, Mailbox, MailboxId, MailboxRole, Message, MessageLocator, Page, PageRequest,
    SearchRequest,
};

/// Apply `action` to `state`, returning backend work to spawn. Never
/// performs I/O, never panics on odd input.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    // A mouse click on a modal button must reach the modal path before the
    // interception swallows everything else (plan §9: a modal intercepts
    // all input; Phase 10.2 adds its buttons as clickable).
    if let Action::Click(target) = action
        && state.overlay.is_some()
    {
        return match target {
            ClickTarget::ErrorButton(button) => click_error_button(state, *button),
            ClickTarget::ConfirmButton(button) => click_confirm_button(state, *button),
            // Clicks "through" the modal do nothing, like any other input.
            _ => Vec::new(),
        };
    }
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
        Action::BackOrCancel => back_or_cancel(state),
        Action::FocusNext => {
            match state.focus {
                Focus::Composer => {
                    if let Some(composer) = state.composer.as_mut() {
                        composer.focus_next();
                    }
                }
                // In the reader Tab walks the attachment chips (plan §15):
                // the selection the save/open keys act on.
                Focus::Reader => cycle_reader_attachment(state, 1),
                _ => state.focus = state.focus.next(),
            }
            Vec::new()
        }
        Action::FocusPrevious => {
            match state.focus {
                Focus::Composer => {
                    if let Some(composer) = state.composer.as_mut() {
                        composer.focus_previous();
                    }
                }
                Focus::Reader => cycle_reader_attachment(state, -1),
                _ => state.focus = state.focus.previous(),
            }
            Vec::new()
        }
        Action::OpenSearch => {
            // Search lives on the mailbox screen; while composing, the '/'
            // is composed text (plan §10: shortcuts never fire in fields).
            if state.focus != Focus::Composer {
                state.focus = Focus::SearchField;
            }
            Vec::new()
        }
        Action::SearchEdit(edit) => {
            search_edit(state, edit);
            Vec::new()
        }
        Action::SubmitSearch => submit_search(state),
        Action::Archive => archive_message(state),
        Action::Trash => trash_message(state),
        Action::SaveAttachment => save_selected_attachment(state, false),
        Action::OpenAttachment => open_selected_attachment(state),
        Action::ToggleStar => toggle_star(state),
        Action::MarkUnread => mark_unread(state),
        Action::Compose => open_composer(state),
        Action::LoadDrafts => load_drafts(state),
        Action::ComposerEdit(edit) => {
            // Editing targets the focused composer control; without a
            // composer open (or without its focus) the edit is inert.
            // Content edits sync the draft and re-arm autosave (plan §14);
            // caret moves leave the revision untouched. While a send of
            // this draft is in flight (Phase 7.6) all editing is frozen:
            // the bytes on the wire must stay what the user saw.
            if state.focus == Focus::Composer
                && let Some(composer) = state.composer.as_mut()
            {
                if composer.sending {
                    tracing::debug!("composer edits frozen while sending");
                } else {
                    composer.apply(edit);
                    if edit.is_content_edit() {
                        composer.sync_draft(state.clock);
                    }
                }
            }
            Vec::new()
        }
        Action::Reply => open_reply(state),
        Action::ReplyAll => open_reply_all(state),
        Action::Forward => open_forward(state),
        Action::Send => send_from_composer(state),
        Action::LeaveComposer => {
            // Routed through `back_or_cancel` for the keyboard (plan §10);
            // dispatched directly this is the same save/leave transition.
            leave_composer(state)
        }
        Action::DiscardDraft => open_discard_confirm(state),
        Action::RetryError | Action::DismissError | Action::DialogEdit(_) => {
            // Only meaningful with their modal open (handled above).
            Vec::new()
        }
        Action::Click(target) => click(state, *target),
        Action::BackendCompleted(result) => backend_completed(state, result),
        Action::Refresh => refresh(state),
        Action::ToggleMouseCapture => {
            // Data only: the runtime applies the capture mode to the
            // terminal (the reducer stays I/O-free). The status line names
            // the mode so the state change is never silent.
            state.mouse_capture = !state.mouse_capture;
            state.set_status(if state.mouse_capture {
                "Mouse capture on — hold Shift to select text · m toggles"
            } else {
                "Mouse capture off — text selection available · m toggles"
            });
            Vec::new()
        }
        Action::Tick { now } => {
            state.ticks += 1;
            let now = **now;
            state.clock = Some(now);
            let mut effects = autosave_tick(state, now);
            effects.extend(auto_refresh_tick(state, now));
            effects
        }
        Action::Resize { width, height } => {
            state.size = (*width, *height);
            // A smaller window may have pushed the selection off screen.
            keep_selection_visible(state);
            clamp_reader_scroll(state);
            Vec::new()
        }
        Action::Quit => {
            state.quit_requested = true;
            Vec::new()
        }
    }
}

// ── Modal overlays (plan §9/§12) ─────────────────────────────────────────

/// Handle `action` while any modal is open. Returns `None` when no modal
/// is open (the caller falls through to normal handling). The attachment
/// dialog likewise falls through for `BackendCompleted`: the pending
/// validation result must land while the dialog is up.
fn modal_reduce(state: &mut AppState, action: &Action) -> Option<Vec<Effect>> {
    match state.overlay {
        Some(Overlay::Error(_)) => Some(error_modal_reduce(state, action)),
        Some(Overlay::ConfirmDiscard(_)) => Some(discard_modal_reduce(state, action)),
        Some(Overlay::AttachmentPath(_)) => match action {
            Action::BackendCompleted(_) => None,
            _ => Some(attachment_dialog_reduce(state, action)),
        },
        None => None,
    }
}

/// Error modal handling (plan §12).
fn error_modal_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
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
        // Only the error modal scrolls; this arm is unreachable in
        // practice (discard_modal_reduce handles its own overlay).
        _ => (0, 1),
    };
    let Some(Overlay::Error(dialog)) = state.overlay.as_mut() else {
        return Vec::new();
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
                        state.reader_attachment = None;
                    }
                    OperationKind::SaveDraft { draft } => {
                        // A draft-save retry replays the *intent* ("save
                        // this draft"), not the failed revision: retry
                        // materializes the newest revision so retrying
                        // after further edits never pushes stale content.
                        if let Some(composer) = state.composer.as_mut()
                            && composer.draft.local_id.as_ref() == Some(&draft.local_id)
                            && let Some(now) = state.clock
                        {
                            let fresh = composer.draft.start_save(now);
                            return vec![state.operations.start(OperationKind::SaveDraft {
                                draft: Box::new(fresh),
                            })];
                        }
                        // No live draft (or no clock yet): replay the
                        // stored snapshot unchanged — the safe direction
                        // is to preserve, never discard.
                    }
                    OperationKind::Send { .. } => {
                        // A send retry re-delivers the frozen bytes
                        // verbatim (plan §12: the intent is replayed
                        // unchanged, under a new id). The composer freezes
                        // again while the replay runs.
                        if let Some(composer) = state.composer.as_mut() {
                            composer.sending = true;
                        }
                    }
                    _ => {}
                }
                return vec![state.operations.start(spec.kind)];
            }
            if !wants_retry {
                // Enter on Dismiss: close without new work.
                state.overlay = None;
                state.focus = focus;
            }
            // `RetryError` on a non-retryable failure keeps the modal open.
            return Vec::new();
        }
        // Everything else is swallowed while the modal is open.
        _ => {}
    }
    Vec::new()
}

/// Confirm-discard dialog handling (plan §14). Every input is swallowed
/// except button switching, keep (Esc/Keep button), and confirm.
fn discard_modal_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Some(Overlay::ConfirmDiscard(dialog)) = state.overlay.as_mut() else {
        return Vec::new();
    };
    match action {
        Action::FocusNext => dialog.button = dialog.button.next(),
        Action::FocusPrevious => dialog.button = dialog.button.previous(),
        // Esc keeps the draft: closing the dialog is not a discard.
        Action::BackOrCancel => {
            let focus = dialog.previous_focus;
            state.overlay = None;
            state.focus = focus;
        }
        Action::Activate => {
            let confirm = dialog.button == ConfirmButton::Discard;
            let focus = dialog.previous_focus;
            let dialog = state.overlay.take().expect("dialog open");
            let Overlay::ConfirmDiscard(dialog) = dialog else {
                unreachable!("checked above")
            };
            state.focus = focus;
            if confirm {
                return confirm_discard(state, dialog.draft);
            }
        }
        // Error-modal-only actions do nothing here.
        _ => {}
    }
    Vec::new()
}

/// Confirmed discard (plan §14): remove the local draft immediately, then
/// delete its journal entry and remote copies via one best-effort backend
/// operation. Any in-flight save of the same draft is cancelled first so
/// it cannot resurrect the draft after the sweep ran.
fn confirm_discard(state: &mut AppState, draft: crate::domain::DraftSnapshot) -> Vec<Effect> {
    state.composer = None;
    if let Some(Route::Composer) = state.active_route() {
        state.routes.pop();
        state.focus = Focus::MessageList;
    }
    state.operations.cancel_draft_saves(&draft.local_id);
    state.set_status("Draft discarded");
    vec![state.operations.start(OperationKind::DeleteDraft {
        draft: Box::new(draft),
        reason: DraftRemovalReason::Discard,
    })]
}

/// Attachment path-entry dialog handling (plan §15, Phase 8). The entry is
/// plain data: edits move the caret, Enter submits the raw path for
/// backend validation, Esc cancels. Rejections keep the dialog open with
/// the detail inline — the retry surface for missing/unreadable files.
fn attachment_dialog_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Some(Overlay::AttachmentPath(dialog)) = state.overlay.as_mut() else {
        return Vec::new();
    };
    match action {
        Action::DialogEdit(edit) => {
            apply_dialog_edit(dialog, edit);
            Vec::new()
        }
        // Esc closes without attaching (plan §10: Esc cancels overlays).
        Action::BackOrCancel => {
            let focus = dialog.previous_focus;
            state.overlay = None;
            state.focus = focus;
            Vec::new()
        }
        Action::Activate => {
            let Some(raw) = nonempty_entry(dialog) else {
                return Vec::new();
            };
            // Validation is backend work (fs access): the entry stays open
            // and untouched until the result arrives.
            dialog.error = None;
            vec![
                state
                    .operations
                    .start(OperationKind::ReadAttachment { path: raw }),
            ]
        }
        // Everything else is swallowed while the dialog is open.
        _ => Vec::new(),
    }
}

/// The trimmed, non-empty dialog entry, or `None` after setting the
/// inline error (an empty submission is an input problem, not an
/// operation).
fn nonempty_entry(dialog: &mut AttachmentPathDialog) -> Option<std::path::PathBuf> {
    let trimmed = dialog.input.trim();
    if trimmed.is_empty() {
        dialog.error = Some(String::from("Enter a file path"));
        return None;
    }
    Some(std::path::PathBuf::from(trimmed))
}

/// One character-level edit of the dialog entry (caret editing like the
/// composer's single-line fields).
fn apply_dialog_edit(dialog: &mut AttachmentPathDialog, edit: &DialogEdit) {
    match edit {
        DialogEdit::Char(c) => {
            let cursor = dialog.cursor;
            let offset = dialog
                .input
                .char_indices()
                .nth(cursor)
                .map(|(offset, _)| offset)
                .unwrap_or(dialog.input.len());
            dialog.input.insert(offset, *c);
            dialog.cursor = cursor + 1;
        }
        DialogEdit::Backspace => {
            if dialog.cursor > 0 {
                let offset = dialog
                    .input
                    .char_indices()
                    .nth(dialog.cursor - 1)
                    .map(|(offset, _)| offset)
                    .unwrap_or(dialog.input.len());
                dialog.input.remove(offset);
                dialog.cursor -= 1;
            }
        }
        DialogEdit::Delete => {
            let cursor = dialog.cursor;
            if cursor < dialog.input.chars().count() {
                let offset = dialog
                    .input
                    .char_indices()
                    .nth(cursor)
                    .map(|(offset, _)| offset)
                    .unwrap_or(dialog.input.len());
                dialog.input.remove(offset);
            }
        }
        DialogEdit::CursorLeft => dialog.cursor = dialog.cursor.saturating_sub(1),
        DialogEdit::CursorRight => {
            dialog.cursor = dialog
                .cursor
                .saturating_add(1)
                .min(dialog.input.chars().count());
        }
    }
    // A fresh edit supersedes the stale complaint about the old entry.
    dialog.error = None;
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

// ── Mouse clicks (plan §10, Phase 10.1/10.2) ─────────────────────────────

/// Apply a mouse click recorded during render. Every arm below mirrors the
/// keyboard path — selecting, opening, focusing, or pressing — so the
/// mouse never unlocks behavior the keyboard cannot reach (plan §10). A
/// click on an already-selected row activates it, matching the
/// select-then-Enter rhythm the keyboard uses. Modal-button clicks are
/// routed before the modal interception (see `reduce`); this function only
/// ever runs with no overlay open.
fn click(state: &mut AppState, target: ClickTarget) -> Vec<Effect> {
    match target {
        ClickTarget::ComposeButton => reduce(state, &Action::Compose),
        ClickTarget::Mailbox(index) => click_mailbox(state, index),
        ClickTarget::SearchField => reduce(state, &Action::OpenSearch),
        ClickTarget::MessageRow(index) => click_message_row(state, index),
        ClickTarget::ReaderAttachment(index) => click_reader_attachment(state, index),
        ClickTarget::ReaderAction(action) => click_reader_action(state, action),
        ClickTarget::ComposerField(field) => click_composer_field(state, field),
        // Modal buttons outside a modal cannot happen (their regions are
        // only recorded while the modal renders); the arm keeps the match
        // total.
        ClickTarget::ErrorButton(_) | ClickTarget::ConfirmButton(_) => Vec::new(),
    }
}

/// Click a modal button: focus it (the same state Tab produces), then run
/// the same path Enter would (plan §12).
fn click_error_button(state: &mut AppState, button: ModalButton) -> Vec<Effect> {
    if let Some(Overlay::Error(dialog)) = state.overlay.as_mut() {
        // A dimmed Retry button (failure without a retry intent) does
        // nothing, like a Retry that only `Tab` could reach.
        if button == ModalButton::Retry && dialog.retry.is_none() {
            return Vec::new();
        }
        dialog.button = button;
    }
    match button {
        ModalButton::Retry => reduce(state, &Action::RetryError),
        ModalButton::Dismiss => reduce(state, &Action::DismissError),
    }
}

/// Click a confirm-discard button (plan §14): focus, then the same
/// confirm/keep path Enter takes.
fn click_confirm_button(state: &mut AppState, button: ConfirmButton) -> Vec<Effect> {
    if let Some(Overlay::ConfirmDiscard(dialog)) = state.overlay.as_mut() {
        dialog.button = button;
    }
    reduce(state, &Action::Activate)
}

/// Click a sidebar mailbox row: focus follows the click, a new row is
/// selected (the arrows' job), an already-selected row activates (Enter's
/// job, plan §10).
fn click_mailbox(state: &mut AppState, index: usize) -> Vec<Effect> {
    let count = state.mailboxes.as_loaded().map(Vec::len).unwrap_or(0);
    if index >= count {
        return Vec::new();
    }
    state.focus = Focus::Sidebar;
    if index == state.mailbox_selection {
        return match state.selected_mailbox().cloned() {
            Some(mailbox) => switch_mailbox(state, &mailbox.id),
            None => Vec::new(),
        };
    }
    state.mailbox_selection = index;
    Vec::new()
}

/// Click a message row: focus the list, select the row, and open it when
/// it was already the selection (arrows + Enter equivalent).
fn click_message_row(state: &mut AppState, index: usize) -> Vec<Effect> {
    if index >= state.messages.items.len() {
        return Vec::new();
    }
    state.focus = Focus::MessageList;
    if index == state.selection {
        return match state.selected_message().cloned() {
            Some(summary) => open_message(state, summary),
            None => Vec::new(),
        };
    }
    state.selection = index;
    keep_selection_visible(state);
    Vec::new()
}

/// Click a control on the reader action row (plan §10): the advertised
/// key's action, on the open message. Focus follows the click so the
/// action path (which keys off `Focus::Reader`) sees the reader context.
fn click_reader_action(state: &mut AppState, action: ReaderAction) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Message(_))) {
        return Vec::new();
    }
    state.focus = Focus::Reader;
    let action = match action {
        ReaderAction::Reply => Action::Reply,
        ReaderAction::Forward => Action::Forward,
        ReaderAction::Archive => Action::Archive,
        ReaderAction::Star => Action::ToggleStar,
        ReaderAction::Unread => Action::MarkUnread,
        ReaderAction::Trash => Action::Trash,
        ReaderAction::SaveAttachment => Action::SaveAttachment,
        ReaderAction::OpenAttachment => Action::OpenAttachment,
    };
    reduce(state, &action)
}

/// Click an attachment chip in the reader (plan §15): select it; clicking
/// the already-selected chip opens it (`o`'s job — reuse a saved path or
/// save first, then open).
fn click_reader_attachment(state: &mut AppState, index: usize) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Message(_))) {
        return Vec::new();
    }
    let count = state
        .open_message
        .as_loaded()
        .map(|message| message.attachments.len())
        .unwrap_or(0);
    if index >= count {
        return Vec::new();
    }
    state.focus = Focus::Reader;
    if state.reader_attachment.unwrap_or(0) == index {
        return reduce(state, &Action::OpenAttachment);
    }
    state.reader_attachment = Some(index);
    Vec::new()
}

/// Click a composer control (plan §10): focus it; buttons activate, like
/// Tab-then-Enter would. Text fields place the caret the way `Tab` does
/// (at the end of the field; the body keeps its caret).
fn click_composer_field(state: &mut AppState, field: ComposerField) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Composer)) {
        return Vec::new();
    }
    let focused = state
        .composer
        .as_mut()
        .is_some_and(|composer| composer.focus_field(field));
    if !focused {
        return Vec::new();
    }
    state.focus = Focus::Composer;
    if field.accepts_text() {
        Vec::new()
    } else {
        activate_composer(state)
    }
}

// ── Reply / forward seeding (plan §14, Phase 7.3) ────────────────────────

/// Seed a reply draft from the open message (reader). Reply acts on full
/// message data — headers, threading ids, and the quotable body — so it
/// requires a loaded reader; a draft already in the composer is never
/// clobbered (plan §14: one composer at a time).
fn open_reply(state: &mut AppState) -> Vec<Effect> {
    let Some(message) = state.open_message.as_loaded() else {
        tracing::debug!("reply ignored: no loaded message in the reader");
        return Vec::new();
    };
    if state.composer.is_some() {
        state.set_status("A draft is already open — send or discard it first");
        return Vec::new();
    }
    let seed = crate::domain::reply::seed_reply(message, crate::domain::ReplyKind::Reply, None);
    open_seeded_composer(state, seed, "Reply draft ready")
}

/// Seed a reply-all draft (Phase 7.5): recipients merged, deduplicated,
/// and the configured account address excluded.
fn open_reply_all(state: &mut AppState) -> Vec<Effect> {
    let Some(message) = state.open_message.as_loaded() else {
        tracing::debug!("reply-all ignored: no loaded message in the reader");
        return Vec::new();
    };
    if state.composer.is_some() {
        state.set_status("A draft is already open — send or discard it first");
        return Vec::new();
    }
    let own = state.account_email.clone();
    let seed = crate::domain::reply::seed_reply(
        message,
        crate::domain::ReplyKind::ReplyAll,
        own.as_deref(),
    );
    open_seeded_composer(state, seed, "Reply-all draft ready")
}

/// Seed a forward draft from the open message (reader).
fn open_forward(state: &mut AppState) -> Vec<Effect> {
    let Some(message) = state.open_message.as_loaded() else {
        tracing::debug!("forward ignored: no loaded message in the reader");
        return Vec::new();
    };
    if state.composer.is_some() {
        state.set_status("A draft is already open — send or discard it first");
        return Vec::new();
    }
    let seed = crate::domain::reply::seed_forward(message);
    open_seeded_composer(state, seed, "Forward draft ready")
}

/// Install a seeded draft in the composer, pushing the composer route on
/// top of the current one (reply from the reader returns to the reader on
/// Esc). The seeded draft starts clean: autosave engages on the first
/// edit.
fn open_seeded_composer(
    state: &mut AppState,
    seed: crate::domain::reply::Seed,
    status: &str,
) -> Vec<Effect> {
    let draft = crate::domain::Draft {
        to: seed.to,
        cc: seed.cc,
        bcc: String::new(),
        subject: seed.subject,
        body: seed.body,
        in_reply_to: seed.in_reply_to,
        references: seed.references,
        ..crate::domain::Draft::default()
    };
    state.composer = Some(ComposerState::from_draft(draft));
    if !matches!(state.active_route(), Some(Route::Composer)) {
        state.routes.push(Route::Composer);
    }
    state.focus = Focus::Composer;
    state.set_status(status);
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

// ── Attachment save (plan §15, Phase 8.4) ────────────────────────────────

/// Save the selected reader attachment (plan §15). Reader-only: the open
/// message supplies the locator and the part id, the chip cursor supplies
/// the attachment. `d` saves into the downloads directory; the backend
/// owns collision handling and returns the path actually written. `o`
/// (open) reuses a path saved this session or saves first, then chains
/// the platform opener (Phase 8.5).
fn save_selected_attachment(state: &mut AppState, open_after: bool) -> Vec<Effect> {
    if state.focus != Focus::Reader {
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
    vec![state.operations.start(OperationKind::SaveAttachment {
        request,
        open_after,
    })]
}

/// Open the selected reader attachment with the platform handler (plan
/// §15, Phase 8.5): a file already saved this session opens from where it
/// landed (no duplicate downloads); otherwise the attachment is saved
/// first and the opener chains on the confirmed path.
fn open_selected_attachment(state: &mut AppState) -> Vec<Effect> {
    if state.focus != Focus::Reader {
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
    if let Some(path) = state.saved_attachments.get(&key).cloned() {
        tracing::debug!(path = %path.display(), "opening previously saved attachment");
        return vec![state.operations.start(OperationKind::OpenPath { path })];
    }
    save_selected_attachment(state, true)
}

/// Apply a finished attachment save: record the path for `Open` reuse and
/// tell the user where the file actually landed (the backend may have
/// collision-renamed it — that path, never the requested one, is shown).
fn attachment_saved(state: &mut AppState, path: &std::path::Path) -> Vec<Effect> {
    let Some(message) = state.open_message.as_loaded() else {
        return Vec::new();
    };
    let Some((_, attachment)) = selected_attachment(state) else {
        return Vec::new();
    };
    state
        .saved_attachments
        .insert((message.id.clone(), attachment.part_id), path.to_path_buf());
    state.set_status(format!("Saved to {}", path.display()));
    Vec::new()
}

// ── Send (plan §14, Phase 7.6/7.7) ───────────────────────────────────────

/// Ctrl+Enter / Send button (plan §10). Sends only from the composer: the
/// route gate makes the action inert anywhere else. Refusals (invalid or
/// missing recipients, a send already in flight) surface on the status
/// line — the problem is visible input, not an operational failure — and
/// start nothing, so the draft is untouched.
fn send_from_composer(state: &mut AppState) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Composer)) {
        // Ctrl+Enter sends only from the composer (plan §19 Phase 7).
        tracing::debug!("send ignored outside the composer");
        return Vec::new();
    }
    let Some(composer) = state.composer.as_ref() else {
        return Vec::new();
    };
    if composer.sending {
        tracing::debug!("send ignored: one is already in flight");
        return Vec::new();
    }
    if state.operations.is_sending() {
        state.set_status("A send is already in progress");
        return Vec::new();
    }
    let draft = &composer.draft;
    // Attached files ride through by path (plan §15, Phase 8.2): the
    // backend reads the bytes when it serializes the MIME.
    let attachments = draft
        .attachments
        .iter()
        .map(|att| crate::domain::OutboundAttachment {
            name: att.name.clone(),
            path: att.path.clone(),
        })
        .collect();
    let message = crate::domain::OutboundMessage::with_attachments(
        &draft.to,
        &draft.cc,
        &draft.bcc,
        crate::domain::OutgoingContent {
            subject: draft.subject.clone(),
            body: draft.body.clone(),
            in_reply_to: draft.in_reply_to.clone(),
            references: draft.references.clone(),
        },
        draft.message_id.clone(),
        attachments,
    );
    let message = match message {
        Ok(message) => message,
        Err(blocker) => {
            state.set_status(blocker.to_string());
            return Vec::new();
        }
    };
    if let Some(composer) = state.composer.as_mut() {
        composer.sending = true;
    }
    state.set_status("Sending…");
    vec![state.operations.start(OperationKind::Send {
        message: Box::new(message),
    })]
}

/// Apply a classified send outcome (plan §12). Only [`SendOutcome::Sent`]
/// is definitive; every other outcome opens the modal and keeps the draft
/// (plan §19 Phase 7: failed send keeps the draft intact). `message` is
/// the frozen payload, replayed verbatim by retries.
fn send_completed(
    state: &mut AppState,
    outcome: &crate::domain::SendOutcome,
    message: &crate::domain::OutboundMessage,
) -> Vec<Effect> {
    use crate::domain::SendOutcome;
    match outcome {
        SendOutcome::Sent => confirm_send(state),
        other => {
            // The draft stays exactly as it was, editable again; the modal
            // carries the typed retry intent.
            if let Some(composer) = state.composer.as_mut() {
                composer.sending = false;
            }
            let failure = OperationFailure {
                code: other.code(),
                detail: {
                    let detail = sanitize(other.detail());
                    if detail.is_empty() {
                        String::from("the send outcome could not be determined")
                    } else {
                        detail
                    }
                },
                retry: Some(
                    OperationKind::Send {
                        message: Box::new(message.clone()),
                    }
                    .retry_spec(),
                ),
                ambiguous: other.is_ambiguous(),
            };
            // The specific send status must survive the modal opening
            // (the modal sets the generic "Operation failed" first).
            let effects = open_error_modal(state, &failure);
            state.set_status(if other.is_ambiguous() {
                "Send outcome unclear"
            } else {
                "Send failed"
            });
            effects
        }
    }
}

/// A confirmed send (plan §19 Phase 7, Phase 7.6): leave the composer,
/// return to the prior route, and resolve the draft — journal entry and
/// remote copies removed through the same backend sweep a discard uses
/// (ADR 0002 §D.4). That cleanup is best-effort (`DraftRemovalReason::Sent`):
/// delivery is already confirmed, so a leftover copy must never claim a
/// failure afterwards.
fn confirm_send(state: &mut AppState) -> Vec<Effect> {
    state.set_status("Message sent");
    // The composer may have been left mid-send (Esc saves/leaves); the
    // draft data — with its stable ids — is what gets resolved here.
    let snapshot = state
        .composer
        .take()
        .map(|composer| composer.draft.snapshot());
    if matches!(state.active_route(), Some(Route::Composer)) {
        state.routes.pop();
        state.focus = Focus::MessageList;
    }
    match snapshot {
        Some(snapshot) => vec![state.operations.start(OperationKind::DeleteDraft {
            draft: Box::new(snapshot),
            reason: DraftRemovalReason::Sent,
        })],
        None => {
            tracing::debug!("send confirmed without a draft to resolve");
            Vec::new()
        }
    }
}

// ── Backend results (plan §11) ───────────────────────────────────────────

/// Failure handling for list-shaped results (mailbox pages and searches,
/// Phase 9): a *foreground* failure opens the Retry/Dismiss modal; a
/// *background* (timer) refresh failure never interrupts the user — the
/// first failure lands in the status line, and repeated identical failures
/// are suppressed until a success or a manual refresh clears the record
/// (Phase 9.6).
fn list_failure(state: &mut AppState, failure: &OperationFailure, origin: OperationOrigin) {
    if origin == OperationOrigin::Background {
        if state.last_background_error.as_deref() != Some(failure.detail.as_str()) {
            tracing::info!(detail = %failure.detail, "background refresh failed");
            state.set_status("Refresh failed — the timer will retry");
            state.last_background_error = Some(failure.detail.clone());
        } else {
            tracing::debug!("identical background refresh failure; status unchanged");
        }
        return;
    }
    // The last coherent page stays visible; the modal offers Retry/Dismiss
    // (plan §12).
    open_error_modal(state, failure);
}

/// Apply a backend result. Results for unknown, cancelled, or superseded
/// operation ids never mutate state: `finish` removes the operation, and a
/// superseded operation was already cancelled and removed when its
/// replacement started.
fn backend_completed(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    let Some(op) = state.operations.get(result.id) else {
        tracing::debug!(id = %result.id, "dropping result for unknown or cancelled operation");
        return Vec::new();
    };
    let kind = op.kind.clone();
    let origin = op.origin;
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
                    state.last_background_error = None;
                    apply_page(state, page.clone());
                    Vec::new()
                }
                Ok(OperationOutcome::Mailboxes(_)) => {
                    tracing::warn!(id = %result.id, "mailbox payload for a page operation");
                    Vec::new()
                }
                Err(failure) => {
                    // Foreground failures open the Retry/Dismiss modal;
                    // background (timer) refresh failures never interrupt
                    // the user (Phase 9.6).
                    list_failure(state, failure, origin);
                    Vec::new()
                }
                _ => {
                    tracing::warn!(id = %result.id, "unexpected payload for a page operation");
                    Vec::new()
                }
            }
        }
        OperationKind::Search(request) => {
            // Currency check: the results must belong to the open search —
            // same query and mailbox (a re-submit supersedes the older
            // operation, so only races with navigation land here).
            let current = matches!(
                state.active_route(),
                Some(Route::Search(route))
                    if route.mailbox_id == request.mailbox_id && route.query == request.query
            );
            state.operations.finish(result.id);
            if !current {
                tracing::debug!(
                    id = %result.id,
                    mailbox = %request.mailbox_id.0,
                    "dropping search result for a closed or changed search"
                );
                return Vec::new();
            }
            match &result.outcome {
                Ok(OperationOutcome::Page(page)) => {
                    state.last_background_error = None;
                    apply_page(state, page.clone());
                    Vec::new()
                }
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a search operation");
                    Vec::new()
                }
                Err(failure) => {
                    list_failure(state, failure, origin);
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
        OperationKind::SaveDraft { draft } => {
            state.operations.finish(result.id);
            save_draft_completed(state, draft, result)
        }
        OperationKind::LoadDrafts => {
            state.operations.finish(result.id);
            match &result.outcome {
                Ok(OperationOutcome::Drafts(drafts)) => drafts_restored(state, drafts),
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a draft restore");
                    Vec::new()
                }
                Err(failure) => {
                    // The journal is the crash-safety net; a failure to read
                    // it must be visible (plan §12) even though mail
                    // browsing can continue without drafts.
                    open_error_modal(state, failure)
                }
            }
        }
        OperationKind::DeleteDraft { reason, .. } => {
            state.operations.finish(result.id);
            // A discard applied its local half optimistically when the
            // confirm dialog was accepted (Phase 6.6); a send removed the
            // composer on confirmation (Phase 7.6). Failures follow the
            // removal reason: discards open the modal, post-send cleanup
            // is best-effort (ADR 0002) and never claims a failed send.
            match &result.outcome {
                Ok(OperationOutcome::Done) => Vec::new(),
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a draft removal");
                    Vec::new()
                }
                Err(failure) => match reason {
                    DraftRemovalReason::Discard => open_error_modal(state, failure),
                    DraftRemovalReason::Sent => {
                        tracing::warn!(
                            detail = %failure.detail,
                            "the sent message's draft copy could not be removed"
                        );
                        Vec::new()
                    }
                },
            }
        }
        OperationKind::Send { message } => {
            state.operations.finish(result.id);
            match &result.outcome {
                Ok(OperationOutcome::SendOutcome(outcome)) => {
                    send_completed(state, outcome, message)
                }
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for a send");
                    Vec::new()
                }
                Err(failure) => {
                    // Structural refusal (missing identity, spawn I/O):
                    // nothing was transmitted, the draft stays intact and
                    // editable (plan §19 Phase 7: failed send keeps it).
                    if let Some(composer) = state.composer.as_mut() {
                        composer.sending = false;
                    }
                    state.set_status("Send failed");
                    open_error_modal(state, failure)
                }
            }
        }
        OperationKind::ReadAttachment { path } => {
            state.operations.finish(result.id);
            attachment_validated(state, path, result)
        }
        OperationKind::SaveAttachment {
            request: _,
            open_after,
        } => {
            state.operations.finish(result.id);
            match &result.outcome {
                Ok(OperationOutcome::SavedPath(path)) => {
                    attachment_saved(state, path);
                    if *open_after {
                        // Save-then-open (Phase 8.5): the opener chains on
                        // the confirmed path — which may be a
                        // collision-renamed name, so the chain uses exactly
                        // what was written.
                        return vec![
                            state
                                .operations
                                .start(OperationKind::OpenPath { path: path.clone() }),
                        ];
                    }
                    Vec::new()
                }
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for an attachment save");
                    Vec::new()
                }
                Err(failure) => open_error_modal(state, failure),
            }
        }
        OperationKind::OpenPath { .. } => {
            state.operations.finish(result.id);
            match &result.outcome {
                Ok(OperationOutcome::Done) => {
                    state.set_status("Opened");
                    Vec::new()
                }
                Ok(_) => {
                    tracing::warn!(id = %result.id, "unexpected payload for an open");
                    Vec::new()
                }
                Err(failure) => open_error_modal(state, failure),
            }
        }
    }
}

/// Apply the journal restore (ADR 0002 §D.5): rebuild the newest draft
/// into the composer so composing after a crash continues it. Never
/// clobbers a live composer. A gap between the recorded and
/// remote-confirmed revisions leaves the draft dirty, so the autosave
/// self-heals the gap once composing resumes.
fn drafts_restored(state: &mut AppState, drafts: &[crate::domain::RestoredDraft]) -> Vec<Effect> {
    let Some(restored) = drafts.iter().max_by_key(|entry| entry.draft.revision) else {
        return Vec::new();
    };
    if state.composer.is_some() {
        tracing::debug!("draft restore skipped: a composer draft already exists");
        return Vec::new();
    }
    let snapshot = restored.draft.clone();
    // A gap between the recorded and remote-confirmed revisions means the
    // crash interrupted a push: restore the draft dirty so the autosave
    // self-heals it once the window elapses.
    let save = if snapshot.revision > restored.saved_revision {
        crate::domain::DraftSaveState::Debouncing
    } else {
        crate::domain::DraftSaveState::Saved
    };
    let draft = crate::domain::Draft {
        to: snapshot.to,
        cc: snapshot.cc,
        bcc: snapshot.bcc,
        subject: snapshot.subject,
        body: snapshot.body,
        attachments: snapshot.attachments,
        revision: snapshot.revision,
        saved_revision: restored.saved_revision,
        saved_at: None,
        last_edit_at: state.clock,
        save,
        local_id: Some(snapshot.local_id),
        message_id: snapshot.message_id,
        in_reply_to: snapshot.in_reply_to,
        references: snapshot.references,
        remote_id: snapshot.remote_id,
    };
    tracing::info!(
        local_id = %draft.local_id.as_ref().map(|id| id.0.as_str()).unwrap_or("?"),
        revision = draft.revision,
        saved_revision = draft.saved_revision,
        "draft restored from journal"
    );
    state.composer = Some(ComposerState::from_draft(draft));
    Vec::new()
}

/// Apply a confirmed draft save (plan §14). Currency check: the draft must
/// still be the one in the composer (a discarded draft is gone and its
/// result is dropped). Success for the newest revision marks the draft
/// saved; a stale success (edits happened meanwhile) immediately chains
/// another save so revision N+1 is never left unpushed.
fn save_draft_completed(
    state: &mut AppState,
    snapshot: &crate::domain::DraftSnapshot,
    result: &OperationResult,
) -> Vec<Effect> {
    let current = state
        .composer
        .as_ref()
        .is_some_and(|c| c.draft.local_id.as_ref() == Some(&snapshot.local_id));
    if !current {
        tracing::debug!(
            local_id = %snapshot.local_id.0,
            "dropping draft result for a discarded draft"
        );
        return Vec::new();
    }
    let composer = state.composer.as_mut().expect("checked above");
    match &result.outcome {
        Ok(OperationOutcome::DraftSaved { remote_id }) => {
            let chain =
                composer
                    .draft
                    .confirm_saved(snapshot.revision, remote_id.clone(), state.clock);
            if chain {
                // Remain dirty and save again (plan §14): one follow-up
                // save covering the newest revision. It supersedes nothing
                // in flight — the previous save just completed.
                draft_save_effect(state).into_iter().collect()
            } else {
                Vec::new()
            }
        }
        Err(failure) => {
            // Unsaved state + Retry/Dismiss modal; content retained
            // (plan §14 acceptance). A newer revision debounce outranks
            // the stale failure — its scheduled save retries anyway.
            if snapshot.revision >= composer.draft.revision {
                composer.draft.mark_failed();
            }
            open_error_modal(state, failure);
            state.set_status("Draft save failed");
            Vec::new()
        }
        Ok(_) => {
            tracing::warn!(id = %result.id, "unexpected payload for a draft save");
            Vec::new()
        }
    }
}

/// Apply a finished attachment validation (plan §15, Phase 8). Currency:
/// the dialog must still be open, still showing the entry that was
/// submitted — an Esc or an edit in the meantime drops the result. A
/// validated file becomes a chip and a content edit (autosave carries the
/// attachment list into the journal); a rejection keeps the dialog open
/// with the detail inline, retryable in place.
fn attachment_validated(
    state: &mut AppState,
    path: &std::path::Path,
    result: &OperationResult,
) -> Vec<Effect> {
    let Some(Overlay::AttachmentPath(dialog)) = state.overlay.as_ref() else {
        tracing::debug!(id = %result.id, "dropping attachment validation for a closed dialog");
        return Vec::new();
    };
    if dialog.input.trim() != path.to_string_lossy() {
        tracing::debug!(id = %result.id, "dropping attachment validation for an edited entry");
        return Vec::new();
    }
    match &result.outcome {
        Ok(OperationOutcome::Attachment(att)) => {
            let att = att.clone();
            let focus = dialog.previous_focus;
            state.overlay = None;
            state.focus = focus;
            let Some(composer) = state.composer.as_mut() else {
                tracing::debug!("validated attachment ignored: no composer");
                return Vec::new();
            };
            let name = att.name.clone();
            let size = att.size;
            if composer.add_attachment(att) {
                let last = composer.draft.attachments.len() - 1;
                composer.field = ComposerField::Attachment(last);
                // Attaching is a content edit: the revision bumps and the
                // autosave journal carries the new attachment list.
                composer.draft.note_edit(state.clock);
                state.set_status(format!(
                    "Attached {name} ({})",
                    crate::ui::text::human_size(size)
                ));
            } else {
                state.set_status(format!("{name} is already attached"));
            }
            Vec::new()
        }
        Err(failure) => {
            // Detailed and retryable in place: the entry stays editable
            // (plan §15, Phase 8 acceptance).
            let detail = failure.detail.clone();
            if let Some(Overlay::AttachmentPath(dialog)) = state.overlay.as_mut() {
                dialog.error = Some(detail);
            }
            Vec::new()
        }
        Ok(_) => {
            tracing::warn!(id = %result.id, "unexpected payload for an attachment validation");
            Vec::new()
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

/// Move the reader's attachment-chip cursor (Tab/Shift+Tab, plan §15).
/// Wraps within the loaded message's chip count; inert without a loaded
/// message or attachments.
fn cycle_reader_attachment(state: &mut AppState, delta: i64) {
    let len = state
        .open_message
        .as_loaded()
        .map(|message| message.attachments.len())
        .unwrap_or(0);
    if len == 0 {
        return;
    }
    let current = state.reader_attachment.unwrap_or(0).min(len - 1);
    let next = (current as i64 + delta).rem_euclid(len as i64) as usize;
    state.reader_attachment = Some(next);
}

/// The attachment the reader's save/open keys act on, when the open
/// message carries any (plan §15).
fn selected_attachment(state: &AppState) -> Option<(usize, &crate::domain::Attachment)> {
    let message = state.open_message.as_loaded()?;
    let index = state
        .reader_attachment
        .unwrap_or(0)
        .min(message.attachments.len().saturating_sub(1));
    let attachment = message.attachments.get(index)?;
    Some((index, attachment))
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
    // The visible context could be a mailbox page or search results
    // (Phase 9); the re-sync follows whichever is open.
    request_visible_page(state, state.messages.offset)
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
        Focus::Composer | Focus::Dialog | Focus::ErrorModal => {}
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
    let total = crate::ui::screens::reader::content_line_count(
        state,
        crate::ui::layout::reader_width(state.size).max(10),
    ) as i64;
    let max = (total - viewport).max(0);
    let next = (state.reader_scroll as i64 + delta).clamp(0, max);
    state.reader_scroll = next as usize;
}

/// Reflow on terminal resize (plan §13/§19 Phase 5): the document re-wraps
/// at the new reader width inside the renderer, and the scroll anchor is
/// clamped to the re-flowed length so the viewport can never point past the
/// end of the document.
fn clamp_reader_scroll(state: &mut AppState) {
    if !matches!(state.active_route(), Some(Route::Message(_))) {
        return;
    }
    let viewport = crate::ui::layout::reader_rows_visible(state.size).max(1) as i64;
    let total = crate::ui::screens::reader::content_line_count(
        state,
        crate::ui::layout::reader_width(state.size).max(10),
    ) as i64;
    let max = (total - viewport).max(0);
    state.reader_scroll = (state.reader_scroll as i64).clamp(0, max) as usize;
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
    // same next page. The in-flight kind follows the visible context
    // (mailbox page vs search results, Phase 9).
    let base = match state.active_route() {
        Some(Route::Search(route)) => state
            .operations
            .search_in_flight(&route.mailbox_id)
            .map(|pending| pending.offset as i64),
        Some(route) => route
            .mailbox_id()
            .and_then(|id| state.operations.page_in_flight(id))
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
    // Unknown total (maildir, search): a short page is the last one, so
    // there is nothing valid to request.
    if state.messages.total.is_none() && state.messages.items.len() < state.messages.limit {
        return Vec::new();
    }
    // The selection keeps pointing at the current page until the result
    // applies; `apply_page` re-resolves it by identity.
    request_visible_page(state, target as usize)
}

/// Request one page of whatever list the active route shows (Phase 9): the
/// active mailbox's page, or the open search's results. Both share the
/// list state, selection, and scroll machinery (Phase 9.1).
fn request_visible_page(state: &mut AppState, offset: usize) -> Vec<Effect> {
    match state.active_route() {
        Some(Route::Search(route)) => {
            let request = SearchRequest {
                mailbox_id: route.mailbox_id.clone(),
                query: route.query.clone(),
                offset,
                limit: state.messages.limit.max(1),
            };
            vec![state.operations.start(OperationKind::Search(request))]
        }
        Some(route) => match route.mailbox_id().cloned() {
            Some(mailbox_id) => {
                let request = PageRequest {
                    mailbox_id,
                    offset,
                    limit: state.messages.limit.max(1),
                };
                vec![state.operations.start(OperationKind::LoadPage(request))]
            }
            None => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// Background variant of [`request_visible_page`] (Phase 9.4): same
/// request, but failures are handled as background work (Phase 9.6).
fn request_visible_page_background(state: &mut AppState, offset: usize) -> Vec<Effect> {
    match state.active_route() {
        Some(Route::Search(route)) => {
            let request = SearchRequest {
                mailbox_id: route.mailbox_id.clone(),
                query: route.query.clone(),
                offset,
                limit: state.messages.limit.max(1),
            };
            vec![
                state
                    .operations
                    .start_background(OperationKind::Search(request)),
            ]
        }
        Some(route) => match route.mailbox_id().cloned() {
            Some(mailbox_id) => {
                let request = PageRequest {
                    mailbox_id,
                    offset,
                    limit: state.messages.limit.max(1),
                };
                vec![
                    state
                        .operations
                        .start_background(OperationKind::LoadPage(request)),
                ]
            }
            None => Vec::new(),
        },
        None => Vec::new(),
    }
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
        Focus::Composer => activate_composer(state),
        // The dialog intercepts Enter itself; unreachable in practice.
        Focus::Reader | Focus::Dialog | Focus::SearchField | Focus::ErrorModal => {
            if state.focus == Focus::SearchField {
                reduce(state, &Action::SubmitSearch)
            } else {
                Vec::new()
            }
        }
    }
}

/// Enter on a composer control (plan §10): newline in the body, reveal a
/// hidden Cc/Bcc field, open the attachment path dialog (plan §15), remove
/// the focused attachment chip, or activate the focused action. Enter on
/// To/Cc/Bcc/Subject itself does nothing (single-line fields have no
/// activation).
fn activate_composer(state: &mut AppState) -> Vec<Effect> {
    let Some(field) = state.composer.as_ref().map(|c| c.field) else {
        return Vec::new();
    };
    match field {
        ComposerField::Body => {
            if let Some(composer) = state.composer.as_mut() {
                composer.apply(&crate::app::action::ComposerEdit::Newline);
            }
            Vec::new()
        }
        ComposerField::CcToggle => {
            if let Some(composer) = state.composer.as_mut() {
                composer.show_cc = true;
                composer.enter_cc();
            }
            Vec::new()
        }
        ComposerField::BccToggle => {
            if let Some(composer) = state.composer.as_mut() {
                composer.show_bcc = true;
                composer.enter_bcc();
            }
            Vec::new()
        }
        ComposerField::Attach => open_attachment_dialog(state),
        ComposerField::Attachment(index) => remove_attachment(state, index),
        ComposerField::Send => reduce(state, &Action::Send),
        ComposerField::Discard => reduce(state, &Action::DiscardDraft),
        ComposerField::To | ComposerField::Cc | ComposerField::Bcc | ComposerField::Subject => {
            Vec::new()
        }
    }
}

/// Open the attachment path-entry overlay (plan §15: path entry, no file
/// browser in v1). No-op without a composer.
fn open_attachment_dialog(state: &mut AppState) -> Vec<Effect> {
    if state.composer.is_none() {
        return Vec::new();
    }
    state.overlay = Some(Overlay::AttachmentPath(AttachmentPathDialog {
        input: String::new(),
        cursor: 0,
        error: None,
        previous_focus: state.focus,
    }));
    state.focus = Focus::Dialog;
    Vec::new()
}

/// Remove the focused attachment chip (plan §15: "allow removal before
/// send"). Removal is a content edit: the revision bumps and autosave
/// journals the shorter attachment list.
fn remove_attachment(state: &mut AppState, index: usize) -> Vec<Effect> {
    let Some(composer) = state.composer.as_mut() else {
        return Vec::new();
    };
    let removed = composer
        .draft
        .attachments
        .get(index)
        .map(|att| att.name.clone());
    composer.remove_attachment(index);
    if let Some(name) = removed {
        composer.draft.note_edit(state.clock);
        state.set_status(format!("Removed {name}"));
    }
    Vec::new()
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
    state.reader_attachment = None;
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
        state.reader_attachment = None;
        state.focus = Focus::MessageList;
    }
}

/// Open (or reopen) the built-in composer (plan §19 Phase 6). A left-open
/// draft is preserved in `AppState.composer`, so composing again returns to
/// it; only one composer exists at a time.
fn open_composer(state: &mut AppState) -> Vec<Effect> {
    if matches!(state.active_route(), Some(Route::Composer)) {
        return Vec::new();
    }
    if state.composer.is_none() {
        state.composer = Some(ComposerState::new());
    }
    state.routes.push(Route::Composer);
    state.focus = Focus::Composer;
    Vec::new()
}

/// Start the journal restore (plan §19 Phase 6 crash/restart acceptance).
/// Dispatched once at startup by the runtime.
fn load_drafts(state: &mut AppState) -> Vec<Effect> {
    if state.operations.is_loading_drafts() {
        return Vec::new();
    }
    vec![state.operations.start(OperationKind::LoadDrafts)]
}

/// Open the confirm-discard dialog (plan §14: discard only after explicit
/// confirmation). A no-op without a draft.
fn open_discard_confirm(state: &mut AppState) -> Vec<Effect> {
    let Some(composer) = state.composer.as_ref() else {
        return Vec::new();
    };
    let draft = composer.draft.snapshot();
    state.overlay = Some(Overlay::ConfirmDiscard(DiscardDialog {
        draft,
        button: ConfirmButton::Keep,
        previous_focus: state.focus,
    }));
    state.focus = Focus::ErrorModal;
    Vec::new()
}

/// Leave the composer (plan §14): pop the route, return to the prior
/// route, and FORCE a save of any unsaved revision — no debounce, never a
/// silent discard. The draft data stays in `AppState.composer` so compose
/// reopens it, and the save completes in the background. A save of the
/// current revision already in flight is not duplicated; an in-flight save
/// of an older revision is superseded by the forced one.
fn leave_composer(state: &mut AppState) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Composer)) {
        return Vec::new();
    }
    state.routes.pop();
    state.focus = Focus::MessageList;
    let Some(composer) = state.composer.as_ref() else {
        return Vec::new();
    };
    let dirty = composer.draft.is_dirty();
    let in_flight = composer.draft.local_id.as_ref().is_some_and(|local_id| {
        state
            .operations
            .is_saving_draft(local_id, composer.draft.revision)
    });
    if dirty && !in_flight {
        tracing::debug!(
            revision = composer.draft.revision,
            "forcing draft save on leave"
        );
        draft_save_effect(state).into_iter().collect()
    } else {
        Vec::new()
    }
}

/// Autosave (plan §14): when the two-second debounce has elapsed on a
/// dirty draft, transition it to saving and start one save operation. The
/// registry supersedes any older save of the same draft, so only the
/// newest revision is ever pushed (ADR 0002 §D.2 coalescing).
fn autosave_tick(state: &mut AppState, now: chrono::DateTime<chrono::FixedOffset>) -> Vec<Effect> {
    let Some(composer) = state.composer.as_mut() else {
        return Vec::new();
    };
    if composer.draft.save != DraftSaveState::Debouncing {
        return Vec::new();
    }
    // An edit made before the first tick (no clock yet) arms now, so the
    // debounce can never stall on a missing timestamp.
    if composer.draft.last_edit_at.is_none() {
        composer.draft.last_edit_at = Some(now);
        return Vec::new();
    }
    if !composer.draft.autosave_due(now, state.autosave_delay_ms) {
        return Vec::new();
    }
    tracing::debug!(
        revision = composer.draft.revision,
        "debounce elapsed; saving draft"
    );
    let snapshot = composer.draft.start_save(now);
    vec![state.operations.start(OperationKind::SaveDraft {
        draft: Box::new(snapshot),
    })]
}

/// Start a save of the current draft revision right away (forced saves:
/// follow-up after a stale success, retries). `None` when there is no
/// composer, no clock yet, or nothing unsaved to write.
fn draft_save_effect(state: &mut AppState) -> Option<Effect> {
    let now = state.clock?;
    let composer = state.composer.as_mut()?;
    if !composer.draft.is_dirty() {
        return None;
    }
    let snapshot = composer.draft.start_save(now);
    Some(state.operations.start(OperationKind::SaveDraft {
        draft: Box::new(snapshot),
    }))
}

fn switch_mailbox(state: &mut AppState, mailbox_id: &MailboxId) -> Vec<Effect> {
    if state.active_route()
        == Some(&Route::Mailbox(MailboxRoute {
            mailbox_id: mailbox_id.clone(),
        }))
    {
        return Vec::new();
    }
    // A reader, search, or composer open on top is replaced by the new
    // mailbox: the stack is rebuilt around the new root mailbox route — a
    // reader may sit above a search route — and the search's stashed
    // mailbox context is dropped with it (Phase 9.1).
    if !state.routes.is_empty() {
        state.routes.clear();
        state.search_return = None;
        state.open_message = Loadable::Idle;
        state.reader_scroll = 0;
        state.reader_attachment = None;
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
///
/// The composer overrides the cancel step (plan §10 composer contract:
/// "`Esc` save/leave; never silently discard"): leaving forces a save of
/// the draft instead of cancelling the in-flight autosave.
fn back_or_cancel(state: &mut AppState) -> Vec<Effect> {
    if let Some(Overlay::Error(dialog)) = state.overlay.take() {
        state.focus = dialog.previous_focus;
        return Vec::new();
    }
    if state.focus == Focus::SearchField {
        state.focus = Focus::MessageList;
        return Vec::new();
    }
    if matches!(state.active_route(), Some(Route::Composer)) {
        // The composer may sit at the root (no mailbox loaded yet); Esc
        // leaves it either way, preserving the draft (plan §14).
        return leave_composer(state);
    }
    if let Some(op) = state.operations.cancel_foreground() {
        tracing::info!(id = %op.id, kind = ?op.kind, "cancelled foreground operation");
        state.set_status(format!("{} — cancelled", op.kind.summary()));
        return Vec::new();
    }
    if state.routes.len() > 1 {
        // Leave an open search first: its results are regenerated on
        // re-submit, the mailbox context underneath is stashed (Phase 9.1).
        if matches!(state.active_route(), Some(Route::Search(_))) {
            leave_search(state);
            return Vec::new();
        }
        // Pop the reader: the mailbox route underneath still holds the
        // exact page, selection, and scroll.
        state.routes.pop();
        state.open_message = Loadable::Idle;
        state.reader_scroll = 0;
        state.reader_attachment = None;
        state.focus = Focus::MessageList;
        return Vec::new();
    }
    // Root route with nothing to cancel or close: exit cleanly (plan §19
    // Phase 1 acceptance: app exits with Esc/quit).
    state.quit_requested = true;
    Vec::new()
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

// ── Search (plan §16/§19 Phase 9) ────────────────────────────────────────

/// Enter in the search field: run the query against the current mailbox.
/// The query travels to the backend unchanged (Phase 9.2: no local parser).
/// The mailbox list context is stashed for an exact return (Phase 9.1), and
/// search results reuse the mailbox list's page/selection/scroll machinery.
/// Re-submitting while the search route is open re-runs the (edited) query
/// without disturbing the stashed context.
fn submit_search(state: &mut AppState) -> Vec<Effect> {
    // Submitting is the search field's Enter; dispatched elsewhere it is
    // inert (the field is the only submit affordance, plan §10).
    if state.focus != Focus::SearchField {
        return Vec::new();
    }
    let query = state.search_query.clone();
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
        if let Some(Route::Search(route)) = state.routes.last_mut() {
            route.query = query.clone();
        }
    } else {
        state.search_return = Some(ListStash {
            page: state.messages.clone(),
            selection: state.selection,
            scroll: state.list_scroll,
        });
        state.routes.push(Route::Search(SearchRoute {
            query: query.clone(),
            mailbox_id: mailbox_id.clone(),
        }));
    }
    state.selection = 0;
    state.list_scroll = 0;
    state.messages = Page::empty(state.messages.limit);
    state.focus = Focus::MessageList;
    let limit = state.messages.limit.max(1);
    vec![state.operations.start(OperationKind::Search(SearchRequest {
        mailbox_id,
        query,
        offset: 0,
        limit,
    }))]
}

/// Leave the search route: restore the stashed mailbox list context
/// (Phase 9.1) — page, selection, and scroll come back exactly as they
/// were, with no reload.
fn leave_search(state: &mut AppState) {
    if matches!(state.active_route(), Some(Route::Search(_))) {
        state.routes.pop();
        if let Some(stash) = state.search_return.take() {
            state.messages = stash.page;
            state.selection = stash.selection;
            state.list_scroll = stash.scroll;
        }
        state.focus = Focus::MessageList;
    }
}

/// Manual refresh (`Ctrl+R`): (re)load the mailbox listing while startup
/// has not completed, otherwise refresh the visible context — the mailbox
/// page or the open search results (Phase 9.5). Re-arms the periodic
/// timer, so a manual refresh never collides with an imminent auto one.
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
    state.last_refresh_at = state.clock;
    request_visible_page(state, state.messages.offset)
}

/// The periodic timer (Phase 9.4, plan §11): every
/// `refresh_interval_seconds` of injected clock time, refresh the visible
/// context in the background. The timer never interrupts conflicting work:
/// it stands down while a modal is open, the composer is on screen, or any
/// operation is in flight, and retries on the next tick once they clear.
/// The first tick arms the timer (the reducer has no clock before then).
fn auto_refresh_tick(
    state: &mut AppState,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Vec<Effect> {
    if state.refresh_interval_seconds == 0 {
        return Vec::new();
    }
    let Some(last) = state.last_refresh_at else {
        state.last_refresh_at = Some(now);
        return Vec::new();
    };
    let elapsed = (now - last).num_seconds().max(0) as u64;
    if elapsed < state.refresh_interval_seconds {
        return Vec::new();
    }
    let conflicts = state.overlay.is_some()
        || matches!(state.active_route(), Some(Route::Composer))
        || !state.operations.is_empty();
    if conflicts {
        tracing::debug!("auto refresh stood down: conflicting work in flight");
        return Vec::new();
    }
    state.last_refresh_at = Some(now);
    tracing::debug!(elapsed, "auto refresh");
    request_visible_page_background(state, state.messages.offset)
}

#[cfg(test)]
#[path = "reducer_tests.rs"]
mod tests;
