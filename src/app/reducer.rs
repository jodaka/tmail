//! Deterministic, I/O-free reducer (plan §5/§9).
//!
//! `reduce` is the only writer of `AppState`. State transitions that need
//! backend data start an operation in the registry and return an
//! [`Effect`] for the runtime to execute; results come back as
//! `Action::BackendCompleted` and apply only while their operation is still
//! registered, so stale, cancelled, or superseded results never win
//! (plan §11). Reducers never perform I/O themselves.

use crate::app::action::{Action, AttachmentBrowse, BulkOp, ClickTarget, ComposerEdit, SearchEdit};
use crate::app::composer::{ComposerField, ComposerState};
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::{
    DraftRemovalReason, OperationFailure, OperationId, OperationKind, OperationOrigin,
    OperationOutcome, OperationResult,
};
use crate::app::overlay::{
    AttachmentFileDialog, ConfirmButton, DiscardDialog, ErrorDialog, ModalButton, Overlay,
    ThemePickerDialog,
};
use crate::app::route::{MailboxRoute, MessageRoute, Route, SearchRoute};
use crate::app::sanitize::sanitize;
use crate::app::state::{AppState, ListStash, Loadable};
use crate::app::wizard::wizard_reduce;
use crate::domain::{
    DraftSaveState, Mailbox, MailboxId, MailboxRole, Message, MessageLocator, Page, PageRequest,
    SearchRequest, bare_message_id,
};

/// Apply `action` to `state`, returning backend work to spawn. Never
/// performs I/O, never panics on odd input.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    // The account configuration wizard (ADR 0003) owns the whole screen
    // while active: it intercepts every action — mailbox navigation,
    // warmup refreshes, modals — before anything else can react. Clock,
    // size, quit, and backend results stay live.
    if state.wizard.is_some() {
        return wizard_reduce(state, action);
    }
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
        // The theme picker previews palettes from inside the modal path;
        // the body caret must track it like every other action.
        sync_body_caret(state);
        return effects;
    }
    let effects = match action {
        Action::MoveUp => move_selection(state, -1),
        Action::MoveDown => move_selection(state, 1),
        Action::PagePrevious => page_step(state, -1),
        Action::PageNext => page_step(state, 1),
        Action::Activate => activate(state),
        Action::BackOrCancel => back_or_cancel(state),
        Action::FocusNext => focus_step(state, 1),
        Action::FocusPrevious => focus_step(state, -1),
        Action::OpenSearch => {
            // Search lives on the mailbox screen; while composing, the '/'
            // is composed text (plan §10: shortcuts never fire in fields).
            // The sidebar focus inside compose mode is gated the same way:
            // submitting cannot run over the composer, so the field must
            // not strand focus there.
            if state.focus != Focus::Composer
                && !matches!(state.active_route(), Some(Route::Composer))
            {
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
        Action::MarkRead => mark_read(state),
        Action::ToggleSelected => toggle_selected(state),
        Action::SelectAll => toggle_select_all(state),
        Action::Compose => open_composer(state),
        Action::LoadDrafts => load_drafts(state),
        Action::ComposerEdit(edit) => {
            // Editing targets the focused composer control; without a
            // composer open (or without its focus) the edit is inert.
            // Content edits sync the draft and re-arm autosave (plan §14);
            // caret moves leave the revision untouched. Undo/redo (ticket
            // kfmt) rewrite the body content only when the history
            // applies a step, so they sync conditionally. While a send of
            // this draft is in flight (Phase 7.6) all editing is frozen:
            // the bytes on the wire must stay what the user saw.
            if state.focus == Focus::Composer
                && let Some(composer) = state.composer.as_mut()
            {
                if composer.sending {
                    tracing::debug!("composer edits frozen while sending");
                } else {
                    let content_changed = match edit {
                        ComposerEdit::Undo => composer.undo_body(),
                        ComposerEdit::Redo => composer.redo_body(),
                        _ => {
                            composer.apply(edit);
                            edit.is_content_edit()
                        }
                    };
                    if content_changed {
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
        Action::EditExternal => edit_externally(state),
        Action::EditorFinished { id, result } => editor_finished(state, *id, result.clone()),
        Action::RetryError
        | Action::DismissError
        | Action::DialogEdit(_)
        | Action::AttachmentBrowse(_) => {
            // Only meaningful with their modal open (handled above).
            Vec::new()
        }
        Action::Wizard(_) => {
            // Only meaningful with the wizard active (handled by the
            // interception at the top of `reduce`).
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
        Action::OpenThemePicker => open_theme_picker(state),
        Action::Tick { now } => tick(state, **now),
        Action::Resize { width, height } => resize(state, *width, *height),
        Action::Quit => {
            state.quit_requested = true;
            Vec::new()
        }
    };
    sync_body_caret(state);
    effects
}

/// Keep the body editor's caret and selection styles in step (tickets
/// tz12, kfmt): the accent caret block while the body is focused — the
/// same one the single-line fields draw — and no caret otherwise. Runs
/// after every action so focus cycling, selection changes, and live
/// theme switches (theme picker preview) can never show a stale style.
fn sync_body_caret(state: &mut AppState) {
    let theme = state.active_theme();
    if let Some(composer) = state.composer.as_mut() {
        composer.sync_body_styles(&theme);
    }
}

/// The system handlers (injected clock, terminal size, quit) the wizard
/// slice reuses: during the wizard these stay live while every other
/// action is swallowed.
pub(crate) fn reduce_unwizarded(state: &mut AppState, action: &Action) -> Vec<Effect> {
    match action {
        Action::Tick { now } => tick(state, **now),
        Action::Resize { width, height } => resize(state, *width, *height),
        Action::Quit => {
            state.quit_requested = true;
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn tick(state: &mut AppState, now: chrono::DateTime<chrono::FixedOffset>) -> Vec<Effect> {
    state.ticks += 1;
    state.clock = Some(now);
    clear_expired_status(state, now);
    let mut effects = autosave_tick(state, now);
    effects.extend(auto_refresh_tick(state, now));
    effects
}

fn resize(state: &mut AppState, width: u16, height: u16) -> Vec<Effect> {
    state.size = (width, height);
    // Ticket kjfq: auto-sized pages track the terminal — the limit
    // is however many rows fit the list right now. A change re-loads
    // the visible page in the background (supersession collapses a
    // resize storm; the newest request wins) so the list refills.
    let mut effects = Vec::new();
    if state.page_size_auto {
        let visible = crate::ui::layout::messages_visible(state.size, state.view_mode).max(1);
        if state.messages.limit != visible {
            state.messages.limit = visible;
            if !state.messages.items.is_empty() {
                effects = request_visible_page_background(state, state.messages.offset);
            }
        }
    }
    // A smaller window may have pushed the selection off screen.
    keep_selection_visible(state);
    clamp_reader_scroll(state);
    effects
}

// ── Modal overlays (plan §9/§12) ─────────────────────────────────────────

/// Status-message timeout (ticket h1d7): with `[tmail].status_timeout > 0`
/// a message set at `status.shown_at` clears when the window elapses. The
/// renderer fades it toward the background over the closing 0.3 s of the
/// window (`statusbar::render`); here it only leaves the state. The
/// default `0` keeps a message until the next one replaces it.
fn clear_expired_status(state: &mut AppState, now: chrono::DateTime<chrono::FixedOffset>) {
    if state.status_timeout_seconds == 0 || state.status.message.is_none() {
        return;
    }
    let Some(shown_at) = state.status.shown_at else {
        return;
    };
    let elapsed = (now - shown_at).num_seconds().max(0) as u64;
    if elapsed >= state.status_timeout_seconds {
        state.status.message = None;
        state.status.shown_at = None;
    }
}

/// Handle `action` while any modal is open. Returns `None` when no modal
/// is open (the caller falls through to normal handling). The attachment
/// dialog likewise falls through for `BackendCompleted`: the pending
/// validation result must land while the dialog is up.
fn modal_reduce(state: &mut AppState, action: &Action) -> Option<Vec<Effect>> {
    match state.overlay {
        Some(Overlay::Error(_)) => Some(error_modal_reduce(state, action)),
        Some(Overlay::ConfirmDiscard(_)) => Some(discard_modal_reduce(state, action)),
        Some(Overlay::AttachmentExplorer(_)) => match action {
            Action::BackendCompleted(_) => None,
            _ => Some(attachment_dialog_reduce(state, action)),
        },
        Some(Overlay::ThemePicker(_)) => Some(theme_picker_reduce(state, action)),
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

/// Attachment file chooser handling (plan §15, ticket 95x0). The
/// explorer is plain state: pure selection moves apply at once, while
/// directory changes are backend listings — navigation that changes
/// directories freezes until the listing lands. Enter submits the
/// selected file for backend validation; Esc cancels. Rejections keep
/// the dialog open with the detail inline.
fn attachment_dialog_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Some(Overlay::AttachmentExplorer(dialog)) = state.overlay.as_mut() else {
        return Vec::new();
    };
    // Directory changes and submissions are backend work: the dialog is
    // frozen/marked here and the operation starts on the registry — a
    // borrow of the `operations` field only, disjoint from the overlay
    // the dialog was borrowed from.
    match action {
        Action::AttachmentBrowse(browse) => {
            // Pure selection movement: no filesystem access, applied at
            // once (also while a listing is in flight — the landing
            // listing replaces the selection anyway).
            let pure = match browse {
                AttachmentBrowse::Up => Some(ratatui_explorer::Input::Up),
                AttachmentBrowse::Down => Some(ratatui_explorer::Input::Down),
                AttachmentBrowse::PageUp => Some(ratatui_explorer::Input::PageUp),
                AttachmentBrowse::PageDown => Some(ratatui_explorer::Input::PageDown),
                AttachmentBrowse::Parent | AttachmentBrowse::Open => None,
            };
            if let Some(input) = pure {
                dialog.browse(input);
                Vec::new()
            } else if let Some(path) = dialog.step_target(*browse == AttachmentBrowse::Parent) {
                if dialog.listing {
                    Vec::new()
                } else {
                    dialog.listing = true;
                    dialog.error = None;
                    vec![
                        state
                            .operations
                            .start(OperationKind::ListAttachmentFiles { path: Some(path) }),
                    ]
                }
            } else {
                Vec::new()
            }
        }
        Action::Activate => {
            // A directory descends; a file submits for validation.
            match dialog.step_target(false) {
                Some(dir) => {
                    if dialog.listing {
                        Vec::new()
                    } else {
                        dialog.listing = true;
                        dialog.error = None;
                        vec![
                            state
                                .operations
                                .start(OperationKind::ListAttachmentFiles { path: Some(dir) }),
                        ]
                    }
                }
                None => match dialog.selected_file() {
                    Some(file) => {
                        dialog.error = None;
                        vec![
                            state
                                .operations
                                .start(OperationKind::ReadAttachment { path: file }),
                        ]
                    }
                    None => Vec::new(),
                },
            }
        }
        // Esc closes without attaching (plan §10: Esc cancels overlays).
        Action::BackOrCancel => {
            let focus = dialog.previous_focus;
            state.overlay = None;
            state.focus = focus;
            Vec::new()
        }
        // Everything else is swallowed while the dialog is open.
        _ => Vec::new(),
    }
}

/// Open the theme picker (ticket k5ba). The cursor starts on the active
/// palette — Enter is a no-op until the user moves, and the preview begins
/// from where the user already is. No-op without a theme list.
fn open_theme_picker(state: &mut AppState) -> Vec<Effect> {
    if state.themes.is_empty() {
        return Vec::new();
    }
    let cursor = state.theme_index.min(state.themes.len() - 1);
    // The scroll window opens with the cursor row on screen: a long theme
    // list must not hide the palette the user is currently on.
    let visible = crate::ui::components::theme_picker::visible_rows(state.size).max(1);
    let scroll = if cursor >= visible {
        (cursor + 1 - visible).min(crate::ui::components::theme_picker::max_scroll(
            state.themes.len(),
            state.size,
        ))
    } else {
        0
    };
    state.overlay = Some(Overlay::ThemePicker(ThemePickerDialog {
        original: state.theme_index,
        cursor,
        scroll,
        previous_focus: state.focus,
    }));
    state.focus = Focus::ThemePicker;
    Vec::new()
}

/// Theme picker handling (ticket k5ba). Up/Down move the cursor and apply
/// the highlighted palette at once — the preview the ticket asks for —
/// with the same clamped (non-wrapping) movement the message list uses.
/// Enter keeps the previewed palette; Esc restores the palette the picker
/// opened with. Everything else is swallowed while the picker is open.
fn theme_picker_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    // Viewport math comes from the renderer's layout, so the clamp the
    // reducer computes always matches what is drawn.
    let len = state.themes.len();
    let visible = crate::ui::components::theme_picker::visible_rows(state.size).max(1);
    let max_scroll = crate::ui::components::theme_picker::max_scroll(len, state.size);
    let Some(Overlay::ThemePicker(dialog)) = state.overlay.as_mut() else {
        return Vec::new();
    };
    match action {
        Action::MoveUp | Action::MoveDown => {
            let delta = if matches!(action, Action::MoveUp) {
                -1
            } else {
                1
            };
            let next = (dialog.cursor as i64 + delta).clamp(0, len as i64 - 1) as usize;
            dialog.cursor = next;
            dialog.scroll = if next < dialog.scroll {
                next
            } else if next >= dialog.scroll + visible {
                (next + 1 - visible).min(max_scroll)
            } else {
                dialog.scroll
            };
            // The highlighted theme is the preview: it applies at once.
            state.theme_index = next;
        }
        Action::BackOrCancel => {
            // Esc never changes the theme: the preview is undone by
            // restoring the index the picker opened with.
            state.theme_index = dialog.original.min(len.saturating_sub(1));
            let focus = dialog.previous_focus;
            state.overlay = None;
            state.focus = focus;
        }
        Action::Activate => {
            let name = state
                .themes
                .get(state.theme_index)
                .map(|(name, _)| name.as_str())
                .unwrap_or("default")
                .to_owned();
            let focus = dialog.previous_focus;
            state.overlay = None;
            state.focus = focus;
            state.set_status(format!("Theme: {name}"));
        }
        _ => {}
    }
    Vec::new()
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
        ClickTarget::ComposerField(field) => click_composer_field(state, field),
        // Modal buttons outside a modal cannot happen (their regions are
        // only recorded while the modal renders); the arm keeps the match
        // total.
        ClickTarget::ErrorButton(_) | ClickTarget::ConfirmButton(_) => Vec::new(),
        ClickTarget::BulkAction(op) => {
            // The buttons act on the selection (ticket p0s3): focus the
            // list first so the bulk path (not the reader path) applies,
            // then run the advertised action's exact code path.
            if state.selection_active() {
                state.focus = Focus::MessageList;
            }
            match op {
                BulkOp::Trash => reduce(state, &Action::Trash),
                BulkOp::Archive => reduce(state, &Action::Archive),
                BulkOp::MarkRead => reduce(state, &Action::MarkRead),
                BulkOp::MarkUnread => reduce(state, &Action::MarkUnread),
            }
        }
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
            Some(summary) => open_selected(state, summary),
            None => Vec::new(),
        };
    }
    state.selection = index;
    keep_selection_visible(state);
    Vec::new()
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
    if composer_open(state) {
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
    if composer_open(state) {
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
    if composer_open(state) {
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
    install_composer_draft(state, draft, status);
    Vec::new()
}

/// Put a ready draft into the composer and open the composer screen over
/// the current route. The one-composer rule lives with the callers: this
/// installs unconditionally.
fn install_composer_draft(state: &mut AppState, draft: crate::domain::Draft, status: &str) {
    state.composer = Some(ComposerState::from_draft(draft));
    if !matches!(state.active_route(), Some(Route::Composer)) {
        state.routes.push(Route::Composer);
    }
    state.focus = Focus::Composer;
    state.set_status(status);
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

/// Locators for a bulk operation (ticket p0s3): only when the list holds
/// focus, selection mode is on, and the visible selection is non-empty.
/// Reader shortcuts keep acting on the open message even while a selection
/// exists — the selection belongs to the list behind the reader.
fn bulk_targets(state: &AppState) -> Option<Vec<MessageLocator>> {
    if state.focus != Focus::MessageList || !state.selection_active() {
        return None;
    }
    let locators = state.selected_locators();
    (!locators.is_empty()).then_some(locators)
}

fn archive_message(state: &mut AppState) -> Vec<Effect> {
    bulk_or_single(
        state,
        "Archiving {count} messages…",
        "Archiving…",
        false,
        OperationKind::Archive,
    )
}

fn trash_message(state: &mut AppState) -> Vec<Effect> {
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
/// on open, so the single path stays list-only).
fn mark_read(state: &mut AppState) -> Vec<Effect> {
    bulk_or_single(
        state,
        "Marking {count} messages read…",
        "Marking read…",
        true,
        |locator| OperationKind::SetRead {
            locator,
            read: true,
        },
    )
}

/// The unread counterpart (ticket p0s3).
fn mark_unread(state: &mut AppState) -> Vec<Effect> {
    bulk_or_single(
        state,
        "Marking {count} messages unread…",
        "Marking unread…",
        false,
        |locator| OperationKind::SetRead {
            locator,
            read: false,
        },
    )
}

/// The target summary carries the current state to invert; the UI only
/// flips once the backend confirms (plan §19 Phase 4 acceptance).
fn toggle_star(state: &mut AppState) -> Vec<Effect> {
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

/// The shared "bulk in selection mode, else the focused row" skeleton of
/// the flag operations (ticket p0s3): builds one [`OperationKind`] per
/// target from `make`, and forms the status message — `bulk` templates one
/// `{count}` plural in bulk mode, `single` is the fixed phrase for one
/// message. `list_only` keeps the single path from firing over the reader
/// (mark read/unread are list shortcuts today); the bulk path always
/// implies list focus.
fn bulk_or_single(
    state: &mut AppState,
    bulk: &str,
    single: &str,
    list_only: bool,
    make: impl Fn(MessageLocator) -> OperationKind,
) -> Vec<Effect> {
    if let Some(locators) = bulk_targets(state) {
        state.set_status(bulk.replace("{count}", &locators.len().to_string()));
        return locators
            .into_iter()
            .map(|locator| state.operations.start(make(locator)))
            .collect();
    }
    if list_only && state.focus != Focus::MessageList {
        return Vec::new();
    }
    match message_target(state) {
        Some(locator) => {
            state.set_status(single);
            vec![state.operations.start(make(locator))]
        }
        None => Vec::new(),
    }
}

// ── Bulk selection (ticket p0s3) ─────────────────────────────────────────

/// Space on a focused message row: toggle its bulk-selection mark, then
/// advance the cursor to the next row (ticket yy4m) so several messages
/// can be marked by pressing Space repeatedly. The mark rides the backend
/// id, so it survives paging and refreshes while the row stays listed.
fn toggle_selected(state: &mut AppState) -> Vec<Effect> {
    if state.focus != Focus::MessageList {
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
fn toggle_select_all(state: &mut AppState) -> Vec<Effect> {
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
/// A result payload that cannot belong to this operation kind: wiring
/// bugs of the manager/reducer contract, never user-facing — log and
/// keep the state untouched.
fn unexpected_payload(id: OperationId, kind: &str) -> Vec<Effect> {
    tracing::warn!(id = %id, "unexpected payload for a {kind} operation");
    Vec::new()
}

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
                    // Ticket haeb: every successful listing refreshes the
                    // cached sidebar.
                    if let Some(cache) = &state.page_cache {
                        cache.store_mailboxes(mailboxes);
                    }
                    mailboxes_loaded(state, mailboxes.clone())
                }
                Ok(OperationOutcome::Page(_)) => {
                    tracing::warn!(id = %result.id, "page payload for a mailbox operation");
                    Vec::new()
                }
                Ok(_) => unexpected_payload(result.id, "mailbox"),
                Err(failure) => {
                    // The sidebar keeps a dim failed note; the modal carries
                    // the full sanitized detail and the retry intent.
                    state.mailboxes = Loadable::Failed(failure.detail.clone());
                    open_error_modal(state, failure)
                }
            }
        }
        OperationKind::LoadPage(request) => {
            // Currency check: the request must still target the mailbox
            // whose page the visible list shows. A newer request for the
            // same mailbox superseded this operation, so its id would
            // already be unknown above; this guard drops results that
            // raced a mailbox switch or a search taking over the list.
            // Reader and composer routes are overlays: a load finishing
            // while the user reads or composes still updates the list
            // behind them — routes, focus, and selection untouched
            // (ticket sazy).
            let current = visible_mailbox_page(state).is_some_and(|id| *id == request.mailbox_id);
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
                    let effects = apply_page(state, page.clone());
                    // Ticket haeb: every successful load refreshes the
                    // cached page — with the previews applied (ticket
                    // wxtx), so the next cold start renders rows without
                    // re-fetching anything.
                    if let Some(cache) = &state.page_cache {
                        cache.store(&request.mailbox_id, None, &state.messages);
                    }
                    effects
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
                _ => unexpected_payload(result.id, "page"),
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
                    let effects = apply_page(state, page.clone());
                    // Ticket haeb: search results cache under their query —
                    // with the previews applied (ticket wxtx).
                    if let Some(cache) = &state.page_cache {
                        cache.store(&request.mailbox_id, Some(&request.query), &state.messages);
                    }
                    effects
                }
                Ok(_) => unexpected_payload(result.id, "search"),
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
                Ok(_) => unexpected_payload(result.id, "message"),
                Err(failure) => {
                    // The reader shows a failure placeholder; the modal
                    // carries Retry/Dismiss (plan §12). Coherent state.
                    state.open_message = Loadable::Failed(failure.detail.clone());
                    open_error_modal(state, failure)
                }
            }
        }
        OperationKind::OpenDraft(locator) => {
            // Currency check: the drafts mailbox must still be displayed
            // (a switch, a reader, or a composer opened meanwhile drops the
            // result — Enter again refetches).
            let current = match state.active_route() {
                Some(Route::Mailbox(route)) => route.mailbox_id == locator.mailbox,
                Some(Route::Search(route)) => route.mailbox_id == locator.mailbox,
                _ => false,
            };
            state.operations.finish(result.id);
            if !current {
                tracing::debug!(
                    id = %result.id,
                    mailbox = %locator.mailbox.0,
                    "dropping draft result for a closed context"
                );
                return Vec::new();
            }
            match &result.outcome {
                Ok(OperationOutcome::Message(message)) => {
                    draft_message_loaded(state, (**message).clone())
                }
                Ok(_) => unexpected_payload(result.id, "draft"),
                Err(failure) => open_error_modal(state, failure),
            }
        }
        OperationKind::Preview(_) => {
            state.operations.finish(result.id);
            match &result.outcome {
                Ok(OperationOutcome::Message(message)) => {
                    preview_loaded(state, (**message).clone())
                }
                Ok(_) => unexpected_payload(result.id, "preview"),
                Err(failure) => {
                    // A preview is decorative background context (ticket
                    // wxtx): its failure never interrupts the user and is
                    // never retried — the row simply keeps no snippet.
                    tracing::debug!(
                        id = %result.id,
                        detail = %failure.detail,
                        "preview fetch failed; row stays without a snippet"
                    );
                    Vec::new()
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
                    unexpected_payload(result.id, "flag");
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
                    unexpected_payload(result.id, "flag");
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
                Ok(_) => unexpected_payload(result.id, "move"),
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
                Ok(_) => unexpected_payload(result.id, "draft restore"),
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
            // removal reason: discards open the modal, tmail-send cleanup
            // is best-effort (ADR 0002) and never claims a failed send.
            match &result.outcome {
                Ok(OperationOutcome::Done) => Vec::new(),
                Ok(_) => unexpected_payload(result.id, "draft removal"),
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
                Ok(_) => unexpected_payload(result.id, "send"),
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
        OperationKind::ListAttachmentFiles { .. } => {
            state.operations.finish(result.id);
            attachment_listing_ready(state, result)
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
                Ok(_) => unexpected_payload(result.id, "attachment save"),
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
                Ok(_) => unexpected_payload(result.id, "open"),
                Err(failure) => open_error_modal(state, failure),
            }
        }
        // The external editor completes through `Action::EditorFinished`,
        // not the result channel (Phase 11: it runs on the terminal owner,
        // not in the manager). A result arriving here would be a routing
        // bug; finish quietly so the registry cannot leak.
        OperationKind::EditExternally { .. } => {
            tracing::warn!(id = %result.id, "result for an external-editor operation");
            state.operations.finish(result.id);
            Vec::new()
        }
        // Wizard operations complete through the wizard slice, which
        // intercepts `BackendCompleted` first (ADR 0003 §3.7). Reaching
        // this arm would be a routing bug; finish so nothing leaks.
        OperationKind::DiscoverConfig { .. }
        | OperationKind::TestAccount { .. }
        | OperationKind::SaveAccount { .. } => {
            tracing::warn!(id = %result.id, "wizard result reached the main backend path");
            state.operations.finish(result.id);
            Vec::new()
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
/// still be the one in the composer (a discarded draft is gone; a draft
/// swapped out by opening another one is no longer in the slot — the
/// backend already has the pushed copy, so the local confirm is dropped).
/// Success for the newest revision marks the draft saved; a stale success
/// (edits happened meanwhile) immediately chains another save so revision
/// N+1 is never left unpushed.
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
            "dropping draft result for a discarded or replaced draft"
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
        Ok(_) => unexpected_payload(result.id, "draft save"),
    }
}

/// Apply a finished attachment validation (plan §15, ticket 95x0).
/// Currency: the dialog must still be open, and the chooser's selection
/// must still be the validated file — navigation in the meantime drops
/// the result. A validated file becomes a chip and a content edit
/// (autosave carries the attachment list into the journal); a rejection
/// keeps the chooser open with the detail inline, retryable by pressing
/// Enter on the file again.
fn attachment_validated(
    state: &mut AppState,
    path: &std::path::Path,
    result: &OperationResult,
) -> Vec<Effect> {
    let Some(Overlay::AttachmentExplorer(dialog)) = state.overlay.as_ref() else {
        tracing::debug!(id = %result.id, "dropping attachment validation for a closed dialog");
        return Vec::new();
    };
    let selection_matches = dialog
        .selected_file()
        .is_some_and(|selected| selected == path);
    if !selection_matches {
        tracing::debug!(id = %result.id, "dropping attachment validation for a moved selection");
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
            // Detailed and retryable in place: the chooser stays open with
            // the same selection (plan §15 acceptance, ticket 95x0).
            let detail = failure.detail.clone();
            if let Some(Overlay::AttachmentExplorer(dialog)) = state.overlay.as_mut() {
                dialog.error = Some(detail);
            }
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "attachment validation"),
    }
}

/// Apply a finished directory listing (ticket 95x0). Currency: the
/// dialog must still be open — an Esc in the meantime drops the result.
/// A landed listing replaces the chooser's explorer state; a failed one
/// keeps the dialog open with the detail inline (navigation stays free,
/// so the user can head elsewhere or Esc).
fn attachment_listing_ready(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    if !matches!(state.overlay, Some(Overlay::AttachmentExplorer(_))) {
        tracing::debug!(id = %result.id, "dropping directory listing for a closed dialog");
        return Vec::new();
    }
    match &result.outcome {
        Ok(OperationOutcome::Explorer(explorer)) => {
            let mut explorer = explorer.as_ref().clone();
            // The chooser renders with the active palette; theme changes
            // cannot happen while a modal is up, so this sticks.
            let theme = state.active_theme();
            explorer.set_theme(crate::ui::components::attachment_dialog::explorer_theme(
                &theme,
            ));
            if let Some(Overlay::AttachmentExplorer(dialog)) = state.overlay.as_mut() {
                dialog.explorer = Some(explorer);
                dialog.listing = false;
                dialog.error = None;
            }
            Vec::new()
        }
        Err(failure) => {
            let detail = failure.detail.clone();
            if let Some(Overlay::AttachmentExplorer(dialog)) = state.overlay.as_mut() {
                dialog.listing = false;
                dialog.error = Some(detail);
            }
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "directory listing"),
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

/// Apply the fetched message: show it, fill the list snippet (the same
/// one-line body preview the background previews produce, ticket wxtx),
/// and mark unread mail read after successful load (plan §19 Phase 4) as a
/// separate, retryable flag operation whose confirmation updates the list.
fn message_loaded(state: &mut AppState, message: Message) -> Vec<Effect> {
    let snippet = crate::ui::rich::preview_text(&message);
    let message_id = message.id.clone();
    let has_attachments = !message.attachments.is_empty();
    // Ticket haeb: cache the viewed message (bounded by [tmail.cache]).
    if let Some(cache) = &state.page_cache
        && let Some(Route::Message(route)) = state.active_route()
    {
        cache.store_message(&route.mailbox_id, &message_id.0, &message);
    }
    state.open_message = Loadable::Loaded(message);
    // The parsed message knows attachments better than the envelope did
    // (ticket r84f: IMAP envelopes carry no body structure, so the flag
    // was false and the paperclip never rendered).
    sync_row_attachments(state, &message_id, has_attachments);
    if let Some(snippet) = snippet {
        // The session keeps the preview: page loads and refreshes restore
        // it instead of re-fetching the message (ticket wxtx).
        state.previews.insert(message_id.clone(), snippet.clone());
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
    let locator = route.summary.into_locator();
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
fn sync_row_attachments(
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
fn mailboxes_loaded(state: &mut AppState, mailboxes: Vec<Mailbox>) -> Vec<Effect> {
    // Cold start: no mailbox context established yet (empty stack). Warm
    // start: the stack is rooted at a mailbox — possibly under a reader,
    // search, or composer the user opened while the fetch ran; those all
    // stay.
    if !matches!(state.routes.first(), Some(Route::Mailbox(_))) {
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
fn refresh_sidebar_listing(state: &mut AppState, mailboxes: Vec<Mailbox>) {
    let cursor_id = state
        .mailboxes
        .as_loaded()
        .and_then(|list| list.get(state.mailbox_selection))
        .map(|m| m.id.clone());
    // The mailbox the UI is rooted at (the warm path guarantees one).
    let active_id = match state.routes.first() {
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
fn apply_mailbox_listing(state: &mut AppState, mailboxes: Vec<Mailbox>) -> Vec<Effect> {
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

/// The mailbox whose page the visible list currently shows, when it shows
/// one: the nearest mailbox route on the stack. Reader and composer routes
/// are overlays over the list beneath them (the walk continues past them),
/// so a page load finishing behind them still applies (ticket sazy). A
/// search route owns the visible list with query results — a mailbox page
/// must never land there, so the walk stops.
fn visible_mailbox_page(state: &AppState) -> Option<&MailboxId> {
    for route in state.routes.iter().rev() {
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
fn apply_page(state: &mut AppState, page: Page<crate::domain::MessageSummary>) -> Vec<Effect> {
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
            && let Some(text) = state.previews.get(&item.id)
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
const MAX_IN_FLIGHT_PREVIEWS: usize = 6;

/// Satisfy the visible rows' previews (ticket wxtx), in priority order:
/// rows this session already previewed were restored by `apply_page`;
/// rows whose full message is already cached on disk (an earlier fetch,
/// this session or a previous one) take their preview straight from the
/// cache — no backend work, and old cached messages are never re-fetched.
/// Only genuinely unknown messages start a background fetch, each
/// independently, within the rolling window. Rows beyond the window stay
/// unrequested so the next apply or preview completion picks them up.
fn start_missing_previews(state: &mut AppState) -> Vec<Effect> {
    let mut budget = MAX_IN_FLIGHT_PREVIEWS.saturating_sub(state.operations.previews_in_flight());
    let mut effects = Vec::new();
    let candidates: Vec<crate::domain::MessageSummary> = state
        .messages
        .items
        .iter()
        .filter(|s| s.snippet.is_none() && !state.preview_requested.contains(&s.id))
        .cloned()
        .collect();
    for summary in candidates {
        let cached = state
            .page_cache
            .as_ref()
            .and_then(|cache| cache.load_message(&summary.mailbox_id, &summary.id.0));
        let text = cached.as_ref().and_then(crate::ui::rich::preview_text);
        match text {
            Some(text) => {
                // Already fetched (this session or before): serve the
                // preview from the copy on disk and remember it for the
                // session. The read refreshed the entry's LRU stamp, so a
                // previewed message stays cached like a viewed one.
                state.preview_requested.insert(summary.id.clone());
                state.previews.insert(summary.id.clone(), text.clone());
                if let Some(item) = state.messages.items.iter_mut().find(|s| s.id == summary.id) {
                    item.snippet = Some(text);
                    item.has_attachments = cached
                        .as_ref()
                        .is_some_and(|message| !message.attachments.is_empty());
                }
            }
            None if cached.is_some() => {
                // Fetched before, but the body carries no preview text
                // (an empty body): satisfied, nothing to request. The
                // attachment flag still reconciles from the cached copy.
                state.preview_requested.insert(summary.id.clone());
                sync_row_attachments(
                    state,
                    &summary.id,
                    cached
                        .as_ref()
                        .is_some_and(|message| !message.attachments.is_empty()),
                );
            }
            None if budget > 0 => {
                // Genuinely unknown: fetch in the background, once.
                budget -= 1;
                state.preview_requested.insert(summary.id.clone());
                effects.push(
                    state
                        .operations
                        .start_background(OperationKind::Preview(summary.into_locator())),
                );
            }
            // Beyond the window: leave unrequested for the rolling refill.
            None => {}
        }
    }
    effects
}

/// Apply a fetched preview (ticket wxtx): convert the body to one line of
/// plain text, remember it for the session, fill the list row's snippet,
/// cache the message for an instant open, and roll the fetch window. A
/// result for a message no longer listed (mailbox switched, row moved) is
/// dropped — but still cached, so it helps if the message returns.
fn preview_loaded(state: &mut AppState, message: Message) -> Vec<Effect> {
    if let Some(cache) = &state.page_cache {
        cache.store_message(&message.mailbox_id, &message.id.0, &message);
    }
    let message_id = message.id.clone();
    if let Some(text) = crate::ui::rich::preview_text(&message) {
        state.previews.insert(message_id.clone(), text.clone());
    }
    // The parsed message knows attachments better than the envelope did
    // (ticket r84f: IMAP envelopes carry no body structure, so the flag
    // was false and the paperclip never rendered). The row's snippet,
    // when still missing, fills from the preview computed above.
    let has_attachments = !message.attachments.is_empty();
    if let Some(summary) = state.messages.items.iter_mut().find(|s| s.id == message_id) {
        summary.has_attachments = has_attachments;
        if summary.snippet.is_none() {
            summary.snippet = state.previews.get(&message_id).cloned();
        }
    }
    start_missing_previews(state)
}

// ── Navigation and input ─────────────────────────────────────────────────

/// Tab/Shift+Tab focus cycling (plan §10). While composing, the folder
/// list joins the composer's cycle — Tab past the last control (Shift+Tab
/// before the first) steps out to the sidebar, and the next step returns
/// into the composer's first (last) control — so a draft can be parked on
/// any mailbox without leaving the composer: switching preserves the draft
/// (plan §14). The mailbox screen's other focusable controls (search
/// field, message list) are off screen while the
/// composer replaces the list, so the cycle skips them.
fn focus_step(state: &mut AppState, delta: i64) -> Vec<Effect> {
    match state.focus {
        Focus::Composer => {
            let Some(composer) = state.composer.as_mut() else {
                return Vec::new();
            };
            // The sidebar sits just past the cycle's ends: Tab leaves from
            // the last control, Shift+Tab from the first.
            let step_out = (delta > 0 && composer.field == ComposerField::Discard)
                || (delta < 0 && composer.field == ComposerField::To);
            if step_out {
                state.focus = Focus::Sidebar;
            } else if delta > 0 {
                composer.focus_next();
            } else {
                composer.focus_previous();
            }
            Vec::new()
        }
        // Back into the composer: Tab re-enters at the first control,
        // Shift+Tab at the last — one linear cycle.
        Focus::Sidebar if matches!(state.active_route(), Some(Route::Composer)) => {
            state.focus = Focus::Composer;
            let field = if delta > 0 {
                ComposerField::To
            } else {
                ComposerField::Discard
            };
            if let Some(composer) = state.composer.as_mut() {
                composer.focus_field(field);
            }
            Vec::new()
        }
        // In the reader Tab walks the attachment chips (plan §15): the
        // selection the save/open keys act on.
        Focus::Reader => {
            cycle_reader_attachment(state, delta);
            Vec::new()
        }
        _ => {
            state.focus = if delta > 0 {
                state.focus.next()
            } else {
                state.focus.previous()
            };
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
        Focus::Reader => scroll_reader(state, delta),
        // Wizard input never reaches the mailbox navigation (the wizard
        // intercepts everything first, ADR 0003).
        Focus::Composer
        | Focus::Dialog
        | Focus::ThemePicker
        | Focus::ErrorModal
        | Focus::Wizard => {}
    }
    Vec::new()
}

/// Left/Right: pages in the message list, viewport steps in the reader.
fn page_step(state: &mut AppState, delta: i64) -> Vec<Effect> {
    match state.focus {
        Focus::Reader => {
            let (viewport, _) = reader_scroll_bounds(state);
            scroll_reader(state, delta * viewport);
            Vec::new()
        }
        _ => change_page(state, delta),
    }
}

/// Scroll the reader body (Up/Down in reader focus, plan §10: "scroll
/// focused area"). The budget comes from the same pure content functions
/// the renderer draws, so the reducer's clamp always matches the frame.
/// The fixed header never scrolls (ticket 6864): the viewport is the rows
/// under it and the clamp tracks the body alone.
fn scroll_reader(state: &mut AppState, delta: i64) {
    let (viewport, total) = reader_scroll_bounds(state);
    let max = (total - viewport).max(0);
    let next = (state.reader_scroll as i64 + delta).clamp(0, max);
    state.reader_scroll = next as usize;
}

/// The reader's scrollable viewport (body rows under the fixed header) and
/// the scrollable body length, from the same pure functions the renderer
/// draws — reducer and frame can never disagree (ticket 6864).
fn reader_scroll_bounds(state: &AppState) -> (i64, i64) {
    let width = crate::ui::layout::reader_width(state.size).max(10);
    let viewport = crate::ui::layout::reader_rows_visible(state.size)
        .saturating_sub(crate::ui::screens::reader::header_line_count(state, width))
        .max(1) as i64;
    let total = crate::ui::screens::reader::scroll_line_count(state, width) as i64;
    (viewport, total)
}

/// Reflow on terminal resize (plan §13/§19 Phase 5): the document re-wraps
/// at the new reader width inside the renderer, and the scroll anchor is
/// clamped to the re-flowed body length so the viewport can never point
/// past the end of the document (the fixed header never scrolls, ticket
/// 6864).
fn clamp_reader_scroll(state: &mut AppState) {
    if !matches!(state.active_route(), Some(Route::Message(_))) {
        return;
    }
    let (viewport, total) = reader_scroll_bounds(state);
    let max = (total - viewport).max(0);
    state.reader_scroll = (state.reader_scroll as i64).clamp(0, max) as usize;
}

/// Shift `list_scroll` so the selection stays on screen. Row geometry comes
/// from the same pure layout math the renderer uses (`messages_visible`,
/// view-mode aware: comfortable rows cost two lines), keeping the
/// bookkeeping consistent with what is actually drawn.
fn keep_selection_visible(state: &mut AppState) {
    let visible = crate::ui::layout::messages_visible(state.size, state.view_mode).max(1);
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
            // Ticket haeb: the cached page for this exact query renders
            // immediately — cold contexts only (empty list), so a move
            // re-sync can never resurrect moved rows from the cache; the
            // fresh load starts right after and replaces the page.
            let mut effects = Vec::new();
            if state.messages.items.is_empty()
                && let Some(cache) = &state.page_cache
                && let Some(page) = cache.load(
                    &request.mailbox_id,
                    Some(&request.query),
                    request.offset,
                    request.limit,
                )
            {
                effects.extend(apply_page(state, page));
            }
            effects.push(state.operations.start(OperationKind::Search(request)));
            effects
        }
        Some(route) => match route.mailbox_id().cloned() {
            Some(mailbox_id) => {
                let request = PageRequest {
                    mailbox_id,
                    offset,
                    limit: state.messages.limit.max(1),
                };
                // Ticket haeb: cached summaries render instantly in cold
                // contexts (empty list: startup, mailbox switch) — a move
                // re-sync has a non-empty list, so it can never resurrect
                // moved rows from the cache. The fresh load starts right
                // after and its result overwrites this page.
                let mut effects = Vec::new();
                if state.messages.items.is_empty()
                    && let Some(cache) = &state.page_cache
                    && let Some(page) =
                        cache.load(&request.mailbox_id, None, request.offset, request.limit)
                {
                    effects.extend(apply_page(state, page));
                }
                effects.push(state.operations.start(OperationKind::LoadPage(request)));
                effects
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
    // Ticket haeb: in cold contexts (empty list — startup, mailbox switch)
    // the cached page renders instantly; the fresh load below still runs
    // and its result overwrites the cache and the page.
    let mut effects = Vec::new();
    if state.messages.items.is_empty()
        && let Some(cache) = &state.page_cache
        && let Some(page) = cache.load(&request.mailbox_id, None, request.offset, request.limit)
    {
        effects.extend(apply_page(state, page));
    }
    effects.push(state.operations.start(OperationKind::LoadPage(request)));
    effects
}

fn activate(state: &mut AppState) -> Vec<Effect> {
    match state.focus {
        Focus::Sidebar => match state.selected_mailbox().cloned() {
            Some(mailbox) => switch_mailbox(state, &mailbox.id),
            None => Vec::new(),
        },
        Focus::MessageList => match state.selected_message().cloned() {
            Some(summary) => open_selected(state, summary),
            None => Vec::new(),
        },
        Focus::Composer => activate_composer(state),
        // The dialogs intercept Enter themselves; unreachable in practice.
        // Enter on the reader presses the selected attachment chip (plan
        // §15, ticket 61qx): `o`'s open path — a session save is reused,
        // otherwise the chip is saved first and the opener chains on the
        // confirmed path. Inert without a loaded message or attachments.
        Focus::Reader => open_selected_attachment(state),
        Focus::Dialog
        | Focus::ThemePicker
        | Focus::SearchField
        | Focus::ErrorModal
        | Focus::Wizard => {
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

/// Open the attachment file chooser (plan §15, ticket 95x0): a
/// `ratatui_explorer` listing over the user's home directory. The
/// listing itself is backend work — the dialog opens immediately in its
/// pending state and the explorer arrives with the result. No-op without
/// a composer.
fn open_attachment_dialog(state: &mut AppState) -> Vec<Effect> {
    if state.composer.is_none() {
        return Vec::new();
    }
    state.overlay = Some(Overlay::AttachmentExplorer(Box::new(
        AttachmentFileDialog {
            explorer: None,
            listing: true,
            error: None,
            previous_focus: state.focus,
        },
    )));
    state.focus = Focus::Dialog;
    vec![
        state
            .operations
            .start(OperationKind::ListAttachmentFiles { path: None }),
    ]
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

/// Enter (or a second click) on a selected list row. A draft in the
/// mailbox with the `Drafts` role reopens in the composer instead of the
/// reader — the one context where Enter composes (plan §14: reopening
/// continues the draft); every other message opens the reader.
fn open_selected(state: &mut AppState, summary: crate::domain::MessageSummary) -> Vec<Effect> {
    if state.active_mailbox_role() == Some(MailboxRole::Drafts) {
        return open_draft_message(state, summary);
    }
    open_message(state, summary)
}

/// Whether the in-memory draft and the listed message are the same draft:
/// matched on the stable RFC `Message-ID` (envelope listings carry bare
/// ids, snapshots the bracketed form) or on the backend id of the last
/// confirmed remote copy.
fn is_same_draft(draft: &crate::domain::Draft, summary: &crate::domain::MessageSummary) -> bool {
    let ids_match = match (&draft.message_id, &summary.message_id) {
        (Some(draft_id), Some(summary_id)) => {
            bare_message_id(draft_id) == bare_message_id(summary_id)
        }
        _ => false,
    };
    ids_match || draft.remote_id.as_ref() == Some(&summary.id)
}

/// Whether a composer screen is open right now (the one-composer rule,
/// plan §14): a draft the user is editing — or parked with Tab while
/// picking a mailbox — is never clobbered. A draft left behind (`Esc`
/// saved it) stays in `AppState.composer` for the Drafts list, but it no
/// longer blocks a reply/forward (ticket 61qx): the seed replaces it, and
/// its possibly in-flight save result is dropped by the local_id currency
/// check in `save_draft_completed`.
fn composer_open(state: &AppState) -> bool {
    matches!(state.active_route(), Some(Route::Composer))
}

/// Secure the draft parked in the composer slot before it is replaced
/// (ticket sazy): a forced save of its newest revision, unless a save of
/// exactly that revision is already in flight (it carries the same
/// content). Drafts are durable in the Drafts mailbox, so a secured parked
/// draft never blocks another one from opening.
enum Secured {
    /// Nothing to do: no parked draft, it is clean, or its newest
    /// revision is already being pushed.
    Nothing,
    /// The save effect to launch before replacing the slot.
    Save(Effect),
    /// The parked draft is dirty but cannot be secured yet (no clock —
    /// before the first tick): the caller must not replace it.
    Cannot,
}

fn secure_parked_draft(state: &mut AppState) -> Secured {
    let Some(composer) = state.composer.as_ref() else {
        return Secured::Nothing;
    };
    if !composer.draft.is_dirty() {
        return Secured::Nothing;
    }
    let in_flight = composer.draft.local_id.as_ref().is_some_and(|local_id| {
        state
            .operations
            .is_saving_draft(local_id, composer.draft.revision)
    });
    if in_flight {
        return Secured::Nothing;
    }
    match draft_save_effect(state) {
        Some(effect) => Secured::Save(effect),
        None => Secured::Cannot,
    }
}

/// Enter on a message in the Drafts mailbox (plan §14). The in-memory
/// draft (left open earlier, or restored at startup) is the newest known
/// state of itself — reopening continues it without a backend round-trip.
/// A *different* parked draft never blocks (ticket sazy): it is secured
/// with a forced save and the fetched draft takes the composer slot when
/// it lands. The only remaining refusal is a dirty draft with no clock
/// yet — before the first tick — where the save cannot be stamped.
fn open_draft_message(state: &mut AppState, summary: crate::domain::MessageSummary) -> Vec<Effect> {
    let same = state
        .composer
        .as_ref()
        .is_some_and(|c| is_same_draft(&c.draft, &summary));
    if same {
        open_composer_screen(state);
        return Vec::new();
    }
    let mut effects = Vec::new();
    match secure_parked_draft(state) {
        Secured::Save(effect) => effects.push(effect),
        Secured::Cannot => {
            state.set_status("Still starting up — try again in a moment");
            return Vec::new();
        }
        Secured::Nothing => {}
    }
    let locator = summary.into_locator();
    state.set_status("Opening draft…");
    effects.push(state.operations.start(OperationKind::OpenDraft(locator)));
    effects
}

/// Apply a fetched draft copy (the `OpenDraft` result): turn it into a
/// composer draft and open the composer, replacing the draft parked in
/// the slot. The parked copy is secured the same way the intent secured
/// it (`secure_parked_draft`) — a draft composed or restored while the
/// fetch ran is saved to the Drafts mailbox, never clobbered — and its
/// late save result drops on the local_id mismatch (the fetched copy
/// carries a fresh identity). Currency checks upstream: the drafts
/// mailbox must still be displayed. The selected row must still be this
/// draft, so the draft that opens is the one the cursor is on (a moved
/// selection drops the stale fetch).
fn draft_message_loaded(state: &mut AppState, message: crate::domain::Message) -> Vec<Effect> {
    let selected = state
        .selected_message()
        .is_some_and(|row| row.id == message.id);
    if !selected {
        tracing::debug!(
            id = %message.id.0,
            "draft fetch dropped: the selection moved on"
        );
        return Vec::new();
    }
    let mut effects = Vec::new();
    match secure_parked_draft(state) {
        Secured::Save(effect) => effects.push(effect),
        Secured::Cannot => {
            tracing::debug!(
                id = %message.id.0,
                "draft fetch dropped: the parked draft cannot be secured yet"
            );
            return Vec::new();
        }
        Secured::Nothing => {}
    }
    let draft = crate::domain::draft_from_message(&message);
    install_composer_draft(state, draft, "Draft opened");
    effects
}

/// Open the selected message: push the reader route, snapshot the summary,
/// and fetch the full message. The list page, selection, and scroll stay
/// untouched so `Esc` restores them exactly (plan §19 Phase 4).
fn open_message(state: &mut AppState, summary: crate::domain::MessageSummary) -> Vec<Effect> {
    let mailbox_id = summary.mailbox_id.clone();
    let locator = summary.into_locator();
    state.routes.push(Route::Message(MessageRoute {
        mailbox_id,
        summary,
    }));
    state.focus = Focus::Reader;
    state.open_message = Loadable::Loading;
    state.reader_scroll = 0;
    state.reader_attachment = None;
    // Ticket haeb: a previously viewed message renders instantly from the
    // cache; the fresh load still runs and replaces it (so read/unread
    // state and any remote changes converge).
    if let Some(cache) = &state.page_cache
        && let Some(message) = cache.load_message(&locator.mailbox, &locator.id.0)
    {
        state.open_message = Loadable::Loaded(message);
    }
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

// ── External editor (plan §14, Phase 11) ─────────────────────────────────

/// Ctrl+E in the composer: save the draft first (plan §14 step 1), mark
/// the composer as externally edited so background autosave stays quiet
/// while the editor owns the file (step 5), and emit the effect the main
/// loop runs synchronously — the terminal must be suspended from the thread
/// that owns it (steps 2–7). The editor edits the body only.
fn edit_externally(state: &mut AppState) -> Vec<Effect> {
    if state.focus != Focus::Composer || state.composer.is_none() {
        return Vec::new();
    }
    let Some(program) = state.editor_command.clone() else {
        // Builtin editor configured: nothing external to run.
        return Vec::new();
    };
    let body = state
        .composer
        .as_ref()
        .map(|composer| composer.body.lines().join("\n"))
        .unwrap_or_default();
    // Step 1 (save first) — forced save when the draft has unsaved edits;
    // a clean draft is already saved, so the editor opens immediately.
    let mut effects = draft_save_effect(state).into_iter().collect::<Vec<_>>();
    if let Some(composer) = state.composer.as_mut() {
        composer.external_editing = true;
    }
    effects.push(
        state
            .operations
            .start(OperationKind::EditExternally { program, body }),
    );
    effects
}

/// The external editor exited (plan §14 steps 6–8): import the edited text,
/// mark the draft dirty, and save exactly once. An editor failure imports
/// nothing — the pre-launch save already put the draft somewhere safe —
/// and surfaces a sanitized status. Terminal restoration happened in the
/// runtime before this runs, success or failure (step 7).
fn editor_finished(
    state: &mut AppState,
    id: OperationId,
    result: Result<String, String>,
) -> Vec<Effect> {
    state.operations.finish(id);
    let should_save = match result {
        Ok(content) => {
            let now = state.clock;
            let changed = state
                .composer
                .as_mut()
                .map(|composer| {
                    composer.external_editing = false;
                    composer.import_body(&content, now)
                })
                .unwrap_or(false);
            if changed {
                // Steps 7/8: the import is one edit — mark dirty and save
                // once (the forced save, not the autosave debounce).
                state.set_status("Imported from external editor");
                true
            } else {
                state.set_status("External editor: no changes");
                false
            }
        }
        Err(detail) => {
            if let Some(composer) = state.composer.as_mut() {
                composer.external_editing = false;
            }
            state.set_status(format!("External editor failed: {detail}"));
            false
        }
    };
    if should_save {
        draft_save_effect(state).into_iter().collect()
    } else {
        Vec::new()
    }
}

/// Open the composer with a blank new email (the `c` key and the sidebar
/// Compose button, plan §19 Phase 6, ticket v5x8). A draft left open
/// earlier stays preserved in `AppState.composer` — its forced save on
/// leave keeps the Drafts-mailbox copy current — but composing again
/// always starts clean: the saved draft is reopened explicitly from the
/// Drafts list. Already composing is a no-op; only one composer exists
/// at a time.
fn open_composer(state: &mut AppState) -> Vec<Effect> {
    if matches!(state.active_route(), Some(Route::Composer)) {
        return Vec::new();
    }
    state.composer = Some(ComposerState::new());
    open_composer_screen(state);
    Vec::new()
}

/// Push the composer route and hand it focus, showing whatever draft
/// `AppState.composer` holds (a fresh blank one, a seeded reply, or a
/// draft reopened from the Drafts list).
fn open_composer_screen(state: &mut AppState) {
    if !matches!(state.active_route(), Some(Route::Composer)) {
        state.routes.push(Route::Composer);
    }
    state.focus = Focus::Composer;
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
/// silent discard. The draft data stays in `AppState.composer` so the
/// save can complete and the Drafts list can reopen it without a fetch;
/// `c` itself starts a fresh blank draft (ticket v5x8). A save of the
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
    // Phase 11.5: the external editor owns the body file — no background
    // autosave is promised while it runs (plan §14 step 5).
    if composer.external_editing {
        return Vec::new();
    }
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
    // The selection set belongs to the previous mailbox's list (ticket
    // p0s3): ids from another folder must never leak into bulk operations
    // here.
    state.selected.clear();
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
    // With nothing to cancel or close, an active selection is the next
    // thing Esc releases (ticket p0s3) before it goes back or quits.
    if state.selection_active() && !matches!(state.focus, Focus::Reader | Focus::Composer) {
        state.selected.clear();
        state.set_status("Selection cleared");
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
/// were, with no reload. The search field clears with it (ticket 32b3):
/// leaving the results means the query is done, so `/` opens an empty
/// field for the next search.
fn leave_search(state: &mut AppState) {
    if matches!(state.active_route(), Some(Route::Search(_))) {
        state.routes.pop();
        if let Some(stash) = state.search_return.take() {
            state.messages = stash.page;
            state.selection = stash.selection;
            state.list_scroll = stash.scroll;
        }
        state.focus = Focus::MessageList;
        state.search_query.clear();
        // Search selections do not follow the user back to the mailbox
        // list (ticket p0s3): the visible set changed entirely.
        state.selected.clear();
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
        let mut effects = Vec::new();
        // Ticket haeb: the cached mailbox listing renders the sidebar
        // (and, through the page cache, the first page) instantly; the
        // fresh listing still loads and replaces it.
        if let Some(cache) = &state.page_cache
            && let Some(mailboxes) = cache.load_mailboxes()
            && !mailboxes.is_empty()
        {
            state.mailboxes = Loadable::Loaded(mailboxes.clone());
            effects = apply_mailbox_listing(state, mailboxes);
        }
        effects.push(state.operations.start(OperationKind::LoadMailboxes));
        return effects;
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
    // The timer never interrupts interactive work: it stands down while a
    // modal is open, the composer is on screen, or a *foreground* operation
    // is in flight (ticket wxtx: silent background preview fetches do not
    // block it), and retries on the next tick once they clear. The first
    // tick arms the timer (the reducer has no clock before then).
    fn conflicts(state: &AppState) -> bool {
        state.overlay.is_some()
            || matches!(state.active_route(), Some(Route::Composer))
            || state.operations.has_foreground()
    }
    if conflicts(state) {
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
