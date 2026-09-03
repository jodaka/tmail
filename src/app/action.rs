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
