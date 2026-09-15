//! Mouse clicks (plan §10, Phase 10.1/10.2): hit-tested against exactly
//! what the frame showed — the reducer sees typed `ClickTarget`s, never
//! coordinates.
use super::actions::open_reader_link;
use super::composer_flow::switch_mailbox;
use super::navigation::{activate_composer, keep_selection_visible, open_selected};
use super::reduce;
use crate::app::action::{Action, BulkOp, ClickTarget};
use crate::app::composer::ComposerField;
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::overlay::{ConfirmButton, ModalButton, Overlay};
use crate::app::route::Route;
use crate::app::state::{AppState, ReaderFocus};

// ── Mouse clicks (plan §10, Phase 10.1/10.2) ─────────────────────────────

/// Apply a mouse click recorded during render. Every arm below mirrors the
/// keyboard path — selecting, opening, focusing, or pressing — so the
/// mouse never unlocks behavior the keyboard cannot reach (plan §10). A
/// click on an already-selected row activates it, matching the
/// select-then-Enter rhythm the keyboard uses. Modal-button clicks are
/// routed before the modal interception (see `reduce`); this function only
/// ever runs with no overlay open.
pub(crate) fn click(state: &mut AppState, target: ClickTarget) -> Vec<Effect> {
    match target {
        ClickTarget::ComposeButton => reduce(state, Action::Compose),
        ClickTarget::Mailbox(index) => click_mailbox(state, index),
        ClickTarget::MailboxTitle => click_mailbox_title(state),
        ClickTarget::SearchField => reduce(state, Action::OpenSearch),
        ClickTarget::MessageRow(index) => click_message_row(state, index),
        ClickTarget::ReaderLink(index) => click_reader_link(state, index),
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
                state.session.focus = Focus::MessageList;
            }
            match op {
                BulkOp::Trash => reduce(state, Action::Trash),
                BulkOp::Archive => reduce(state, Action::Archive),
                BulkOp::MarkRead => reduce(state, Action::MarkRead),
                BulkOp::MarkUnread => reduce(state, Action::MarkUnread),
            }
        }
    }
}

/// Click a modal button: focus it (the same state Tab produces), then run
/// the same path Enter would (plan §12).
pub(crate) fn click_error_button(state: &mut AppState, button: ModalButton) -> Vec<Effect> {
    if let Some(Overlay::Error(dialog)) = state.session.overlay.as_mut() {
        // A dimmed Retry button (failure without a retry intent) does
        // nothing, like a Retry that only `Tab` could reach.
        if button == ModalButton::Retry && dialog.retry.is_none() {
            return Vec::new();
        }
        dialog.button = button;
    }
    match button {
        ModalButton::Retry => reduce(state, Action::RetryError),
        ModalButton::Dismiss => reduce(state, Action::DismissError),
    }
}

/// Click a confirm button (plan §14): focus, then the same confirm/keep
/// path Enter takes. One path for both confirm dialogs — the composer's
/// discard dialog and the account switch's confirmation (ticket c0n0) —
/// the open one receives the button.
pub(crate) fn click_confirm_button(state: &mut AppState, button: ConfirmButton) -> Vec<Effect> {
    match state.session.overlay.as_mut() {
        Some(Overlay::ConfirmDiscard(dialog)) => dialog.button = button,
        Some(Overlay::SwitchConfirm(dialog)) => dialog.button = button,
        _ => {}
    }
    reduce(state, Action::Activate)
}

/// Click a sidebar mailbox row: focus follows the click, a new row is
/// selected (the arrows' job), an already-selected row activates (Enter's
/// job, plan §10).
pub(crate) fn click_mailbox(state: &mut AppState, index: usize) -> Vec<Effect> {
    let count = state.mailboxes.as_loaded().map(Vec::len).unwrap_or(0);
    if index >= count {
        return Vec::new();
    }
    state.session.focus = Focus::Sidebar;
    if index == state.mailbox_selection {
        return match state.selected_mailbox().cloned() {
            Some(mailbox) => switch_mailbox(state, &mailbox.id),
            None => Vec::new(),
        };
    }
    state.mailbox_selection = index;
    Vec::new()
}

/// Click the compact mode's mailbox-title button (issue brnw): focus it,
/// then open the Mailboxes popup — the same path Enter takes. A button
/// click presses it (the Compose button's convention), it does not
/// select-and-wait. The renderer records the target only in compact
/// mode, so a stale full-mode frame never triggers it.
pub(crate) fn click_mailbox_title(state: &mut AppState) -> Vec<Effect> {
    state.session.focus = Focus::MailboxTitle;
    reduce(state, Action::Activate)
}

/// Click a message row: focus the list, select the row, and open it when
/// it was already the selection (arrows + Enter equivalent).
pub(crate) fn click_message_row(state: &mut AppState, index: usize) -> Vec<Effect> {
    if index >= state.messages.items.len() {
        return Vec::new();
    }
    state.session.focus = Focus::MessageList;
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

/// Click an attachment chip in the reader (plan §15): focus it; clicking
/// the already-focused chip opens it (`o`'s job — reuse a saved path or
/// save first, then open).
pub(crate) fn click_reader_attachment(state: &mut AppState, index: usize) -> Vec<Effect> {
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
    state.session.focus = Focus::Reader;
    if state.reader_focus == Some(ReaderFocus::Attachment(index)) {
        return reduce(state, Action::OpenAttachment);
    }
    state.reader_focus = Some(ReaderFocus::Attachment(index));
    Vec::new()
}

/// Click a link in the reader body (ticket hc9n): focus it; clicking the
/// already-focused link opens it in the platform browser (Enter's job,
/// mirroring the attachment chips).
pub(crate) fn click_reader_link(state: &mut AppState, index: usize) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Message(_))) {
        return Vec::new();
    }
    let width = crate::view::layout::reader_width(state.session.size);
    if index >= crate::app::reader::link_count(state, width) {
        return Vec::new();
    }
    state.session.focus = Focus::Reader;
    if state.reader_focus == Some(ReaderFocus::Link(index)) {
        return open_reader_link(state, width, index);
    }
    state.reader_focus = Some(ReaderFocus::Link(index));
    Vec::new()
}

/// Click a composer control (plan §10): focus it; buttons activate, like
/// Tab-then-Enter would. Text fields place the caret the way `Tab` does
/// (at the end of the field; the body keeps its caret).
pub(crate) fn click_composer_field(state: &mut AppState, field: ComposerField) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Composer)) {
        return Vec::new();
    }
    let focused = state
        .session
        .composer
        .as_mut()
        .is_some_and(|composer| composer.focus_field(field));
    if !focused {
        return Vec::new();
    }
    state.session.focus = Focus::Composer;
    if field.accepts_text() {
        Vec::new()
    } else {
        activate_composer(state)
    }
}
