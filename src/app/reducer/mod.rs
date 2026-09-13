//! Deterministic, I/O-free reducer (plan §5/§9).
//!
//! `reduce` is the only writer of `AppState`. State transitions that need
//! backend data start an operation in the registry and return an
//! [`Effect`] for the runtime to execute; results come back as
//! `Action::BackendCompleted` and apply only while their operation is still
//! registered, so stale, cancelled, or superseded results never win
//! (plan §11). Reducers never perform I/O themselves.

use crate::app::action::{Action, ClickTarget, ComposerEdit};
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::app::wizard::wizard_reduce;

/// Apply `action` to `state`, returning backend work to spawn. Consumes
/// the action by value (ticket sakb): payload-carrying variants
/// (`BackendCompleted`, `EditorFinished`, `Tick`) move their payloads into
/// the handlers instead of every handler deep-cloning them. Never
/// performs I/O, never panics on odd input.
pub fn reduce(state: &mut AppState, action: Action) -> Vec<Effect> {
    if let Some(effects) = intercept(state, &action) {
        return effects;
    }
    let effects = dispatch(state, action);
    sync_body_caret(state);
    effects
}

/// The input consumers that see every action before the main dispatch:
/// the wizard (owns the whole screen, ADR 0003), a modal's clickable
/// buttons, and the modal overlay itself (plan §9). `Some` means the
/// action was consumed there.
fn intercept(state: &mut AppState, action: &Action) -> Option<Vec<Effect>> {
    // The account configuration wizard (ADR 0003) owns the whole screen
    // while active: it intercepts every action — mailbox navigation,
    // warmup refreshes, modals — before anything else can react. Clock,
    // size, quit, and backend results stay live.
    if state.session.wizard.is_some() {
        return Some(wizard_reduce(state, action));
    }
    // A mouse click on a modal button must reach the modal path before the
    // interception swallows everything else (plan §9: a modal intercepts
    // all input; Phase 10.2 adds its buttons as clickable).
    if let Action::Click(target) = action
        && state.session.overlay.is_some()
    {
        return Some(match target {
            ClickTarget::ErrorButton(button) => click_error_button(state, *button),
            ClickTarget::ConfirmButton(button) => click_confirm_button(state, *button),
            // Clicks "through" the modal do nothing, like any other input.
            _ => Vec::new(),
        });
    }
    // A modal overlay intercepts all input while it is open (plan §9).
    let effects = modal_reduce(state, action)?;
    // The theme picker previews palettes from inside the modal path;
    // the body caret must track it like every other action.
    sync_body_caret(state);
    Some(effects)
}

/// The main action dispatch, after the interceptors had their chance.
fn dispatch(state: &mut AppState, action: Action) -> Vec<Effect> {
    match action {
        Action::MoveUp => move_selection(state, -1),
        Action::MoveDown => move_selection(state, 1),
        Action::PagePrevious => page_step(state, -1),
        Action::PageNext => page_step(state, 1),
        Action::Activate => activate(state),
        Action::BackOrCancel => back_or_cancel(state),
        Action::FocusNext => focus_step(state, 1),
        Action::FocusPrevious => focus_step(state, -1),
        Action::OpenSearch => open_search(state),
        Action::SearchEdit(edit) => {
            search_edit(state, &edit);
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
        Action::OpenHelp => open_help(state),
        Action::LoadDrafts => load_drafts(state),
        Action::ComposerEdit(edit) => composer_edit(state, &edit),
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
        Action::EditorFinished { id, result } => editor_finished(state, id, result),
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
        Action::Click(target) => click(state, target),
        Action::BackendCompleted(result) => backend_completed(state, result),
        Action::Refresh => refresh(state),
        Action::ToggleMouseCapture => toggle_mouse_capture(state),
        Action::OpenThemePicker => open_theme_picker(state),
        Action::Tick { now } => tick(state, *now),
        Action::SetTerminalFocus(focused) => {
            state.session.terminal_focused = focused;
            Vec::new()
        }
        Action::Resize { width, height } => resize(state, width, height),
        Action::Quit => {
            state.session.quit_requested = true;
            Vec::new()
        }
    }
}

/// `/` focuses the search field on the mailbox screen; while composing the
/// key is composed text (plan §10: shortcuts never fire in fields). The
/// sidebar focus inside compose mode is gated the same way: submitting
/// cannot run over the composer, so the field must not strand focus there.
fn open_search(state: &mut AppState) -> Vec<Effect> {
    if state.session.focus != Focus::Composer
        && !matches!(state.active_route(), Some(Route::Composer))
    {
        state.session.focus = Focus::SearchField;
    }
    Vec::new()
}

/// Composer edits (plan §14): editing targets the focused composer control;
/// without a composer open (or without its focus) the edit is inert.
/// Content edits sync the draft and re-arm autosave; caret moves leave the
/// revision untouched. Undo/redo (ticket kfmt) rewrite the body content
/// only when the history applies a step, so they sync conditionally. While
/// a send of this draft is in flight (Phase 7.6) all editing is frozen:
/// the bytes on the wire must stay what the user saw.
fn composer_edit(state: &mut AppState, edit: &ComposerEdit) -> Vec<Effect> {
    if state.session.focus == Focus::Composer
        && let Some(composer) = state.session.composer.as_mut()
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
                composer.sync_draft(state.session.clock);
            }
        }
    }
    Vec::new()
}

/// `m` flips mouse capture. Data only: the runtime applies the capture mode
/// to the terminal (the reducer stays I/O-free). The status line names the
/// mode so the state change is never silent.
fn toggle_mouse_capture(state: &mut AppState) -> Vec<Effect> {
    state.settings.mouse_capture = !state.settings.mouse_capture;
    state.set_status(if state.settings.mouse_capture {
        "Mouse capture on — hold Shift to select text · m toggles"
    } else {
        "Mouse capture off — text selection available · m toggles"
    });
    Vec::new()
}

/// Keep the body editor's caret and selection styles in step (tickets
/// tz12, kfmt): the accent caret block while the body is focused — the
/// same one the single-line fields draw — and no caret otherwise. Runs
/// after every action so focus cycling, selection changes, and live
/// theme switches (theme picker preview) can never show a stale style.
fn sync_body_caret(state: &mut AppState) {
    let theme = state.active_theme();
    if let Some(composer) = state.session.composer.as_mut() {
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
            state.session.quit_requested = true;
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn tick(state: &mut AppState, now: chrono::DateTime<chrono::FixedOffset>) -> Vec<Effect> {
    state.session.ticks += 1;
    state.session.clock = Some(now);
    clear_expired_status(state, now);
    let mut effects = autosave_tick(state, now);
    effects.extend(auto_refresh_tick(state, now));
    effects
}

fn resize(state: &mut AppState, width: u16, height: u16) -> Vec<Effect> {
    state.session.size = (width, height);
    // Ticket kjfq: auto-sized pages track the terminal — the limit
    // is however many rows fit the list right now. A change re-loads
    // the visible page in the background (supersession collapses a
    // resize storm; the newest request wins) so the list refills.
    let mut effects = Vec::new();
    if state.settings.page_size_auto {
        let visible =
            crate::view::layout::messages_visible(state.session.size, state.settings.view_mode)
                .max(1);
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

mod actions;
mod composer_flow;
mod message_results;
mod modals;
mod mouse;
mod navigation;
mod results;
mod search_refresh;
mod seeding;
mod send;

pub(crate) use actions::*;
pub(crate) use composer_flow::*;
pub(crate) use modals::*;
pub(crate) use mouse::*;
pub(crate) use navigation::*;
pub(crate) use results::*;
pub(crate) use search_refresh::*;
pub(crate) use seeding::*;
pub(crate) use send::*;

#[cfg(test)]
mod test_prelude {
    // The test suite addresses these through the reducer's namespace, as
    // it did when the reducer was one file (`use super::*` in
    // reducer_tests/mod.rs re-exports everything from here).
    pub(crate) use crate::app::operation::{
        OperationFailure, OperationKind, OperationOrigin, OperationOutcome, OperationResult,
    };
    pub(crate) use crate::app::state::{Loadable, ReaderFocus};
    pub(crate) use crate::domain::MessageLocator;
}

#[cfg(test)]
#[path = "../reducer_tests/mod.rs"]
mod tests;
