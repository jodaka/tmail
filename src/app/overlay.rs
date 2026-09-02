//! Overlays (plan §9): modals rendered above every screen.
//!
//! Phase 3 adds the error overlay only; the confirm-discard and
//! attachment-path dialogs arrive with their phases and extend this enum.

use crate::app::focus::Focus;
use crate::app::operation::RetrySpec;

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
}
