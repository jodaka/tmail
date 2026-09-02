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
use crate::domain::{Mailbox, MessageSummary, Page, PageRequest};

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
}
