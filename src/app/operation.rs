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
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::domain::{
    Mailbox, Message, MessageId, MessageSummary, Page, PageRequest, RestoredDraft, SearchRequest,
    SendOutcome,
};

pub use crate::domain::operation::OperationId;

mod account;
mod cache;
mod draft;
mod files;
mod mail;
mod platform;

pub use account::AccountOperation;
pub use cache::CacheOperation;
pub use draft::{DraftOperation, DraftRemovalReason};
pub use files::FileOperation;
pub use mail::{MailOperation, SeedKind};
pub use platform::PlatformOperation;

/// The typed intent of one operation (plan §5: "Effects launch typed
/// backend requests"). A family tag around the domain-specific operation
/// enums (issue ceh0): reads and mutations live in [`MailOperation`],
/// draft/send work in [`DraftOperation`], attachment files in
/// [`FileOperation`], desktop handoffs in [`PlatformOperation`], wizard
/// work in [`AccountOperation`], notifications in [`NotifyRequest`], and
/// cache I/O in [`CacheOperation`]. A new feature adds a variant to one
/// family — with its summary/supersession/cancellation rules right
/// beside it — instead of expanding a central catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationKind {
    /// Mail reads and mutations: the list, the reader, flags, and moves.
    Mail(MailOperation),
    /// Drafts: autosave/push, journal restore, discard/sent cleanup, send.
    Draft(DraftOperation),
    /// Attachment files: source validation, chooser listing, saving.
    Files(FileOperation),
    /// Desktop handoffs: platform open, browser, external editor.
    Platform(PlatformOperation),
    /// The configuration wizard: discovery, credential test, save.
    Account(AccountOperation),
    /// New-mail notification (bell or desktop).
    Notify(NotifyRequest),
    /// Summary/message/mailbox cache I/O (off-thread, never the reducer).
    Cache(CacheOperation),
}

/// One new-mail notification (`[tmail].notifications`, ticket b28p): the
/// terminal bell or a desktop notification. Built by the reducer from the
/// messages the background refresh found; delivered by the manager,
/// best-effort — a failure is logged, never modaled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyRequest {
    /// Ring the terminal bell (`\x07`).
    Bell,
    /// Show a desktop notification through `notify-rust`.
    Desktop {
        /// Notification title: a single message's sender, else "Tmail".
        summary: String,
        /// Notification body: a single message's subject, else the count.
        body: String,
    },
}

impl NotifyRequest {
    /// Human-readable label for the status bar and error modal.
    pub(crate) fn summary(&self) -> &'static str {
        "Notifying"
    }

    /// Notifications never supersede anything (plan §11): a lost chime or
    /// banner is nothing to recover.
    pub(crate) fn supersedes(_newer: &NotifyRequest, _older: &NotifyRequest) -> bool {
        false
    }

    /// No notification coalesces duplicates.
    pub(crate) fn duplicates_of(&self, _older: &NotifyRequest) -> bool {
        false
    }

    /// Whether `Esc` may cancel the operation (plan §11): yes — the
    /// delivery is best-effort and a failure is logged, never modaled.
    pub(crate) fn is_cancellable(&self) -> bool {
        true
    }

    /// No notification runs a mail-backend child (issue 1v38): the bell
    /// is a byte to stdout, the desktop path goes to `notify-rust`.
    pub(crate) fn uses_backend_process(&self) -> bool {
        false
    }
}

impl OperationKind {
    /// Human-readable label for the status bar and error modal.
    pub fn summary(&self) -> &'static str {
        match self {
            OperationKind::Mail(op) => op.summary(),
            OperationKind::Draft(op) => op.summary(),
            OperationKind::Files(op) => op.summary(),
            OperationKind::Platform(op) => op.summary(),
            OperationKind::Account(op) => op.summary(),
            OperationKind::Notify(request) => request.summary(),
            OperationKind::Cache(op) => op.summary(),
        }
    }

    /// The typed intent to store for a later retry (plan §12: "Store a
    /// serializable/cloneable `RetrySpec`, not a closure"). Retrying
    /// creates a *new* [`OperationId`]; the intent itself is replayed
    /// unchanged.
    pub fn retry_spec(&self) -> RetrySpec {
        RetrySpec { kind: self.clone() }
    }

    /// Whether `newer` supersedes `older`: a result for `older` must
    /// never mutate state once `newer` started. Only same-family
    /// operations can supersede each other, and each family owns its own
    /// rules; cross-family results simply never apply over one another.
    fn supersedes(newer: &OperationKind, older: &OperationKind) -> bool {
        match (newer, older) {
            (OperationKind::Mail(newer), OperationKind::Mail(older)) => {
                MailOperation::supersedes(newer, older)
            }
            (OperationKind::Draft(newer), OperationKind::Draft(older)) => {
                DraftOperation::supersedes(newer, older)
            }
            (OperationKind::Files(newer), OperationKind::Files(older)) => {
                FileOperation::supersedes(newer, older)
            }
            (OperationKind::Account(newer), OperationKind::Account(older)) => {
                AccountOperation::supersedes(newer, older)
            }
            (OperationKind::Platform(newer), OperationKind::Platform(older)) => {
                PlatformOperation::supersedes(newer, older)
            }
            (OperationKind::Notify(newer), OperationKind::Notify(older)) => {
                NotifyRequest::supersedes(newer, older)
            }
            (OperationKind::Cache(newer), OperationKind::Cache(older)) => {
                CacheOperation::supersedes(newer, older)
            }
            _ => false,
        }
    }

    /// Whether `self` is a *duplicate* of an in-flight `older` operation:
    /// the identical intent on the identical target. Same-family only,
    /// delegated to the family; only the mail family has coalescing rules
    /// (the move mutations, ticket j9bq).
    fn duplicates_of(&self, older: &OperationKind) -> bool {
        match (self, older) {
            (OperationKind::Mail(newer), OperationKind::Mail(older)) => newer.duplicates_of(older),
            (OperationKind::Draft(newer), OperationKind::Draft(older)) => {
                newer.duplicates_of(older)
            }
            (OperationKind::Files(newer), OperationKind::Files(older)) => {
                newer.duplicates_of(older)
            }
            (OperationKind::Platform(newer), OperationKind::Platform(older)) => {
                newer.duplicates_of(older)
            }
            (OperationKind::Account(newer), OperationKind::Account(older)) => {
                newer.duplicates_of(older)
            }
            (OperationKind::Notify(newer), OperationKind::Notify(older)) => {
                newer.duplicates_of(older)
            }
            (OperationKind::Cache(newer), OperationKind::Cache(older)) => {
                newer.duplicates_of(older)
            }
            _ => false,
        }
    }

    /// Whether `Esc` may cancel this operation (plan §11: "cancel the
    /// currently foregrounded cancellable operation"), delegated to the
    /// family: sends and desktop handoffs are never cancellable, exactly
    /// the ambiguity plan §12 forbids hiding.
    pub fn is_cancellable(&self) -> bool {
        match self {
            OperationKind::Mail(op) => op.is_cancellable(),
            OperationKind::Draft(op) => op.is_cancellable(),
            OperationKind::Files(op) => op.is_cancellable(),
            OperationKind::Platform(op) => op.is_cancellable(),
            OperationKind::Account(op) => op.is_cancellable(),
            OperationKind::Notify(request) => request.is_cancellable(),
            OperationKind::Cache(op) => op.is_cancellable(),
        }
    }
}

/// Serializable typed intent for retrying a failed operation (plan §12).
/// Retrying creates a *new* [`OperationId`]; the intent itself is replayed
/// unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// One validated attachment source (plan §15, Phase 8): metadata only,
    /// never bytes.
    Attachment(crate::domain::DraftAttachment),
    /// The attachment chooser's explorer state for one directory listing
    /// (plan §15, ticket 95x0): built by the manager (filesystem access)
    /// and applied by the reducer.
    Explorer(Box<ratatui_explorer::FileExplorer>),
    /// One attachment saved to disk (plan §15, Phase 8.4): the final path
    /// actually written — possibly a collision-renamed name, so the UI
    /// always reports this path, never the requested one.
    SavedPath(std::path::PathBuf),
    /// Ranked discovery candidates for the wizard's email address
    /// (ADR 0003 §3.3); empty means nothing was found in time.
    Discovered(Vec<crate::discovery::DiscoveredService>),
    /// The wizard's credential test passed (ADR 0003 §3.4): the mailboxes
    /// the draft account can list, each tagged with the role the server
    /// itself attributed (RFC 6154 SPECIAL-USE on IMAP) when it did.
    TestAccountCompleted {
        mailboxes: Vec<crate::domain::TestedMailbox>,
    },
    /// The wizard account was merged into the config file (ADR 0003
    /// §3.6): the file path, whether it was freshly created, and the
    /// permissions warning when the file was group/world-readable.
    /// `created_mailboxes` lists the special folders the server did not
    /// have and the manager provisioned before the save (issue txps).
    AccountSaved {
        path: std::path::PathBuf,
        created: bool,
        permissions_warning: Option<String>,
        created_mailboxes: Vec<String>,
    },
    /// One cached page of summaries (ticket haeb): served off disk by the
    /// manager, applied by the reducer only in a cold context.
    CachedPage(Page<MessageSummary>),
    /// One cached full message (ticket haeb): the reader renders it
    /// instantly; the fresh fetch still converges afterwards.
    CachedMessage(Box<Message>),
    /// The cached mailbox listing (ticket haeb): the sidebar renders
    /// instantly; the fresh listing still loads in the background.
    CachedMailboxes(Vec<Mailbox>),
    /// The requested cache entry does not exist (or is stale/unparsable):
    /// the caller falls through to the fresh load. Cache reads never
    /// fail — a broken cache degrades to the spinner, never to an error.
    CacheMiss,
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
    /// Who asked for this work (Phase 9): foreground operations surface
    /// failures in the Retry/Dismiss modal; a *background* refresh failure
    /// never interrupts the user — it lands in the status line, with
    /// repeated identical failures suppressed (Phase 9.6).
    pub origin: OperationOrigin,
}

/// Who requested an operation (Phase 9): see [`Operation::origin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationOrigin {
    Foreground,
    Background,
}

/// Registry of in-flight operations, owned by [`crate::app::state::AppState`].
#[derive(Debug, Default, Clone)]
pub struct OperationRegistry {
    next_id: u64,
    entries: HashMap<OperationId, Operation>,
    /// The most recently started *foreground* operation still in flight —
    /// the one `Esc` cancels first (plan §10: "Cancel work, close overlay,
    /// or go back"). Background-origin work (timer refreshes, ticket wxtx
    /// previews) never takes the slot: `Esc` must reach interactive work,
    /// and the spinner names only what `Esc` would cancel.
    foreground: Option<OperationId>,
}

impl OperationRegistry {
    /// Start a new operation of `kind`: allocates its id, registers it with
    /// a fresh cancellation token, and cancels+removes any in-flight
    /// operation it supersedes. Returns the effect for the runtime to
    /// launch.
    pub fn start(&mut self, kind: OperationKind) -> crate::app::effect::Effect {
        self.start_with_origin(kind, OperationOrigin::Foreground)
    }

    /// Start a *background* operation (Phase 9.4: the periodic timer):
    /// identical lifecycle to [`Self::start`], but failures are handled as
    /// background work (status line, never a modal — Phase 9.6).
    pub fn start_background(&mut self, kind: OperationKind) -> crate::app::effect::Effect {
        self.start_with_origin(kind, OperationOrigin::Background)
    }

    /// Start a foreground operation unless its identical intent is already
    /// in flight (`Archive`/`Trash` of the same message, see
    /// [`OperationKind::duplicates_of`]): a double-press must not spawn a
    /// second concurrent mutation that can only fail. Returns `None` — and
    /// leaves the registry untouched — for the coalesced duplicate.
    pub fn start_unless_duplicate(
        &mut self,
        kind: OperationKind,
    ) -> Option<crate::app::effect::Effect> {
        self.try_start_with_origin(kind, OperationOrigin::Foreground)
    }

    fn try_start_with_origin(
        &mut self,
        kind: OperationKind,
        origin: OperationOrigin,
    ) -> Option<crate::app::effect::Effect> {
        let Some(existing) = self
            .entries
            .values()
            .find(|op| kind.duplicates_of(&op.kind))
            .map(|op| op.id)
        else {
            return Some(self.start_with_origin(kind, origin));
        };
        tracing::debug!(
            id = %existing,
            "duplicate move request coalesced into the in-flight operation"
        );
        None
    }

    fn start_with_origin(
        &mut self,
        kind: OperationKind,
        origin: OperationOrigin,
    ) -> crate::app::effect::Effect {
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
                origin,
            },
        );
        // Only foreground work takes the `Esc`-cancel / spinner slot
        // (ticket wxtx): silent background fetches (previews, timer
        // refreshes) must never absorb `Esc` or announce themselves.
        if origin == OperationOrigin::Foreground {
            self.foreground = Some(id);
        }
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
            OperationKind::Mail(MailOperation::LoadPage(request))
                if &request.mailbox_id == mailbox_id =>
            {
                Some(request.clone())
            }
            _ => None,
        })
    }

    /// The search request currently in flight for `mailbox_id`, if any
    /// (Phase 9). Superseding guarantees at most one.
    pub fn search_in_flight(&self, mailbox_id: &crate::domain::MailboxId) -> Option<SearchRequest> {
        self.entries.values().find_map(|op| match &op.kind {
            OperationKind::Mail(MailOperation::Search(request))
                if &request.mailbox_id == mailbox_id =>
            {
                Some(request.clone())
            }
            _ => None,
        })
    }

    /// Whether a mailbox listing is currently in flight.
    pub fn is_loading_mailboxes(&self) -> bool {
        self.entries
            .values()
            .any(|op| matches!(op.kind, OperationKind::Mail(MailOperation::LoadMailboxes)))
    }

    /// Whether a journal restore is currently in flight.
    pub fn is_loading_drafts(&self) -> bool {
        self.entries
            .values()
            .any(|op| matches!(op.kind, OperationKind::Draft(DraftOperation::LoadDrafts)))
    }

    /// Whether a message send is currently in flight (Phase 7): a second
    /// send must wait rather than risk a duplicate delivery.
    pub fn is_sending(&self) -> bool {
        self.entries
            .values()
            .any(|op| matches!(op.kind, OperationKind::Draft(DraftOperation::Send { .. })))
    }

    /// Whether a save of exactly `revision` of `local_id` is in flight —
    /// used to avoid duplicating an already-running push when forcing a
    /// save on leave (plan §14).
    pub fn is_saving_draft(&self, local_id: &crate::domain::DraftId, revision: u64) -> bool {
        self.entries.values().any(|op| match &op.kind {
            OperationKind::Draft(DraftOperation::SaveDraft { draft }) => {
                draft.local_id == *local_id && draft.revision == revision
            }
            _ => false,
        })
    }

    /// Cancel every in-flight preview fetch (ticket 183r): previews serve
    /// the *displayed* list, and a mailbox switch makes them pointless
    /// while each holds a backend permit until its child finishes — up to
    /// all four, with previews uncancellable by `Esc`. Tokens fire so the
    /// children die at once; entries clear so late results are rejected as
    /// unknown.
    pub fn cancel_previews(&mut self) {
        let ids: Vec<OperationId> = self
            .entries
            .values()
            .filter(|op| matches!(op.kind, OperationKind::Mail(MailOperation::Preview(_))))
            .map(|op| op.id)
            .collect();
        for id in ids {
            if let Some(op) = self.cancel(id) {
                tracing::debug!(id = %op.id, "preview fetch cancelled (mailbox switched)");
            }
        }
    }

    /// Cancel every in-flight save of `local_id` (confirmed discard): the
    /// tokens fire so the backend stops its children, and the operations
    /// are removed so their results can never re-apply.
    pub fn cancel_draft_saves(&mut self, local_id: &crate::domain::DraftId) {
        let ids: Vec<OperationId> = self
            .entries
            .values()
            .filter(|op| match &op.kind {
                OperationKind::Draft(DraftOperation::SaveDraft { draft }) => {
                    draft.local_id == *local_id
                }
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

    /// Cancel every in-flight operation (the confirmed account switch,
    /// ticket c0n0): tokens fire so the backends kill their children,
    /// entries clear so a late result is rejected as unknown, and the
    /// foreground slot empties. Returns how many operations were
    /// cancelled.
    pub fn cancel_all(&mut self) -> usize {
        let count = self.entries.len();
        for op in self.entries.values() {
            op.cancellation.cancel();
        }
        self.entries.clear();
        self.foreground = None;
        count
    }

    /// The in-flight operations as sorted, deduplicated display summaries
    /// with counts (`2× Fetching preview`) — the lines the account-switch
    /// confirmation lists (ticket c0n0).
    pub fn in_flight_summaries(&self) -> Vec<String> {
        let mut counts: std::collections::BTreeMap<&'static str, usize> = Default::default();
        for op in self.entries.values() {
            *counts.entry(op.kind.summary()).or_default() += 1;
        }
        counts
            .into_iter()
            .map(|(summary, count)| {
                if count == 1 {
                    summary.to_owned()
                } else {
                    format!("{summary} ×{count}")
                }
            })
            .collect()
    }

    /// Whether nothing is in flight (spinner hidden).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether any *foreground*-origin operation is in flight. The
    /// auto-refresh timer stands down for interactive work but never for
    /// silent background fetches (ticket wxtx previews).
    pub fn has_foreground(&self) -> bool {
        self.entries
            .values()
            .any(|op| op.origin == OperationOrigin::Foreground)
    }

    /// How many preview fetches (ticket wxtx) are in flight — the rolling
    /// fetch window's occupancy.
    pub fn previews_in_flight(&self) -> usize {
        self.entries
            .values()
            .filter(|op| matches!(op.kind, OperationKind::Mail(MailOperation::Preview(_))))
            .count()
    }

    /// Number of in-flight operations.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn latest(&self) -> Option<OperationId> {
        // Only foreground work can hold the slot (see `foreground`): a
        // background fetch that started later must never inherit it.
        self.entries
            .values()
            .filter(|op| op.origin == OperationOrigin::Foreground)
            .max_by_key(|op| op.id)
            .map(|op| op.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::effect::Effect;
    use crate::domain::{MailboxId, MessageLocator};

    fn page(mailbox: &str, offset: usize) -> OperationKind {
        OperationKind::Mail(MailOperation::LoadPage(PageRequest {
            mailbox_id: MailboxId(String::from(mailbox)),
            offset,
            limit: 20,
        }))
    }

    #[test]
    fn start_allocates_fresh_ids_and_registers() {
        let mut registry = OperationRegistry::default();
        let first = registry.start(OperationKind::Mail(MailOperation::LoadMailboxes));
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

    fn locator(mailbox: &str, id: &str) -> MessageLocator {
        MessageLocator {
            mailbox: MailboxId(String::from(mailbox)),
            id: MessageId(String::from(id)),
            message_id: None,
        }
    }

    #[test]
    fn message_loads_supersede_the_same_id_across_mailboxes() {
        // Only the newest opened message can win: the same listed id may
        // exist in two mailboxes (maildir rename / role target), and the
        // registry keeps one child per message.
        let mut registry = OperationRegistry::default();
        let older = registry.start(OperationKind::Mail(MailOperation::LoadMessage(locator(
            "inbox", "m1",
        ))));
        let newer = registry.start(OperationKind::Mail(MailOperation::LoadMessage(locator(
            "archive", "m1",
        ))));
        assert!(registry.get(older.id).is_none(), "older fetch cancelled");
        assert!(registry.get(newer.id).is_some());
        // A different message in flight is left alone.
        let other = registry.start(OperationKind::Mail(MailOperation::LoadMessage(locator(
            "inbox", "m2",
        ))));
        assert!(registry.get(other.id).is_some());
    }

    #[test]
    fn duplicate_archive_and_trash_requests_coalesce() {
        // A double-press must not run the same move twice concurrently:
        // the second backend run would fail confusingly after the maildir
        // rename. The registry drops the duplicate and keeps the first.
        let mut registry = OperationRegistry::default();
        let first = registry
            .start_unless_duplicate(OperationKind::Mail(MailOperation::Trash(locator(
                "inbox", "m1",
            ))))
            .expect("first owns the work");
        assert!(registry.get(first.id).is_some());
        assert!(
            registry
                .start_unless_duplicate(OperationKind::Mail(MailOperation::Trash(locator(
                    "inbox", "m1"
                ))))
                .is_none(),
            "identical duplicate coalesced"
        );
        assert!(
            registry
                .start_unless_duplicate(OperationKind::Mail(MailOperation::Archive(locator(
                    "inbox", "m1"
                ))))
                .is_some(),
            "a different move of the same message still runs"
        );
        assert!(
            registry
                .start_unless_duplicate(OperationKind::Mail(MailOperation::Trash(locator(
                    "inbox", "m2"
                ))))
                .is_some(),
            "the same move of another message still runs"
        );
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn duplicate_requests_never_supersede_the_first_one() {
        let mut registry = OperationRegistry::default();
        let first = registry
            .start_unless_duplicate(OperationKind::Mail(MailOperation::Archive(locator(
                "inbox", "m1",
            ))))
            .expect("in flight");
        let token = registry.cancellation(first.id).unwrap();
        assert!(
            registry
                .start_unless_duplicate(OperationKind::Mail(MailOperation::Archive(locator(
                    "inbox", "m1"
                ))))
                .is_none()
        );
        assert!(registry.get(first.id).is_some());
        assert!(!token.is_cancelled(), "the first move is untouched");
    }

    #[test]
    fn finish_removes_and_unknown_results_are_none() {
        let mut registry = OperationRegistry::default();
        let effect = registry.start(OperationKind::Mail(MailOperation::LoadMailboxes));
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
        registry.start(OperationKind::Mail(MailOperation::LoadMailboxes));
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

    #[test]
    fn cancel_all_fires_every_token_and_clears_the_registry() {
        // Ticket c0n0: the confirmed account switch cancels everything at
        // once — tokens fire so the children die, entries clear so late
        // results are rejected.
        let mut registry = OperationRegistry::default();
        let first = registry.start(page("inbox", 0));
        let second = registry.start(page("sent", 0));
        let third = registry.start(OperationKind::Mail(MailOperation::LoadMailboxes));
        let tokens: Vec<_> = [&first, &second, &third]
            .into_iter()
            .map(|effect| registry.cancellation(effect.id).unwrap())
            .collect();
        assert_eq!(registry.cancel_all(), 3);
        for token in tokens {
            assert!(token.is_cancelled());
        }
        assert!(registry.is_empty());
        assert!(registry.foreground().is_none());
        assert_eq!(registry.cancel_all(), 0, "second pass is a no-op");
    }

    #[test]
    fn in_flight_summaries_sort_dedupe_and_count() {
        let mut registry = OperationRegistry::default();
        let locator = MessageLocator {
            mailbox: MailboxId(String::from("inbox")),
            id: MessageId(String::from("m1")),
            message_id: None,
        };
        registry.start_background(OperationKind::Mail(MailOperation::Preview(locator.clone())));
        registry.start_background(OperationKind::Mail(MailOperation::Preview(locator)));
        registry.start(page("inbox", 0));
        assert_eq!(
            registry.in_flight_summaries(),
            vec![
                String::from("Fetching preview ×2"),
                String::from("Loading messages"),
            ]
        );
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;
    use crate::domain::{MailboxId, MessageLocator};

    fn page(mailbox: &str, offset: usize) -> OperationKind {
        OperationKind::Mail(MailOperation::LoadPage(PageRequest {
            mailbox_id: MailboxId(String::from(mailbox)),
            offset,
            limit: 20,
        }))
    }

    #[test]
    fn background_start_marks_the_origin() {
        let mut registry = OperationRegistry::default();
        let foreground = registry.start(page("inbox", 0));
        let background = registry.start_background(page("inbox", 0));
        // The background request superseded the foreground one (same page
        // kind, same mailbox) — the flag is on the surviving operation.
        assert!(registry.get(foreground.id).is_none());
        assert_eq!(
            registry.get(background.id).map(|op| op.origin),
            Some(OperationOrigin::Background)
        );
    }

    #[test]
    fn background_operations_never_take_the_foreground_slot() {
        let mut registry = OperationRegistry::default();
        // Ticket wxtx: silent background fetches must never absorb `Esc`
        // or drive the spinner.
        let background = registry.start_background(page("inbox", 0));
        assert!(registry.foreground().is_none());
        assert!(registry.get(background.id).is_some());
        // A later foreground op holds the slot; when it completes, the
        // pointer falls back to nothing — never to the background fetch.
        let foreground = registry.start(OperationKind::Mail(MailOperation::LoadMailboxes));
        assert_eq!(registry.foreground().map(|op| op.id), Some(foreground.id));
        registry.finish(foreground.id);
        assert!(registry.foreground().is_none());
        assert!(registry.get(background.id).is_some());
    }

    #[test]
    fn previews_in_flight_counts_only_preview_operations() {
        let mut registry = OperationRegistry::default();
        let locator = MessageLocator {
            mailbox: MailboxId(String::from("inbox")),
            id: crate::domain::MessageId(String::from("m1")),
            message_id: None,
        };
        assert_eq!(registry.previews_in_flight(), 0);
        registry.start_background(OperationKind::Mail(MailOperation::Preview(locator.clone())));
        registry.start_background(OperationKind::Mail(MailOperation::Preview(locator)));
        registry.start_background(page("inbox", 0));
        assert_eq!(registry.previews_in_flight(), 2);
        // And they do not block the foreground slot:
        assert!(!registry.has_foreground());
    }

    #[test]
    fn cancel_previews_kills_only_the_previews() {
        // Ticket 183r: a mailbox switch must free the permits previews
        // hold without touching any other in-flight work.
        let mut registry = OperationRegistry::default();
        let preview = registry.start_background(OperationKind::Mail(MailOperation::Preview(
            MessageLocator {
                mailbox: MailboxId(String::from("inbox")),
                id: crate::domain::MessageId(String::from("m1")),
                message_id: None,
            },
        )));
        let preview_token = registry.cancellation(preview.id).expect("token");
        let page_op = registry.start(page("inbox", 0));
        let page_token = registry.cancellation(page_op.id).expect("token");
        registry.cancel_previews();
        assert_eq!(registry.previews_in_flight(), 0);
        assert!(registry.get(preview.id).is_none(), "preview removed");
        assert!(preview_token.is_cancelled(), "preview token fired");
        assert!(
            registry.get(page_op.id).is_some(),
            "unrelated work untouched"
        );
        assert!(!page_token.is_cancelled(), "unrelated token untouched");
    }
}
