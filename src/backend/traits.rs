//! The `MailBackend` contract (plan §8): an async, mockable interface around
//! Post's needs rather than a mirror of every Himalaya command.
//!
//! Phase 2 implements the mailbox and message-list operations; read, flags,
//! search, send, and drafts extend the trait in their own phases. Cancellation
//! tokens, `RequestContext`, and `OperationId` arrive with the Phase 3
//! operation manager, at which point the method set grows accordingly.

use async_trait::async_trait;
use thiserror::Error;

use crate::domain::{Mailbox, MessageSummary, Page, PageRequest};

/// Typed backend failures (plan §12: typed library errors at the backend,
/// `anyhow` context at the app layer). Details are safe for display and
/// logging: they never contain credentials — only exit codes and child
/// process diagnostics.
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
    async fn list_mailboxes(&self) -> BackendResult<Vec<Mailbox>>;

    /// One explicit page of message summaries for a mailbox (plan §7/§16).
    async fn list_messages(&self, page: PageRequest) -> BackendResult<Page<MessageSummary>>;
}
