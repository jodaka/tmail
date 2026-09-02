//! The action vocabulary (plan §9). All input sources — keyboard, mouse
//! (Phase 10), timers, backend completions — map into these actions.
//!
//! Deviations from plan §9, all intentional and temporary:
//! - `BackendCompleted(OperationResult)` stays deferred to Phase 3 with the
//!   operation registry it depends on. Until then, Phase 2 carries backend
//!   results in the two dedicated `…Loaded` actions below; errors are plain
//!   strings (typed operation results arrive with the registry).
//! - `SearchChar`/`SearchBackspace` are added because editing the search
//!   field needs character-level actions, which §9's enum lacks.

use crate::domain::{Mailbox, MessageSummary, Page, PageRequest};

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
    /// Backend result: the mailbox listing (startup load; refresh comes
    /// with Phase 9).
    MailboxesLoaded(Result<Vec<Mailbox>, String>),
    /// Backend result: one page of message summaries for `request`.
    PageLoaded {
        request: PageRequest,
        result: Result<Page<MessageSummary>, String>,
    },
    Tick,
    Resize {
        width: u16,
        height: u16,
    },
    Quit,
}
