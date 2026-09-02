//! The action vocabulary (plan §9). All input sources — keyboard, mouse
//! (Phase 10), timers, backend completions — map into these actions.
//!
//! Deviations from plan §9, both intentional and temporary:
//! - `BackendCompleted(OperationResult)` is deferred to Phase 3 together with
//!   the operation registry it depends on.
//! - `SearchChar`/`SearchBackspace` are added because editing the search
//!   field needs character-level actions, which §9's enum lacks.

/// Character-level edit of the search field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEdit {
    Char(char),
    Backspace,
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
    Tick,
    Resize { width: u16, height: u16 },
    Quit,
}
