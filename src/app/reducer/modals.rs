//! Modal overlay handling (plan §9/§12): the help popup, the error
//! Retry/Dismiss modal, the discard and send confirmations, the attachment
//! dialog, and the theme picker — every overlay intercepts all input while
//! open.
use super::composer_flow::{close_composer_route, switch_mailbox};
use crate::app::action::{Action, AttachmentBrowse};
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::{
    DraftRemovalReason, OperationFailure, OperationKind, OperationOrigin, OperationResult,
};
use crate::app::overlay::{
    AccountSwitcherDialog, ConfirmButton, ErrorDialog, HelpDialog, MailboxesDialog, ModalButton,
    Overlay, SwitchConfirmDialog, ThemePickerDialog,
};
use crate::app::state::{AppState, Loadable};

// ── Modal overlays (plan §9/§12) ─────────────────────────────────────────

/// Status-message timeout (ticket h1d7): with `[tmail].status_timeout > 0`
/// a message set at `status.shown_at` clears when the window elapses. The
/// renderer fades it toward the background over the closing 0.3 s of the
/// window (`statusbar::render`); here it only leaves the state. The
/// default `0` keeps a message until the next one replaces it.
pub(crate) fn clear_expired_status(
    state: &mut AppState,
    now: chrono::DateTime<chrono::FixedOffset>,
) {
    if state.settings.status_timeout_seconds == 0 || state.session.status.message.is_none() {
        return;
    }
    let Some(shown_at) = state.session.status.shown_at else {
        // A message armed before the first tick (example: the cached
        // mailbox listing completes inside the first batch, before the
        // 250 ms heartbeat ever injected a clock) has no timer yet. The
        // first tick to see it starts the window; the message can only
        // stay ~250 ms longer than the configured timeout.
        state.session.status.shown_at = Some(now);
        return;
    };
    let elapsed = (now - shown_at).num_seconds().max(0) as u64;
    if elapsed >= state.settings.status_timeout_seconds {
        state.session.status.message = None;
        state.session.status.shown_at = None;
    }
}

/// Handle `action` while any modal is open. Returns `None` when no modal
/// is open (the caller falls through to normal handling). The attachment
/// dialog likewise falls through for `BackendCompleted`: the pending
/// validation result must land while the dialog is up. The error modal
/// (issue 8859) falls through too — successes apply behind it and
/// background failures land in the status line — and queues a *foreground*
/// failure into the open dialog instead of dropping it.
pub(crate) fn modal_reduce(state: &mut AppState, action: &Action) -> Option<Vec<Effect>> {
    match state.session.overlay {
        Some(Overlay::Error(_)) => match action {
            Action::BackendCompleted(result) => {
                // Results are not input (issue 8859): a success applies
                // normally behind the modal, and a background failure
                // keeps its status-line routing. Only a foreground
                // failure queues into the open dialog.
                if result.outcome.is_ok()
                    || state
                        .session
                        .operations
                        .get(result.id)
                        .is_none_or(|op| op.origin == OperationOrigin::Background)
                {
                    None
                } else {
                    Some(queue_error_modal_failure(state, result))
                }
            }
            _ => Some(error_modal_reduce(state, action)),
        },
        Some(Overlay::ConfirmDiscard(_)) => Some(discard_modal_reduce(state, action)),
        Some(Overlay::AttachmentExplorer(_)) => match action {
            Action::BackendCompleted(_) => None,
            _ => Some(attachment_dialog_reduce(state, action)),
        },
        Some(Overlay::ThemePicker(_)) => Some(theme_picker_reduce(state, action)),
        Some(Overlay::AccountSwitcher(_)) => match action {
            // Results must land while the switcher is open (a refresh or
            // preview in flight); the popup otherwise swallows everything.
            Action::BackendCompleted(_) => None,
            _ => Some(account_switcher_reduce(state, action)),
        },
        Some(Overlay::SwitchConfirm(_)) => match action {
            Action::BackendCompleted(_) => None,
            _ => Some(switch_confirm_reduce(state, action)),
        },
        Some(Overlay::Help(_)) => match action {
            // Results must land while help is open (a send/autosave in
            // flight); the popup otherwise swallows everything.
            Action::BackendCompleted(_) => None,
            _ => Some(help_modal_reduce(state, action)),
        },
        Some(Overlay::Mailboxes(_)) => match action {
            // Results must land while the popup is open (a mailbox
            // listing refresh in flight); the popup otherwise swallows
            // everything.
            Action::BackendCompleted(_) => None,
            _ => Some(mailboxes_modal_reduce(state, action)),
        },
        None => None,
    }
}

/// Help handling (user request): the popup is inert except for its own
/// close keys — Esc, `?`, or `Ctrl+h` (whichever the user pressed) all
/// close, restoring the focus underneath.
pub(crate) fn help_modal_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Some(Overlay::Help(dialog)) = state.session.overlay.take() else {
        return Vec::new();
    };
    match action {
        Action::BackOrCancel | Action::OpenHelp | Action::Activate => {
            state.session.focus = dialog.previous_focus;
        }
        _ => {
            // Swallowed: restore the dialog (everything else is inert).
            state.session.overlay = Some(Overlay::Help(dialog));
        }
    }
    Vec::new()
}

/// The shortcuts popup (user request). Only when no wizard owns the keys
/// — wizard input is entirely its own — and not when another overlay is
/// open (the modal path above keeps the existing one).
pub(crate) fn open_help(state: &mut AppState) -> Vec<Effect> {
    if state.session.wizard.is_some()
        || !matches!(
            state.session.focus,
            Focus::MessageList
                | Focus::Reader
                | Focus::Sidebar
                | Focus::MailboxTitle
                | Focus::SearchField
                | Focus::Composer
        )
    {
        return Vec::new();
    }
    state.session.overlay = Some(Overlay::Help(HelpDialog {
        previous_focus: state.session.focus,
    }));
    state.session.focus = Focus::Help;
    Vec::new()
}

/// The Mailboxes popup (issue brnw), compact mode's sidebar stand-in:
/// Enter on the focused mailbox-title button (or a click on it) opens it
/// over the mailbox screen. The cursor starts on the mailbox the session
/// displays — Enter on it closes without switching, the way the account
/// switcher starts on the driving account. Never over the wizard (every
/// key there is wizard-owned); while a mailbox listing is still loading
/// the popup opens anyway and shows the same pending states the sidebar
/// does.
pub(crate) fn open_mailboxes_popup(state: &mut AppState) -> Vec<Effect> {
    if state.session.wizard.is_some() {
        return Vec::new();
    }
    let active = state.active_route().and_then(|r| r.mailbox_id()).cloned();
    let len = state.mailboxes.as_loaded().map(Vec::len).unwrap_or(0);
    let cursor = state
        .mailboxes
        .as_loaded()
        .and_then(|mailboxes| {
            mailboxes
                .iter()
                .position(|mailbox| Some(&mailbox.id) == active.as_ref())
        })
        .unwrap_or(0)
        .min(len.saturating_sub(1));
    // The scroll window opens with the cursor row on screen: a long
    // mailbox list must not hide the mailbox the user is on.
    let visible = crate::view::overlay::mailboxes_visible_rows(state.session.size).max(1);
    let scroll = if cursor >= visible {
        (cursor + 1 - visible).min(crate::view::overlay::mailboxes_max_scroll(
            len,
            state.session.size,
        ))
    } else {
        0
    };
    state.session.overlay = Some(Overlay::Mailboxes(MailboxesDialog {
        cursor,
        scroll,
        previous_focus: state.session.focus,
    }));
    state.session.focus = Focus::Mailboxes;
    Vec::new()
}

/// Mailboxes popup handling (issue brnw). Up/Down move the cursor with
/// the same clamped (non-wrapping) movement the account switcher uses.
/// Enter on the displayed mailbox closes; Enter on another mailbox
/// closes and switches (the sidebar stand-in's whole point). Esc closes
/// without switching. Everything else is swallowed while the popup is
/// open.
pub(crate) fn mailboxes_modal_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    // Viewport math comes from the renderer's layout, so the clamp the
    // reducer computes always matches what is drawn.
    let len = state.mailboxes.as_loaded().map(Vec::len).unwrap_or(0);
    let visible = crate::view::overlay::mailboxes_visible_rows(state.session.size).max(1);
    let max_scroll = crate::view::overlay::mailboxes_max_scroll(len, state.session.size);
    let Some(Overlay::Mailboxes(dialog)) = state.session.overlay.as_mut() else {
        return Vec::new();
    };
    // The listing may have refreshed while the popup was open: keep the
    // cursor and scroll window inside it.
    dialog.cursor = dialog.cursor.min(len.saturating_sub(1));
    dialog.scroll = dialog.scroll.min(max_scroll);
    match action {
        Action::MoveUp | Action::MoveDown => {
            let delta = if matches!(action, Action::MoveUp) {
                -1
            } else {
                1
            };
            if len == 0 {
                return Vec::new();
            }
            let next = (dialog.cursor as i64 + delta).clamp(0, len as i64 - 1) as usize;
            dialog.cursor = next;
            dialog.scroll = if next < dialog.scroll {
                next
            } else if next >= dialog.scroll + visible {
                (next + 1 - visible).min(max_scroll)
            } else {
                dialog.scroll
            };
        }
        Action::BackOrCancel => {
            let focus = dialog.previous_focus;
            state.session.overlay = None;
            state.session.focus = focus;
        }
        Action::Activate => {
            let cursor = dialog.cursor;
            let focus = dialog.previous_focus;
            let target = state
                .mailboxes
                .as_loaded()
                .and_then(|mailboxes| mailboxes.get(cursor))
                .map(|mailbox| mailbox.id.clone());
            return match target {
                // Enter on the displayed mailbox: close, nothing to do.
                Some(id) if state.active_route().and_then(|r| r.mailbox_id()) == Some(&id) => {
                    state.session.overlay = None;
                    state.session.focus = focus;
                    Vec::new()
                }
                // Enter on another mailbox: close, then switch. The
                // switch replaces any open route (reader, search) — the
                // mailbox route becomes the root, exactly what the
                // sidebar's Enter does.
                Some(id) => {
                    state.session.overlay = None;
                    state.session.focus = focus;
                    switch_mailbox(state, &id)
                }
                // Empty or still-loading listing: the arrows and Enter
                // are inert (the popup shows the pending state).
                None => Vec::new(),
            };
        }
        // Everything else is swallowed while the popup is open.
        _ => {}
    }
    Vec::new()
}

/// Error modal handling (plan §12).
pub(crate) fn error_modal_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    // Scroll budget from the same layout math the renderer uses, so the
    // reducer and the drawn modal always agree on the clamp.
    let (max_scroll, viewport) = match &state.session.overlay {
        Some(Overlay::Error(dialog)) => {
            let layout = crate::view::overlay::error_modal_layout(
                state.session.size,
                dialog.code,
                dialog.ambiguous,
                dialog.more_failures,
            );
            (
                crate::view::overlay::error_modal_max_scroll(
                    &dialog.detail,
                    dialog.code,
                    dialog.ambiguous,
                    dialog.more_failures,
                    state.session.size,
                ),
                layout.viewport_lines,
            )
        }
        // Only the error modal scrolls; this arm is unreachable in
        // practice (discard_modal_reduce handles its own overlay).
        _ => (0, 1),
    };
    let Some(Overlay::Error(dialog)) = state.session.overlay.as_mut() else {
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
            state.session.overlay = None;
            state.session.focus = focus;
        }
        Action::Activate | Action::RetryError => {
            // `RetryError` always retries; `Enter` acts on the focused
            // button (plan §12: Retry replays the intent, Dismiss closes).
            let wants_retry =
                matches!(action, Action::RetryError) || dialog.button == ModalButton::Retry;
            let retry = dialog.retry.clone().filter(|_| wants_retry);
            let focus = dialog.previous_focus;
            if let Some(spec) = retry {
                state.session.overlay = None;
                state.session.focus = focus;
                // Retrying replays the equivalent typed intent under a
                // *new* operation id (plan §12; acceptance: new id). The
                // loading placeholders reset so no stale failure text
                // lingers while the replay runs.
                match &spec.kind {
                    OperationKind::LoadMailboxes => state.mailboxes = Loadable::Loading,
                    OperationKind::LoadMessage(_) => {
                        state.open_message = Loadable::Loading;
                        state.reader_scroll = 0;
                        state.reader_focus = None;
                    }
                    OperationKind::SaveDraft { draft } => {
                        // A draft-save retry replays the *intent* ("save
                        // this draft"), not the failed revision: retry
                        // materializes the newest revision so retrying
                        // after further edits never pushes stale content.
                        if let Some(composer) = state.session.composer.as_mut()
                            && composer.draft.local_id.as_ref() == Some(&draft.local_id)
                            && let Some(now) = state.session.clock
                        {
                            let fresh = composer.draft.start_save(now);
                            return vec![state.session.operations.start(
                                OperationKind::SaveDraft {
                                    draft: Box::new(fresh),
                                },
                            )];
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
                        if let Some(composer) = state.session.composer.as_mut() {
                            composer.sending = true;
                        }
                    }
                    _ => {}
                }
                return vec![state.session.operations.start(spec.kind)];
            }
            if !wants_retry {
                // Enter on Dismiss: close without new work.
                state.session.overlay = None;
                state.session.focus = focus;
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
pub(crate) fn discard_modal_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Some(Overlay::ConfirmDiscard(dialog)) = state.session.overlay.as_mut() else {
        return Vec::new();
    };
    match action {
        Action::FocusNext => dialog.button = dialog.button.next(),
        Action::FocusPrevious => dialog.button = dialog.button.previous(),
        // Esc keeps the draft: closing the dialog is not a discard.
        Action::BackOrCancel => {
            let focus = dialog.previous_focus;
            state.session.overlay = None;
            state.session.focus = focus;
        }
        Action::Activate => {
            let confirm = dialog.button == ConfirmButton::Discard;
            let focus = dialog.previous_focus;
            let dialog = state.session.overlay.take().expect("dialog open");
            let Overlay::ConfirmDiscard(dialog) = dialog else {
                unreachable!("checked above")
            };
            state.session.focus = focus;
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
pub(crate) fn confirm_discard(
    state: &mut AppState,
    draft: crate::domain::DraftSnapshot,
) -> Vec<Effect> {
    state.session.composer = None;
    close_composer_route(state);
    state.session.operations.cancel_draft_saves(&draft.local_id);
    state.set_status("Draft discarded");
    vec![state.session.operations.start(OperationKind::DeleteDraft {
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
pub(crate) fn attachment_dialog_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Some(Overlay::AttachmentExplorer(dialog)) = state.session.overlay.as_mut() else {
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
                            .session
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
                                .session
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
                                .session
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
            state.session.overlay = None;
            state.session.focus = focus;
            Vec::new()
        }
        // Everything else is swallowed while the dialog is open.
        _ => Vec::new(),
    }
}

/// Open the theme picker (ticket k5ba). The cursor starts on the active
/// palette — Enter is a no-op until the user moves, and the preview begins
/// from where the user already is. No-op without a theme list.
pub(crate) fn open_theme_picker(state: &mut AppState) -> Vec<Effect> {
    if state.settings.themes.is_empty() {
        return Vec::new();
    }
    let cursor = state
        .settings
        .theme_index
        .min(state.settings.themes.len() - 1);
    // The scroll window opens with the cursor row on screen: a long theme
    // list must not hide the palette the user is currently on.
    let visible = crate::view::overlay::picker_visible_rows(state.session.size).max(1);
    let scroll = if cursor >= visible {
        (cursor + 1 - visible).min(crate::view::overlay::picker_max_scroll(
            state.settings.themes.len(),
            state.session.size,
        ))
    } else {
        0
    };
    state.session.overlay = Some(Overlay::ThemePicker(ThemePickerDialog {
        original: state.settings.theme_index,
        cursor,
        scroll,
        previous_focus: state.session.focus,
    }));
    state.session.focus = Focus::ThemePicker;
    Vec::new()
}

/// Theme picker handling (ticket k5ba). Up/Down move the cursor and apply
/// the highlighted palette at once — the preview the ticket asks for —
/// with the same clamped (non-wrapping) movement the message list uses.
/// Enter keeps the previewed palette; Esc restores the palette the picker
/// opened with. Everything else is swallowed while the picker is open.
pub(crate) fn theme_picker_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    // Viewport math comes from the renderer's layout, so the clamp the
    // reducer computes always matches what is drawn.
    let len = state.settings.themes.len();
    let visible = crate::view::overlay::picker_visible_rows(state.session.size).max(1);
    let max_scroll = crate::view::overlay::picker_max_scroll(len, state.session.size);
    let Some(Overlay::ThemePicker(dialog)) = state.session.overlay.as_mut() else {
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
            state.settings.theme_index = next;
        }
        Action::BackOrCancel => {
            // Esc never changes the theme: the preview is undone by
            // restoring the index the picker opened with.
            state.settings.theme_index = dialog.original.min(len.saturating_sub(1));
            let focus = dialog.previous_focus;
            state.session.overlay = None;
            state.session.focus = focus;
        }
        Action::Activate => {
            let name = state
                .settings
                .themes
                .get(state.settings.theme_index)
                .map(|(name, _)| name.as_str())
                .unwrap_or("default")
                .to_owned();
            let focus = dialog.previous_focus;
            state.session.overlay = None;
            state.session.focus = focus;
            state.set_status(format!("Theme: {name}"));
        }
        _ => {}
    }
    Vec::new()
}

/// Open the account switcher (ticket c0n0). The cursor starts on the
/// account the session drives; with no account resolved (the
/// multi-account-without-default case) it starts at the top. Inert without
/// an account list — a config with no file or no `[accounts]` has nothing
/// to switch to — and never over the wizard (every key there is
/// wizard-owned; the guard mirrors [`open_help`]).
pub(crate) fn open_account_switcher(state: &mut AppState) -> Vec<Effect> {
    if state.session.wizard.is_some() || state.settings.accounts.is_empty() {
        return Vec::new();
    }
    let cursor = state
        .settings
        .accounts
        .iter()
        .position(|account| Some(account.name.as_str()) == state.settings.account_name.as_deref())
        .unwrap_or(0);
    // The scroll window opens with the cursor row on screen: a long
    // account list must not hide the account the user is on.
    let visible = crate::view::overlay::switcher_visible_rows(state.session.size).max(1);
    let scroll = if cursor >= visible {
        (cursor + 1 - visible).min(crate::view::overlay::switcher_max_scroll(
            state.settings.accounts.len(),
            state.session.size,
        ))
    } else {
        0
    };
    state.session.overlay = Some(Overlay::AccountSwitcher(AccountSwitcherDialog {
        cursor,
        scroll,
        previous_focus: state.session.focus,
    }));
    state.session.focus = Focus::AccountSwitcher;
    Vec::new()
}

/// Account switcher handling (ticket c0n0). Up/Down move the cursor with
/// the same clamped (non-wrapping) movement the theme picker uses. Enter
/// on the current account closes; Enter on another account switches at
/// once when nothing would be lost (no in-flight operations, clean
/// composer), and otherwise opens the confirmation listing exactly what
/// would be cancelled and dropped. Esc closes without switching.
/// Everything else is swallowed while the switcher is open.
pub(crate) fn account_switcher_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    // Viewport math comes from the renderer's layout, so the clamp the
    // reducer computes always matches what is drawn.
    let len = state.settings.accounts.len();
    let visible = crate::view::overlay::switcher_visible_rows(state.session.size).max(1);
    let max_scroll = crate::view::overlay::switcher_max_scroll(len, state.session.size);
    let Some(Overlay::AccountSwitcher(dialog)) = state.session.overlay.as_mut() else {
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
        }
        Action::BackOrCancel => {
            let focus = dialog.previous_focus;
            state.session.overlay = None;
            state.session.focus = focus;
        }
        Action::Activate => {
            let cursor = dialog.cursor;
            let focus = dialog.previous_focus;
            let target = state
                .settings
                .accounts
                .get(cursor)
                .map(|account| account.name.clone());
            match target {
                // Enter on the current account: close, nothing to do.
                Some(name) if Some(name.as_str()) == state.settings.account_name.as_deref() => {
                    state.session.overlay = None;
                    state.session.focus = focus;
                }
                Some(name) => {
                    let operations = state.session.operations.in_flight_summaries();
                    let unsaved_draft = state
                        .session
                        .composer
                        .as_ref()
                        .is_some_and(|composer| composer.draft.is_dirty());
                    if operations.is_empty() && !unsaved_draft {
                        // Nothing would be lost: switch at once.
                        state.session.overlay = None;
                        proceed_with_account_switch(state, name);
                    } else {
                        // Confirm first: the dialog lists exactly what
                        // confirming cancels and drops.
                        state.session.overlay = Some(Overlay::SwitchConfirm(SwitchConfirmDialog {
                            target: name,
                            operations,
                            unsaved_draft,
                            button: ConfirmButton::Keep,
                            previous_focus: focus,
                        }));
                    }
                }
                // Empty list (unreachable: the opener is inert then).
                None => {}
            }
        }
        _ => {}
    }
    Vec::new()
}

/// The switch confirmation dialog handling (ticket c0n0). Enter runs the
/// focused button: `Switch anyway` proceeds with the switch, `Keep
/// working` aborts it entirely. Esc aborts the same way — closing the
/// dialog is never a switch.
pub(crate) fn switch_confirm_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Some(Overlay::SwitchConfirm(dialog)) = state.session.overlay.as_mut() else {
        return Vec::new();
    };
    match action {
        Action::FocusNext => dialog.button = dialog.button.next(),
        Action::FocusPrevious => dialog.button = dialog.button.previous(),
        Action::BackOrCancel => {
            let focus = dialog.previous_focus;
            state.session.overlay = None;
            state.session.focus = focus;
        }
        Action::Activate => {
            let confirm = dialog.button == ConfirmButton::Discard;
            let focus = dialog.previous_focus;
            let target = dialog.target.clone();
            state.session.overlay = None;
            if confirm {
                proceed_with_account_switch(state, target);
            } else {
                state.session.focus = focus;
            }
        }
        // Error-modal-only actions do nothing here.
        _ => {}
    }
    Vec::new()
}

/// Confirmed account switch (ticket c0n0): every in-flight operation is
/// cancelled — tokens fire so the backends kill their children, entries
/// clear so a late result can never re-apply — the composer and every
/// overlay close, and the session exits with the switch intent. The
/// runtime rebuilds the whole session against the target account,
/// including a fresh config read: exactly what restarting tmail with that
/// account would produce.
pub(crate) fn proceed_with_account_switch(state: &mut AppState, target: String) -> Vec<Effect> {
    let cancelled = state.session.operations.cancel_all();
    tracing::info!(target = %target, cancelled, "account switch confirmed");
    state.session.overlay = None;
    state.session.composer = None;
    state.session.switch_requested = Some(target);
    Vec::new()
}

/// Open the Retry/Dismiss modal for a failed operation (plan §12).
pub(crate) fn open_error_modal(state: &mut AppState, failure: OperationFailure) -> Vec<Effect> {
    tracing::warn!(code = ?failure.code, detail = %failure.detail, "operation failed");
    state.session.overlay = Some(Overlay::Error(ErrorDialog {
        code: failure.code,
        detail: failure.detail.clone(),
        retry: failure.retry.clone(),
        ambiguous: failure.ambiguous,
        more_failures: 0,
        scroll: 0,
        button: ModalButton::Dismiss,
        previous_focus: state.session.focus,
    }));
    state.session.focus = Focus::ErrorModal;
    state.set_status("Operation failed");
    Vec::new()
}

/// A foreground failure completing while the error modal is already open
/// (issue 8859): the visible failure stays put — replacing it would
/// swallow what the user is reading — and the new failure queues into
/// the dialog as the "and N more failed" line, its detail logged at
/// WARN. The registry entry is consumed here exactly like
/// `backend_completed` would, so the queued operation can never
/// re-apply and cannot leak.
fn queue_error_modal_failure(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    if state.session.operations.finish(result.id).is_none() {
        tracing::debug!(id = %result.id, "dropping result for unknown or cancelled operation");
        return Vec::new();
    }
    let Err(failure) = &result.outcome else {
        // Only failures are routed here; an Ok outcome fell through.
        return Vec::new();
    };
    tracing::warn!(
        id = %result.id,
        code = ?failure.code,
        detail = %failure.detail,
        "operation failed while an error modal was already open"
    );
    if let Some(Overlay::Error(dialog)) = state.session.overlay.as_mut() {
        dialog.more_failures = dialog.more_failures.saturating_add(1);
    }
    Vec::new()
}
