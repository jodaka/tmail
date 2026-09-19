//! Overlays (plan §9): modals rendered above every screen.
//!
//! Phase 3 added the error overlay; the composer adds the confirm-discard
//! dialog (plan §14) and, in Phase 8, the attachment chooser (plan §15,
//! ticket 95x0: a `ratatui_explorer` listing replaces the v1 path entry).

use crate::app::focus::Focus;
use crate::app::operation::RetrySpec;
use crate::domain::DraftSnapshot;

/// Which modal button the keyboard targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalButton {
    Retry,
    Dismiss,
}

impl ModalButton {
    /// Left-to-right order: Retry, then Dismiss; Tab wraps.
    pub fn next(self) -> Self {
        match self {
            ModalButton::Retry => ModalButton::Dismiss,
            ModalButton::Dismiss => ModalButton::Retry,
        }
    }

    pub fn previous(self) -> Self {
        self.next()
    }
}

/// Which button the confirm-discard dialog targets. The safe default is
/// Keep (plan §14: discard happens only after explicit confirmation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmButton {
    Discard,
    Keep,
}

impl ConfirmButton {
    pub fn next(self) -> Self {
        match self {
            ConfirmButton::Discard => ConfirmButton::Keep,
            ConfirmButton::Keep => ConfirmButton::Discard,
        }
    }

    pub fn previous(self) -> Self {
        self.next()
    }
}

/// The scrollable Retry/Dismiss error modal (plan §12). Holds sanitized
/// detail only: secrets were redacted before a failure reached the reducer
/// (Phase 3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDialog {
    /// Himalaya exit status, when the backend ran a child process.
    pub code: Option<i32>,
    /// Sanitized causal detail; may span many lines and scrolls.
    pub detail: String,
    /// Typed intent to replay; `None` means the failure is not retryable.
    pub retry: Option<RetrySpec>,
    /// Whether the outcome is ambiguous (retrying may duplicate work, e.g.
    /// re-sending mail, plan §12).
    pub ambiguous: bool,
    /// Additional failures that completed while this modal was already
    /// open (issue 8859): queued into the dialog as an "and N more
    /// failed" line instead of being dropped or replacing the visible
    /// failure; each detail is logged at WARN.
    pub more_failures: usize,
    /// First visible detail line (scroll offset, clamped by the reducer
    /// with the same layout math the renderer uses).
    pub scroll: usize,
    /// Keyboard-targeted button.
    pub button: ModalButton,
    /// Focus to restore when the modal closes.
    pub previous_focus: Focus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Error(ErrorDialog),
    /// Composer discard confirmation (plan §14): deleting local and remote
    /// draft state happens only after explicit confirmation.
    ConfirmDiscard(DiscardDialog),
    /// Attachment file chooser (plan §15, ticket 95x0): a
    /// `ratatui_explorer` listing of a directory; the selected file is
    /// validated exactly like a typed path was. The explorer state is
    /// plain data like the composer's body textarea. Boxed: the explorer
    /// payload must not bloat the other overlay variants.
    AttachmentExplorer(Box<AttachmentFileDialog>),
    /// Theme picker (ticket k5ba): a small list of every available palette.
    /// Moving the cursor previews the highlighted theme at once; Enter
    /// keeps it and Esc restores the palette the picker opened with.
    ThemePicker(ThemePickerDialog),
    /// The runtime account switcher (ticket c0n0): a small list of every
    /// account in the config file. Enter on another account restarts the
    /// session with it; Enter on the current one closes.
    AccountSwitcher(AccountSwitcherDialog),
    /// The account-switch confirmation (ticket c0n0): listed in-flight
    /// operations would be cancelled (an in-flight send may already be
    /// delivered) and unsaved composer edits would be lost. Confirming
    /// proceeds with the switch; Esc aborts it.
    SwitchConfirm(SwitchConfirmDialog),
    /// The shortcuts help popup (user request): every active binding for
    /// the screen underneath, rendered from the keymap.
    Help(HelpDialog),
    /// The Mailboxes popup (issue brnw): the compact-mode stand-in for the
    /// sidebar — the same mailbox listing the sidebar draws, with the
    /// same switching behavior.
    Mailboxes(MailboxesDialog),
}

/// The runtime account switcher (ticket c0n0). The account list itself
/// lives in `AppState.settings.accounts` (read from the config file at
/// startup — the switch's own reload re-reads the file), so the dialog
/// only tracks where the user is inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSwitcherDialog {
    /// Cursor into `AppState.settings.accounts`.
    pub cursor: usize,
    /// First visible row when the account list outgrows the dialog.
    pub scroll: usize,
    /// Focus to restore when the popup closes.
    pub previous_focus: Focus,
}

/// The account-switch confirmation dialog (ticket c0n0). The in-flight
/// operation summaries are frozen when the dialog opens — a result
/// landing meanwhile simply shrinks what confirming will cancel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchConfirmDialog {
    /// The account the switch would select.
    pub target: String,
    /// Sorted unique summaries of the in-flight operations (`2× …`).
    pub operations: Vec<String>,
    /// Whether the composer holds unsaved changes (a switch drops the
    /// composer and its edits; saved revisions stay in the journal).
    pub unsaved_draft: bool,
    /// Keyboard-targeted button; `Keep` is the safe default.
    pub button: ConfirmButton,
    /// Focus to restore when the dialog closes.
    pub previous_focus: Focus,
}

/// The shortcuts help dialog (user request). The bindings themselves live
/// in the keymap; the help renders live (`KeyMap::help_entries`), so a
/// custom `[tmail.keybindings]` config shows up here unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpDialog {
    /// First visible entry row (scroll offset, clamped by the reducer
    /// with the same layout math the renderer uses — ticket 6t30: a
    /// table taller than the terminal used to be silently clipped).
    pub scroll: usize,
    /// Focus to restore when the popup closes.
    pub previous_focus: Focus,
}

/// The Mailboxes popup (issue brnw), compact mode's sidebar stand-in. The
/// mailbox listing itself lives in `AppState.mailboxes` — the same data
/// the sidebar renders — so the dialog only tracks where the user is
/// inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxesDialog {
    /// Cursor into `AppState.mailboxes`.
    pub cursor: usize,
    /// First visible row when the mailbox list outgrows the popup.
    pub scroll: usize,
    /// Focus to restore when the popup closes.
    pub previous_focus: Focus,
}

/// The attachment file chooser (plan §15, ticket 95x0). A
/// `ratatui_explorer::FileExplorer` carries the directory listing and
/// selection — plain data, mutated only by the reducer, exactly like the
/// composer's body textarea. Directory changes are backend listings (the
/// reducer stays I/O-free); a selected file validates like a typed path
/// did, and a rejection keeps the chooser open with the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentFileDialog {
    /// The explorer state; `None` while the directory listing is in
    /// flight (or after a failed one — see `error`).
    pub explorer: Option<ratatui_explorer::FileExplorer>,
    /// Whether a directory listing is in flight; navigation that would
    /// change directories is frozen until it lands.
    pub listing: bool,
    /// Detail of the last failed listing or attachment validation;
    /// cleared when the next step starts.
    pub error: Option<String>,
    /// First visible line of the wrapped error detail (scroll offset,
    /// clamped by the reducer with the same layout math the renderer
    /// uses; ticket 6t30 — the detail used to lose every line after the
    /// first).
    pub error_scroll: usize,
    /// Focus to restore when the dialog closes (always the composer).
    pub previous_focus: Focus,
}

impl AttachmentFileDialog {
    /// Apply a pure selection move (no filesystem access). Inputs the
    /// explorer cannot act on are ignored.
    pub fn browse(&mut self, input: ratatui_explorer::Input) {
        if let Some(explorer) = self.explorer.as_mut() {
            // Selection movement is not supposed to touch the filesystem,
            // but an Err here is an unforeseen crate behavior — log it
            // rather than swallow it (ticket s843 review).
            if let Err(err) = explorer.handle(input) {
                tracing::warn!(error = %err, "attachment explorer refused a selection move");
            }
        }
    }

    /// The directory a `Parent`/`Open` step would list: the cwd's parent,
    /// or the selected entry's path when it is a directory.
    pub fn step_target(&self, parent: bool) -> Option<std::path::PathBuf> {
        let explorer = self.explorer.as_ref()?;
        if parent {
            explorer.cwd().parent().map(std::path::Path::to_path_buf)
        } else {
            let current = explorer.current();
            current.is_dir.then(|| current.path.clone())
        }
    }

    /// The selected file's path, when the selection is a file (Enter's
    /// submit target).
    pub fn selected_file(&self) -> Option<std::path::PathBuf> {
        let explorer = self.explorer.as_ref()?;
        let current = explorer.current();
        (!current.is_dir).then(|| current.path.clone())
    }
}

/// The composer's confirm-discard dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardDialog {
    /// The draft that would be deleted (journal entry + remote copies).
    pub draft: DraftSnapshot,
    /// Keyboard-targeted button; defaults to the safe `Keep`.
    pub button: ConfirmButton,
    /// Focus to restore when the dialog closes (always the composer).
    pub previous_focus: Focus,
}

/// The theme picker dialog (ticket k5ba). The list itself lives in
/// `AppState.settings.themes` — the same entries the renderer cycles — so the
/// dialog only tracks where the user is inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePickerDialog {
    /// Theme index the picker opened with; Esc restores it (the preview
    /// mutates the live index while navigating).
    pub original: usize,
    /// Cursor into `AppState.settings.themes`; the highlighted theme is previewed
    /// at once (`theme_index` follows the cursor).
    pub cursor: usize,
    /// First visible row when the theme list outgrows the dialog.
    pub scroll: usize,
    /// Focus to restore when the dialog closes.
    pub previous_focus: Focus,
}
