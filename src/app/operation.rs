//! Operation bookkeeping for the operation manager (plan §9/§11).
//!
//! The registry lives in [`crate::app::state::AppState`] and is written only
//! by the reducer: starting an operation allocates the next [`OperationId`],
//! registers a fresh [`Operation`] (with its own
//! [`CancellationToken`](tokio_util::sync::CancellationToken)), and
//! supersedes any in-flight operation of the same kind so a slow older
//! request can never replace a newer result (plan §11: "Ignore completion
//! for unknown, cancelled, or superseded IDs"). The runtime reads the token
//! for each effect and hands it to the backend; the child process it owns is
//! killed on cancellation (Phase 3.2).

use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::domain::{
    DraftSnapshot, Mailbox, Message, MessageId, MessageLocator, MessageSummary, OutboundMessage,
    Page, PageRequest, RestoredDraft, SendOutcome,
};

/// Opaque identifier carried by every backend request and result (plan §5:
/// "Every request and result carries an `OperationId`"). Constructed only
/// by the registry; test code may synthesize unknown ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OperationId(pub u64);

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "op-{}", self.0)
    }
}

/// The typed intent of one backend operation (plan §5: "Effects launch
/// typed backend requests").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationKind {
    /// Fetch the mailbox listing.
    LoadMailboxes,
    /// Fetch one page of message summaries.
    LoadPage(PageRequest),
    /// Fetch one full message (plan §19 Phase 4: reader).
    LoadMessage(MessageLocator),
    /// Mark a message read (`read: true`) or unread.
    SetRead { locator: MessageLocator, read: bool },
    /// Star (`starred: true`) or unstar a message.
    SetStarred {
        locator: MessageLocator,
        starred: bool,
    },
    /// Move a message to the archive mailbox (target resolved by the
    /// adapter, ADR 0001).
    Archive(MessageLocator),
    /// Move a message to trash (himalaya is trash-first).
    Trash(MessageLocator),
    /// Persist one draft revision (plan §14, ADR 0002): journal record +
    /// remote add-then-delete replacement. The snapshot freezes the exact
    /// revision saved, so a stale success can be detected and re-saved.
    /// Boxed: full draft payloads must not bloat every operation kind.
    SaveDraft { draft: Box<DraftSnapshot> },
    /// Restore drafts from the crash-safe journal at startup (ADR 0002
    /// §D.5).
    LoadDrafts,
    /// Delete a draft everywhere (journal + remote). `Discard` follows a
    /// confirmed discard (plan §14) and opens the modal on failure;
    /// `Sent` is the post-send cleanup (ADR 0002: best-effort — delivery
    /// is already confirmed, so a failure must never claim one). Boxed
    /// snapshot, as with `SaveDraft`.
    DeleteDraft {
        draft: Box<DraftSnapshot>,
        reason: DraftRemovalReason,
    },
    /// Deliver one serialized message through the Himalaya stdin contract
    /// (plan §14, Phase 7). The message is frozen at send time; retries
    /// replay the exact bytes under a new operation id. Boxed, as with the
    /// draft payloads.
    Send { message: Box<OutboundMessage> },
}

/// Why a draft is being removed (Phase 7.6): it selects the failure UX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DraftRemovalReason {
    /// Confirmed discard (plan §14): failures open Retry/Dismiss.
    Discard,
    /// Post-send cleanup (ADR 0002 consequences): best-effort, logged only.
    Sent,
}

impl OperationKind {
    /// Human-readable label for the status bar and error modal.
    pub fn summary(&self) -> &'static str {
        match self {
            OperationKind::LoadMailboxes => "Loading mailboxes",
            OperationKind::LoadPage(_) => "Loading messages",
            OperationKind::LoadMessage(_) => "Loading message",
            OperationKind::SetRead { read: true, .. } => "Marking read",
            OperationKind::SetRead { read: false, .. } => "Marking unread",
            OperationKind::SetStarred { starred: true, .. } => "Starring",
            OperationKind::SetStarred { starred: false, .. } => "Unstarring",
            OperationKind::Archive(_) => "Archiving",
            OperationKind::Trash(_) => "Moving to trash",
            OperationKind::SaveDraft { .. } => "Saving draft",
            OperationKind::LoadDrafts => "Restoring drafts",
            OperationKind::DeleteDraft { reason, .. } => match reason {
                DraftRemovalReason::Discard => "Discarding draft",
                DraftRemovalReason::Sent => "Cleaning up sent draft",
            },
            OperationKind::Send { .. } => "Sending message",
        }
    }

    /// The typed intent to store for a later retry (plan §12: "Store a
    /// serializable/cloneable `RetrySpec`, not a closure").
    pub fn retry_spec(&self) -> RetrySpec {
        RetrySpec { kind: self.clone() }
    }

    /// Whether `newer` supersedes `older`: a result for `older` must never
    /// mutate state once `newer` started. Mailbox loads supersede each
    /// other; page loads supersede page loads for the same mailbox; message
    /// loads for the same mailbox supersede each other (only the newest
    /// opened message can win); repeated flag toggles on the same message
    /// supersede each other. Mutations that move mail never supersede — a
    /// lost archive would be unrecoverable from state.
    fn supersedes(newer: &OperationKind, older: &OperationKind) -> bool {
        match (newer, older) {
            (OperationKind::LoadMailboxes, OperationKind::LoadMailboxes) => true,
            (OperationKind::LoadPage(newer), OperationKind::LoadPage(older)) => {
                newer.mailbox_id == older.mailbox_id
            }
            (OperationKind::LoadMessage(newer), OperationKind::LoadMessage(older)) => {
                newer.mailbox == older.mailbox
            }
            (
                OperationKind::SetRead { locator: newer, .. },
                OperationKind::SetRead { locator: older, .. },
            )
            | (
                OperationKind::SetStarred { locator: newer, .. },
                OperationKind::SetStarred { locator: older, .. },
            ) => newer.id == older.id,
            // A newer save of the same draft supersedes an older one: only
            // the newest revision may ever be pushed (plan §14 coalescing,
            // ADR 0002 §D.2). Restores and discards likewise supersede
            // their own kind.
            (
                OperationKind::SaveDraft { draft: newer },
                OperationKind::SaveDraft { draft: older },
            ) => newer.local_id == older.local_id,
            (OperationKind::LoadDrafts, OperationKind::LoadDrafts) => true,
            (
                OperationKind::DeleteDraft { draft: newer, .. },
                OperationKind::DeleteDraft { draft: older, .. },
            ) => newer.local_id == older.local_id,
            // Sends never supersede anything and are never superseded:
            // every delivery attempt must run to its classified outcome.
            _ => false,
        }
    }

    /// Whether `Esc` may cancel this operation (plan §11: "cancel the
    /// currently foregrounded cancellable operation"). Sends are never
    /// cancellable: killing himalaya mid-DATA leaves the delivery state
    /// unknown while the suppressed `Cancelled` result could claim neither
    /// failure nor success — exactly the ambiguity plan §12 forbids
    /// hiding. The user can still leave the composer; the send completes
    /// (or is classified) in the background.
    pub fn is_cancellable(&self) -> bool {
        !matches!(self, OperationKind::Send { .. })
    }
}

/// Serializable typed intent for retrying a failed operation (plan §12).
/// Retrying creates a *new* [`OperationId`]; the intent itself is replayed
/// unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrySpec {
    pub kind: OperationKind,
}

/// Successful payload of one backend operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOutcome {
    Mailboxes(Vec<Mailbox>),
    Page(Page<MessageSummary>),
    /// One fetched full message (plan §19 Phase 4).
    Message(Box<Message>),
    /// A mutation confirmed by the backend; the reducer applies the state
    /// change its own operation kind describes (flags echo only the
    /// affected values, ADR 0001 finding 6).
    Done,
    /// A draft save confirmed: the backend id of the new remote copy
    /// (ADR 0002 §D.3).
    DraftSaved {
        remote_id: MessageId,
    },
    /// Drafts restored from the journal at startup (ADR 0002 §D.5).
    Drafts(Vec<RestoredDraft>),
    /// One classified send outcome (plan §12, Phase 7). Successes and
    /// ambiguous/failed deliveries both arrive here; the reducer decides
    /// between "sent" and the (possibly duplicate-warning) modal.
    SendOutcome(SendOutcome),
}

/// A failure ready for the Retry/Dismiss modal (plan §12). Built by the
/// operation manager from the typed backend error; `detail` is sanitized
/// before it ever reaches state or UI, and `code` is the Himalaya exit
/// status when a child process ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationFailure {
    pub code: Option<i32>,
    pub detail: String,
    pub retry: Option<RetrySpec>,
    /// Set for ambiguous outcomes (e.g. a send that may or may not have
    /// been delivered, plan §12); the modal warns that retrying could
    /// duplicate work.
    pub ambiguous: bool,
}

/// What the backend task sends back through the task result channel for
/// one operation (plan §19 Phase 3: "task result channel").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationResult {
    pub id: OperationId,
    pub outcome: Result<OperationOutcome, OperationFailure>,
}

/// One in-flight backend operation (plan §11).
#[derive(Debug, Clone)]
pub struct Operation {
    pub id: OperationId,
    pub kind: OperationKind,
    pub started_at: Instant,
    pub retry: Option<RetrySpec>,
    /// Cancelling this token terminates the operation, including the child
    /// process the backend owns (Phase 3.2).
    pub cancellation: CancellationToken,
}

/// Registry of in-flight operations, owned by [`crate::app::state::AppState`].
#[derive(Debug, Default, Clone)]
pub struct OperationRegistry {
    next_id: u64,
    entries: HashMap<OperationId, Operation>,
    /// The most recently started operation still in flight — the one `Esc`
    /// cancels first (plan §10: "Cancel work, close overlay, or go back").
    foreground: Option<OperationId>,
}

impl OperationRegistry {
    /// Start a new operation of `kind`: allocates its id, registers it with
    /// a fresh cancellation token, and cancels+removes any in-flight
    /// operation it supersedes. Returns the effect for the runtime to
    /// launch.
    pub fn start(&mut self, kind: OperationKind) -> crate::app::effect::Effect {
        for id in self.superseded_ids(&kind) {
            if let Some(older) = self.cancel(id) {
                tracing::debug!(id = %older.id, "superseded by newer operation");
                older.cancellation.cancel();
            }
        }
        self.next_id = self.next_id.wrapping_add(1);
        let id = OperationId(self.next_id);
        self.entries.insert(
            id,
            Operation {
                id,
                kind: kind.clone(),
                started_at: Instant::now(),
                retry: Some(kind.retry_spec()),
                cancellation: CancellationToken::new(),
            },
        );
        self.foreground = Some(id);
        crate::app::effect::Effect { id, kind }
    }

    /// Ids of in-flight operations that `kind` supersedes.
    fn superseded_ids(&self, kind: &OperationKind) -> Vec<OperationId> {
        self.entries
            .values()
            .filter(|op| OperationKind::supersedes(kind, &op.kind))
            .map(|op| op.id)
            .collect()
    }

    /// The registered operation, if any.
    pub fn get(&self, id: OperationId) -> Option<&Operation> {
        self.entries.get(&id)
    }

    /// The cancellation token for `id`, for the runtime to hand to the
    /// backend together with the effect.
    pub fn cancellation(&self, id: OperationId) -> Option<CancellationToken> {
        self.entries.get(&id).map(|op| op.cancellation.clone())
    }

    /// Complete `id`: removes and returns it, or `None` when the result is
    /// unknown, cancelled, or already superseded (the caller must then
    /// ignore it).
    pub fn finish(&mut self, id: OperationId) -> Option<Operation> {
        let op = self.entries.remove(&id);
        if self.foreground == Some(id) {
            self.foreground = self.latest();
        }
        op
    }

    /// Cancel `id`: fires its token (killing the child the backend owns)
    /// and removes it. Returns the operation when it was still in flight.
    pub fn cancel(&mut self, id: OperationId) -> Option<Operation> {
        let op = self.finish(id);
        if let Some(op) = &op {
            op.cancellation.cancel();
        }
        op
    }

    /// Cancel the foregrounded cancellable operation (`Esc`, plan §10/§11).
    /// A non-cancellable foreground operation (a send in flight) is left
    /// running: `Esc` falls through to navigation instead.
    pub fn cancel_foreground(&mut self) -> Option<Operation> {
        let id = self.foreground?;
        let cancellable = self
            .entries
            .get(&id)
            .is_some_and(|op| op.kind.is_cancellable());
        if !cancellable {
            return None;
        }
        self.cancel(id)
    }

    /// The foregrounded operation, for the status bar spinner.
    pub fn foreground(&self) -> Option<&Operation> {
        self.foreground.and_then(|id| self.entries.get(&id))
    }

    /// The page request currently in flight for `mailbox_id`, if any.
    /// Superseding guarantees at most one.
    pub fn page_in_flight(&self, mailbox_id: &crate::domain::MailboxId) -> Option<PageRequest> {
        self.entries.values().find_map(|op| match &op.kind {
            OperationKind::LoadPage(request) if &request.mailbox_id == mailbox_id => {
                Some(request.clone())
            }
            _ => None,
        })
    }

    /// Whether a mailbox listing is currently in flight.
    pub fn is_loading_mailboxes(&self) -> bool {
        self.entries
            .values()
            .any(|op| matches!(op.kind, OperationKind::LoadMailboxes))
    }

    /// Whether a journal restore is currently in flight.
    pub fn is_loading_drafts(&self) -> bool {
        self.entries
            .values()
            .any(|op| matches!(op.kind, OperationKind::LoadDrafts))
    }

    /// Whether a message send is currently in flight (Phase 7): a second
    /// send must wait rather than risk a duplicate delivery.
    pub fn is_sending(&self) -> bool {
        self.entries
            .values()
            .any(|op| matches!(op.kind, OperationKind::Send { .. }))
    }

    /// Whether a save of exactly `revision` of `local_id` is in flight —
    /// used to avoid duplicating an already-running push when forcing a
    /// save on leave (plan §14).
    pub fn is_saving_draft(&self, local_id: &crate::domain::DraftId, revision: u64) -> bool {
        self.entries.values().any(|op| match &op.kind {
            OperationKind::SaveDraft { draft } => {
                draft.local_id == *local_id && draft.revision == revision
            }
            _ => false,
        })
    }

    /// Cancel every in-flight save of `local_id` (confirmed discard): the
    /// tokens fire so the backend stops its children, and the operations
    /// are removed so their results can never re-apply.
    pub fn cancel_draft_saves(&mut self, local_id: &crate::domain::DraftId) {
        let ids: Vec<OperationId> = self
            .entries
            .values()
            .filter(|op| match &op.kind {
                OperationKind::SaveDraft { draft } => draft.local_id == *local_id,
                _ => false,
            })
            .map(|op| op.id)
            .collect();
        for id in ids {
            if let Some(op) = self.cancel(id) {
                tracing::debug!(id = %op.id, "draft save cancelled for discard");
            }
        }
    }

    /// Whether nothing is in flight (spinner hidden).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of in-flight operations.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn latest(&self) -> Option<OperationId> {
        self.entries.values().max_by_key(|op| op.id).map(|op| op.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::effect::Effect;
    use crate::domain::MailboxId;

    fn page(mailbox: &str, offset: usize) -> OperationKind {
        OperationKind::LoadPage(PageRequest {
            mailbox_id: MailboxId(String::from(mailbox)),
            offset,
            limit: 20,
        })
    }

    #[test]
    fn start_allocates_fresh_ids_and_registers() {
        let mut registry = OperationRegistry::default();
        let first = registry.start(OperationKind::LoadMailboxes);
        let second = registry.start(page("inbox", 0));
        assert_ne!(first.id, second.id);
        assert_eq!(registry.get(first.id).unwrap().kind, first.kind);
        assert_eq!(
            registry.foreground().map(|op| op.id),
            registry.get(second.id).map(|op| op.id)
        );
        assert!(!registry.is_empty());
    }

    #[test]
    fn newer_same_kind_supersedes_and_cancels_older() {
        let mut registry = OperationRegistry::default();
        let older = registry.start(page("inbox", 0));
        let older_token = registry.cancellation(older.id).unwrap();
        let newer = registry.start(page("inbox", 20));
        // The older operation is gone; its token fired so the backend kills
        // the child it owns.
        assert!(registry.get(older.id).is_none());
        assert!(older_token.is_cancelled());
        assert!(registry.get(newer.id).is_some());
    }

    #[test]
    fn page_loads_for_other_mailboxes_do_not_supersede() {
        let mut registry = OperationRegistry::default();
        let inbox = registry.start(page("inbox", 0));
        let sent = registry.start(page("sent", 0));
        assert!(registry.get(inbox.id).is_some());
        assert!(registry.get(sent.id).is_some());
        // Both in flight; the later one is foreground.
        assert_eq!(registry.foreground().unwrap().id, sent.id);
    }

    #[test]
    fn finish_removes_and_unknown_results_are_none() {
        let mut registry = OperationRegistry::default();
        let effect = registry.start(OperationKind::LoadMailboxes);
        let op = registry.finish(effect.id).expect("in flight");
        assert_eq!(op.id, effect.id);
        assert!(registry.get(effect.id).is_none());
        assert!(registry.finish(effect.id).is_none(), "already finished");
        assert!(registry.finish(OperationId(999)).is_none());
    }

    #[test]
    fn cancel_fires_token_and_removes() {
        let mut registry = OperationRegistry::default();
        let effect = registry.start(page("inbox", 0));
        let token = registry.cancellation(effect.id).unwrap();
        let op = registry.cancel(effect.id).expect("in flight");
        assert!(token.is_cancelled());
        assert!(registry.get(op.id).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn cancel_foreground_targets_the_latest_operation() {
        let mut registry = OperationRegistry::default();
        let first = registry.start(page("inbox", 0));
        let second = registry.start(page("sent", 0));
        let cancelled = registry.cancel_foreground().expect("foreground");
        assert_eq!(cancelled.id, second.id);
        assert!(registry.get(first.id).is_some(), "older op untouched");
        assert!(registry.get(second.id).is_none());
        // Foreground falls back to the remaining operation.
        assert_eq!(registry.foreground().unwrap().id, first.id);
    }

    #[test]
    fn page_in_flight_returns_the_only_request_per_mailbox() {
        let mut registry = OperationRegistry::default();
        assert!(
            registry
                .page_in_flight(&MailboxId(String::from("inbox")))
                .is_none()
        );
        registry.start(page("inbox", 20));
        assert_eq!(
            registry.page_in_flight(&MailboxId(String::from("inbox"))),
            Some(PageRequest {
                mailbox_id: MailboxId(String::from("inbox")),
                offset: 20,
                limit: 20,
            })
        );
        registry.start(page("sent", 0));
        assert!(
            registry
                .page_in_flight(&MailboxId(String::from("sent")))
                .is_some()
        );
    }

    #[test]
    fn mailboxes_in_flight_is_detected() {
        let mut registry = OperationRegistry::default();
        assert!(!registry.is_loading_mailboxes());
        registry.start(page("inbox", 0));
        assert!(!registry.is_loading_mailboxes());
        registry.start(OperationKind::LoadMailboxes);
        assert!(registry.is_loading_mailboxes());
    }

    #[test]
    fn effects_carry_registry_ids() {
        let mut registry = OperationRegistry::default();
        let effect = registry.start(page("inbox", 0));
        assert_eq!(effect.id, registry.get(effect.id).unwrap().id);
        assert_eq!(effect.retry_spec().kind, effect.kind);
        let _ = effect as Effect; // type shape check
    }
}
