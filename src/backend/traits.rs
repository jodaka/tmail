//! The `MailBackend` contract (plan §8): an async, mockable interface around
//! Post's needs rather than a mirror of every Himalaya command.
//!
//! Phase 2 implemented the mailbox and message-list operations; read, flags,
//! search, send, and drafts extend the trait in their own phases. Every
//! request carries a [`RequestContext`] with its `OperationId` and
//! cancellation token: cancelling the token terminates the child process the
//! adapter owns (Phase 3.2) and fails the request as cancelled.

use async_trait::async_trait;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::app::operation::OperationId;
use crate::domain::{
    DraftSnapshot, Mailbox, Message, MessageId, MessageLocator, MessageSummary, Page, PageRequest,
    RestoredDraft,
};

/// Per-request context handed to every backend call (plan §8).
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// The operation this request belongs to.
    pub operation: OperationId,
    /// Fired when the operation is cancelled or superseded; the adapter
    /// must stop and terminate any child process it owns.
    pub cancellation: CancellationToken,
}

/// Typed backend failures (plan §12: typed library errors at the backend,
/// `anyhow` context at the app layer). Details are safe for display and
/// logging after [`crate::app::sanitize::sanitize`] runs — they contain
/// exit codes and child diagnostics, never credentials.
#[derive(Debug, Error)]
pub enum BackendError {
    /// The child process ran but exited unsuccessfully. ADR 0001 finding 1:
    /// with `--json`, himalaya reports errors as JSON on stdout with exit
    /// code 1, so the detail prefers that message and falls back to stderr.
    #[error("himalaya command failed (exit {code:?}): {detail}")]
    Command { code: Option<i32>, detail: String },

    /// The child exited successfully but its output could not be turned into
    /// the expected shape (malformed, truncated, or non-UTF-8 JSON). Failing
    /// safely here is a Phase 2 acceptance requirement.
    #[error("himalaya returned unusable output: {0}")]
    InvalidOutput(String),

    /// A request the adapter cannot even translate (e.g. a zero page size).
    #[error("invalid backend request: {0}")]
    InvalidRequest(String),

    /// The request was cancelled before completing; the owned child was
    /// terminated. The operation manager suppresses this outcome rather
    /// than surfacing it as an error (plan §11).
    #[error("operation cancelled")]
    Cancelled,

    /// The child process could not be spawned (missing executable, I/O).
    #[error("himalaya executable could not be run: {0}")]
    Io(#[from] std::io::Error),
}

pub type BackendResult<T> = Result<T, BackendError>;

/// Post's view of the mail backend.
#[async_trait]
pub trait MailBackend: Send + Sync {
    /// All mailboxes the account exposes, with roles resolved by the
    /// adapter (ADR 0001: the UI never guesses folder names).
    async fn list_mailboxes(&self, ctx: RequestContext) -> BackendResult<Vec<Mailbox>>;

    /// One explicit page of message summaries for a mailbox (plan §7/§16).
    async fn list_messages(
        &self,
        ctx: RequestContext,
        page: PageRequest,
    ) -> BackendResult<Page<MessageSummary>>;

    /// One full message, parsed into domain types (plan §7/§13).
    async fn get_message(
        &self,
        ctx: RequestContext,
        locator: MessageLocator,
    ) -> BackendResult<Message>;

    /// Mark a message read/unread via flag operations (plan §19 Phase 4).
    /// The backend applies the change; confirmation is the success return.
    async fn set_read(
        &self,
        ctx: RequestContext,
        locator: MessageLocator,
        read: bool,
    ) -> BackendResult<()>;

    /// Star/unstar via flag operations.
    async fn set_starred(
        &self,
        ctx: RequestContext,
        locator: MessageLocator,
        starred: bool,
    ) -> BackendResult<()>;

    /// Move a message to the account's archive mailbox. The target is
    /// resolved inside the adapter (ADR 0001: the UI never guesses folder
    /// names); a missing archive mailbox is a typed request error.
    async fn archive(&self, ctx: RequestContext, locator: MessageLocator) -> BackendResult<()>;

    /// Move a message to trash (himalaya is trash-first, ADR 0001 finding
    /// 5); permanently deletes when already in trash.
    async fn trash(&self, ctx: RequestContext, locator: MessageLocator) -> BackendResult<()>;

    /// Persist one draft revision (plan §14, ADR 0002): record it in the
    /// crash-safe local journal first, then push it to the remote Drafts
    /// mailbox via add-then-delete replacement (the old remote copy is
    /// deleted only after the new id is confirmed). Returns the backend id
    /// of the confirmed remote copy.
    async fn save_draft(
        &self,
        ctx: RequestContext,
        draft: DraftSnapshot,
    ) -> BackendResult<MessageId>;

    /// Restore drafts from the crash-safe journal (ADR 0002 §D.5, plan §19
    /// Phase 6 crash/restart acceptance): purely local, so it succeeds
    /// even when the account is unreachable.
    async fn load_drafts(&self, ctx: RequestContext) -> BackendResult<Vec<RestoredDraft>>;

    /// Delete a draft everywhere (confirmed discard, plan §14): remove the
    /// journal entry, then best-effort delete every remote copy matching
    /// the draft's identity (two-phase, ADR 0002 §D.4).
    async fn delete_draft(&self, ctx: RequestContext, draft: DraftSnapshot) -> BackendResult<()>;
}
