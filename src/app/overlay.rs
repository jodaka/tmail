//! Overlays (plan §9): modals rendered above every screen.
//!
//! Phase 3 added the error overlay; the composer adds the confirm-discard
//! dialog (plan §14). The attachment-path dialog arrives with Phase 8.

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
