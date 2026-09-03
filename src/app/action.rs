//! The action vocabulary (plan §9). All input sources — keyboard, mouse
//! (Phase 10), timers, backend completions — map into these actions.
//!
//! Deviations from plan §9, all intentional and temporary:
//! - `SearchChar`/`SearchBackspace` are collapsed into `SearchEdit` because
//!   editing the search field needs character-level actions.
//! - Results return as `BackendCompleted(OperationResult)` per plan §9;
//!   `OperationResult` carries the `OperationId` the registry allocated,
//!   and its failure payloads are already sanitized (plan §12).

use crate::app::composer::ComposerField;
use crate::app::operation::OperationResult;
use crate::app::overlay::{ConfirmButton, ModalButton};
use chrono::{DateTime, FixedOffset};

/// Character-level edit of the search field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEdit {
    Char(char),
    Backspace,
}

/// Character-level edit or caret move inside the focused composer control
/// (Phase 6). The reducer routes it by [`ComposerField`](crate::app::composer::ComposerField).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerEdit {
    Char(char),
    Backspace,
    Delete,
    /// Enter in the body inserts a newline (plan §10); elsewhere Enter
    /// activates, which the reducer routes separately.
    Newline,
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
}
impl ComposerEdit {
    /// Whether this edit changes draft content (caret moves do not): only
    /// content edits bump the draft revision and re-arm autosave.
    pub fn is_content_edit(&self) -> bool {
        matches!(
            self,
            ComposerEdit::Char(_)
                | ComposerEdit::Backspace
                | ComposerEdit::Delete
                | ComposerEdit::Newline
        )
    }
}

/// Character-level edit of a modal text field (Phase 8: the attachment
/// path-entry dialog, plan §15). Single-line, so no vertical movement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogEdit {
    Char(char),
    Backspace,
    Delete,
    CursorLeft,
    CursorRight,
}

/// What a mouse click landed on (plan §10, Phase 10.1). Recorded during
/// render as widget rectangles; the mouse layer translates a click into
/// `Action::Click(target)` and the reducer — the only state writer —
/// applies it with full context. Every target has a keyboard equivalent:
/// rows via arrows + Enter, fields via Tab, buttons via Tab + Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickTarget {
    /// The sidebar's Compose affordance (equivalent: `c`).
    ComposeButton,
    /// A sidebar mailbox row (equivalent: arrows + Enter).
    Mailbox(usize),
    /// The topbar search field (equivalent: `/`).
    SearchField,
    /// A message-list row (equivalent: arrows + Enter).
    MessageRow(usize),
    /// An attachment chip in the reader (equivalent: Tab + `d`/`o`).
    ReaderAttachment(usize),
    /// A composer control (equivalent: Tab; Enter activates buttons).
    ComposerField(ComposerField),
    /// Retry/Dismiss buttons of the error modal (equivalent: Tab + Enter).
    ErrorButton(ModalButton),
    /// Discard/Keep buttons of the confirm-discard dialog.
    ConfirmButton(ConfirmButton),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    MoveUp,
    MoveDown,
    PagePrevious,
    PageNext,
    Activate,
    BackOrCancel,
    FocusNext,
    FocusPrevious,
    OpenSearch,
    SearchEdit(SearchEdit),
    SubmitSearch,
    /// Composer text editing / caret movement (Phase 6).
    ComposerEdit(ComposerEdit),
    /// Modal text-field editing (Phase 8: attachment path dialog).
    DialogEdit(DialogEdit),
    /// Restore drafts from the crash-safe journal (startup, Phase 6).
    LoadDrafts,
    Compose,
    Reply,
    ReplyAll,
    Forward,
    Archive,
    Trash,
    ToggleStar,
    MarkUnread,
    Refresh,
    Send,
    /// Save the selected reader attachment to the downloads directory
    /// (plan §15, Phase 8.4).
    SaveAttachment,
    /// Open the selected reader attachment with the platform handler
    /// (plan §15, Phase 8.5): reuse a path saved this session or save
    /// first, then open.
    OpenAttachment,
    LeaveComposer,
    DiscardDraft,
    RetryError,
    DismissError,
    /// A mouse click on a recorded widget rectangle (Phase 10.2). The
    /// reducer applies it with full state context — selecting a row,
    /// opening an already-selected one, focusing a composer control, or
    /// pressing a modal button — so mouse behavior is exactly the keyboard
    /// vocabulary, never mouse-only behavior (plan §10).
    Click(ClickTarget),
    /// Backend result for one operation. Results for unknown, cancelled,
    /// or superseded ids are rejected by the reducer (plan §11).
    BackendCompleted(OperationResult),
    /// Periodic tick. `now` is the injected wall clock: the reducer never
    /// reads a clock itself (plan §20 determinism), using it for the
    /// autosave debounce and saved-at stamps (Phase 6). Boxed to keep the
    /// enum small next to the payload-carrying result variant.
    Tick {
        now: Box<DateTime<FixedOffset>>,
    },
    Resize {
        width: u16,
        height: u16,
    },
    Quit,
}
