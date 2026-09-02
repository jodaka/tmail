//! The action vocabulary (plan §9). All input sources — keyboard, mouse
//! (Phase 10), timers, backend completions — map into these actions.
//!
//! Deviations from plan §9, all intentional and temporary:
//! - `SearchChar`/`SearchBackspace` are collapsed into `SearchEdit` because
//!   editing the search field needs character-level actions.
//! - Results return as `BackendCompleted(OperationResult)` per plan §9;
//!   `OperationResult` carries the `OperationId` the registry allocated,
//!   and its failure payloads are already sanitized (plan §12).

use crate::app::operation::OperationResult;

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
    LeaveComposer,
    DiscardDraft,
    RetryError,
    DismissError,
    /// Backend result for one operation. Results for unknown, cancelled,
    /// or superseded ids are rejected by the reducer (plan §11).
    BackendCompleted(OperationResult),
    Tick,
    Resize {
        width: u16,
        height: u16,
    },
    Quit,
}
