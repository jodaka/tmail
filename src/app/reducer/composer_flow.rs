//! External editor (plan §14, Phase 11) and composer lifecycle: open,
//! park, secure, leave, autosave, and the crash-safe draft plumbing.
use super::navigation::{close_reader, keep_mailbox_visible, request_page};
use super::search_refresh::leave_search;
use crate::app::action::SearchEdit;
use crate::app::composer::ComposerState;
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::{OperationId, OperationKind};
use crate::app::overlay::{ConfirmButton, DiscardDialog, Overlay};
use crate::app::route::{MailboxRoute, Route};
use crate::app::state::{AppState, Loadable};
use crate::domain::{DraftSaveState, MailboxId, Page};

// ── External editor (plan §14, Phase 11) ─────────────────────────────────

/// Pop the composer route and hand focus back to the message list
/// (the shared tail of send, discard, and leave — one composer rule:
/// the screen always leaves the same way).
pub(crate) fn close_composer_route(state: &mut AppState) {
    if matches!(state.active_route(), Some(Route::Composer)) {
        state.session.routes.pop();
        state.session.focus = Focus::MessageList;
    }
}

/// Ctrl+E in the composer: save the draft first (plan §14 step 1), mark
/// the composer as externally edited so background autosave stays quiet
/// while the editor owns the file (step 5), and emit the effect the main
/// loop runs synchronously — the terminal must be suspended from the thread
/// that owns it (steps 2–7). The editor edits the body only.
pub(crate) fn edit_externally(state: &mut AppState) -> Vec<Effect> {
    if state.session.focus != Focus::Composer || state.session.composer.is_none() {
        return Vec::new();
    }
    let Some(program) = state.settings.editor_command.clone() else {
        // Builtin editor configured: nothing external to run.
        return Vec::new();
    };
    let body = state
        .session
        .composer
        .as_ref()
        .map(|composer| composer.body.lines().join("\n"))
        .unwrap_or_default();
    // Step 1 (save first) — forced save when the draft has unsaved edits;
    // a clean draft is already saved, so the editor opens immediately.
    let mut effects = draft_save_effect(state).into_iter().collect::<Vec<_>>();
    if let Some(composer) = state.session.composer.as_mut() {
        composer.external_editing = true;
    }
    effects.push(
        state
            .session
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
pub(crate) fn editor_finished(
    state: &mut AppState,
    id: OperationId,
    result: Result<String, String>,
) -> Vec<Effect> {
    state.session.operations.finish(id);
    let should_save = match result {
        Ok(content) => {
            let now = state.session.clock;
            let changed = state
                .session
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
            if let Some(composer) = state.session.composer.as_mut() {
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
/// earlier stays preserved in `AppState.session.composer` — its forced save on
/// leave keeps the Drafts-mailbox copy current — but composing again
/// always starts clean: the saved draft is reopened explicitly from the
/// Drafts list. Already composing is a no-op; only one composer exists
/// at a time.
pub(crate) fn open_composer(state: &mut AppState) -> Vec<Effect> {
    if matches!(state.active_route(), Some(Route::Composer)) {
        return Vec::new();
    }
    state.session.composer = Some(ComposerState::new());
    open_composer_screen(state);
    Vec::new()
}

/// Push the composer route and hand it focus, showing whatever draft
/// `AppState.session.composer` holds (a fresh blank one, a seeded reply, or a
/// draft reopened from the Drafts list).
pub(crate) fn open_composer_screen(state: &mut AppState) {
    if !matches!(state.active_route(), Some(Route::Composer)) {
        state.session.routes.push(Route::Composer);
    }
    state.session.focus = Focus::Composer;
}

/// Start the journal restore (plan §19 Phase 6 crash/restart acceptance).
/// Dispatched once at startup by the runtime.
pub(crate) fn load_drafts(state: &mut AppState) -> Vec<Effect> {
    if state.session.operations.is_loading_drafts() {
        return Vec::new();
    }
    vec![state.session.operations.start(OperationKind::LoadDrafts)]
}

/// Open the confirm-discard dialog (plan §14: discard only after explicit
/// confirmation). A no-op without a draft.
pub(crate) fn open_discard_confirm(state: &mut AppState) -> Vec<Effect> {
    let Some(composer) = state.session.composer.as_ref() else {
        return Vec::new();
    };
    let draft = composer.draft.snapshot();
    state.session.overlay = Some(Overlay::ConfirmDiscard(DiscardDialog {
        draft,
        button: ConfirmButton::Keep,
        previous_focus: state.session.focus,
    }));
    state.session.focus = Focus::ErrorModal;
    Vec::new()
}

/// Leave the composer (plan §14): pop the route, return to the prior
/// route, and FORCE a save of any unsaved revision — no debounce, never a
/// silent discard. The draft data stays in `AppState.session.composer` so the
/// save can complete and the Drafts list can reopen it without a fetch;
/// `c` itself starts a fresh blank draft (ticket v5x8). A save of the
/// current revision already in flight is not duplicated; an in-flight save
/// of an older revision is superseded by the forced one.
pub(crate) fn leave_composer(state: &mut AppState) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Composer)) {
        return Vec::new();
    }
    state.session.routes.pop();
    state.session.focus = Focus::MessageList;
    let Some(composer) = state.session.composer.as_ref() else {
        return Vec::new();
    };
    let dirty = composer.draft.is_dirty();
    let in_flight = composer.draft.local_id.as_ref().is_some_and(|local_id| {
        state
            .session
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
pub(crate) fn autosave_tick(
    state: &mut AppState,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Vec<Effect> {
    let Some(composer) = state.session.composer.as_mut() else {
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
    if !composer
        .draft
        .autosave_due(now, state.settings.autosave_delay_ms)
    {
        return Vec::new();
    }
    tracing::debug!(
        revision = composer.draft.revision,
        "debounce elapsed; saving draft"
    );
    let snapshot = composer.draft.start_save(now);
    vec![state.session.operations.start(OperationKind::SaveDraft {
        draft: Box::new(snapshot),
    })]
}

/// Start a save of the current draft revision right away (forced saves:
/// follow-up after a stale success, retries). `None` when there is no
/// composer, no clock yet, or nothing unsaved to write.
pub(crate) fn draft_save_effect(state: &mut AppState) -> Option<Effect> {
    let now = state.session.clock?;
    let composer = state.session.composer.as_mut()?;
    if !composer.draft.is_dirty() {
        return None;
    }
    let snapshot = composer.draft.start_save(now);
    Some(state.session.operations.start(OperationKind::SaveDraft {
        draft: Box::new(snapshot),
    }))
}

pub(crate) fn switch_mailbox(state: &mut AppState, mailbox_id: &MailboxId) -> Vec<Effect> {
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
    if !state.session.routes.is_empty() {
        state.session.routes.clear();
        state.session.search_return = None;
        state.open_message = Loadable::Idle;
        state.reader_scroll = 0;
        state.reader_focus = None;
    }
    state.session.routes.push(Route::Mailbox(MailboxRoute {
        mailbox_id: mailbox_id.clone(),
    }));
    state.mailbox_selection = state
        .mailboxes
        .as_loaded()
        .and_then(|ms| ms.iter().position(|m| &m.id == mailbox_id))
        .unwrap_or(0);
    // The switched mailbox (e.g. Drafts while composing) stays on screen.
    keep_mailbox_visible(state);
    state.selection = 0;
    state.list_scroll = 0;
    state.session.focus = Focus::MessageList;
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
pub(crate) fn back_or_cancel(state: &mut AppState) -> Vec<Effect> {
    if let Some(Overlay::Error(dialog)) = state.session.overlay.take() {
        state.session.focus = dialog.previous_focus;
        return Vec::new();
    }
    if state.session.focus == Focus::SearchField {
        state.session.focus = Focus::MessageList;
        return Vec::new();
    }
    if matches!(state.active_route(), Some(Route::Composer)) {
        // The composer may sit at the root (no mailbox loaded yet); Esc
        // leaves it either way, preserving the draft (plan §14).
        return leave_composer(state);
    }
    if let Some(op) = state.session.operations.cancel_foreground() {
        tracing::info!(id = %op.id, kind = ?op.kind, "cancelled foreground operation");
        state.set_status(format!("{} — cancelled", op.kind.summary()));
        return Vec::new();
    }
    // With nothing to cancel or close, an active selection is the next
    // thing Esc releases (ticket p0s3) before it goes back or quits.
    if state.selection_active() && !matches!(state.session.focus, Focus::Reader | Focus::Composer) {
        state.selected.clear();
        state.set_status("Selection cleared");
        return Vec::new();
    }
    if state.session.routes.len() > 1 {
        // Leave an open search first: its results are regenerated on
        // re-submit, the mailbox context underneath is stashed (Phase 9.1).
        if matches!(state.active_route(), Some(Route::Search(_))) {
            leave_search(state);
            return Vec::new();
        }
        // Pop the reader: the mailbox route underneath still holds the
        // exact page, selection, and scroll (same teardown as close_reader).
        close_reader(state);
        return Vec::new();
    }
    // Root route with nothing to cancel or close: exit cleanly (plan §19
    // Phase 1 acceptance: app exits with Esc/quit).
    state.session.quit_requested = true;
    Vec::new()
}

pub(crate) fn search_edit(state: &mut AppState, edit: &SearchEdit) {
    if state.session.focus != Focus::SearchField {
        return;
    }
    match edit {
        SearchEdit::Char(c) => state.session.search_query.push(*c),
        SearchEdit::Backspace => {
            state.session.search_query.pop();
        }
    }
}
