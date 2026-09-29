//! Draft operations (plan §14, ADR 0002): autosave/push, journal
//! restore, discard/sent cleanup, and delivery. One family of the
//! operation vocabulary — see [`super::OperationKind`].

use std::sync::Arc;

use crate::domain::{DraftSnapshot, OutboundMessage};

/// Why a draft is being removed (Phase 7.6): it selects the failure UX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftRemovalReason {
    /// Confirmed discard (plan §14): failures open Retry/Dismiss.
    Discard,
    /// Tmail-send cleanup (ADR 0002 consequences): best-effort, logged only.
    Sent,
}

/// The draft/send operation to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftOperation {
    /// Persist one draft revision (plan §14, ADR 0002): journal record +
    /// remote add-then-delete replacement. The snapshot freezes the exact
    /// revision saved, so a stale success can be detected and re-saved.
    /// Shared (`Arc`, issue 6m97): boxed out of the enum's inline size,
    /// and refcounted so the registry's kind/retry clones and every
    /// launch copy the payload by reference count — never a deep body
    /// copy. The backend call consumes its own owned copy.
    SaveDraft { draft: Arc<DraftSnapshot> },
    /// Restore drafts from the crash-safe journal at startup (ADR 0002
    /// §D.5).
    LoadDrafts,
    /// Delete a draft everywhere (journal + remote). `Discard` follows a
    /// confirmed discard (plan §14) and opens the modal on failure;
    /// `Sent` is the tmail-send cleanup (ADR 0002: best-effort — delivery
    /// is already confirmed, so a failure must never claim one). Shared
    /// snapshot, as with `SaveDraft`.
    DeleteDraft {
        draft: Arc<DraftSnapshot>,
        reason: DraftRemovalReason,
    },
    /// Deliver one serialized message through the Himalaya stdin contract
    /// (plan §14, Phase 7). The message is frozen at send time; retries
    /// replay the exact bytes under a new operation id. Shared, as with
    /// the draft payloads.
    Send { message: Arc<OutboundMessage> },
}

impl DraftOperation {
    /// Human-readable label for the status bar and error modal.
    pub(crate) fn summary(&self) -> &'static str {
        match self {
            DraftOperation::SaveDraft { .. } => "Saving draft",
            DraftOperation::LoadDrafts => "Restoring drafts",
            DraftOperation::DeleteDraft { reason, .. } => match reason {
                DraftRemovalReason::Discard => "Discarding draft",
                DraftRemovalReason::Sent => "Cleaning up sent draft",
            },
            DraftOperation::Send { .. } => "Sending message",
        }
    }

    /// Whether `newer` supersedes `older` (plan §11): a newer save of the
    /// same draft supersedes an older one — only the newest revision may
    /// ever be pushed (plan §14 coalescing, ADR 0002 §D.2). Restores and
    /// discards likewise supersede their own kind.
    pub(crate) fn supersedes(newer: &DraftOperation, older: &DraftOperation) -> bool {
        match (newer, older) {
            (
                DraftOperation::SaveDraft { draft: newer },
                DraftOperation::SaveDraft { draft: older },
            ) => newer.local_id == older.local_id,
            (DraftOperation::LoadDrafts, DraftOperation::LoadDrafts) => true,
            (
                DraftOperation::DeleteDraft { draft: newer, .. },
                DraftOperation::DeleteDraft { draft: older, .. },
            ) => newer.local_id == older.local_id,
            _ => false,
        }
    }

    /// No draft operation coalesces duplicates (review: sends are excluded
    /// on purpose — every delivery attempt must run to its classified
    /// outcome; see [`DraftOperation::supersedes`] for the save rules).
    pub(crate) fn duplicates_of(&self, _older: &DraftOperation) -> bool {
        false
    }

    /// Whether `Esc` may cancel the operation (plan §11): a send is never
    /// cancellable — killing himalaya mid-DATA leaves the delivery state
    /// unknown, exactly the ambiguity plan §12 forbids hiding. Journal
    /// saves/restores/cleanups are cancellable: the journal is the source
    /// of truth and a retry replays them.
    pub(crate) fn is_cancellable(&self) -> bool {
        !matches!(self, DraftOperation::Send { .. })
    }

    /// Whether the dispatch runs a mail-backend child process (issue
    /// 1v38): the journal restore is local today but conservatively
    /// counts as backend work, as before the family split.
    pub(crate) fn uses_backend_process(&self) -> bool {
        true
    }
}
