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

use tokio_util::sync::CancellationToken;

use crate::domain::{
    DraftSnapshot, Mailbox, MailboxId, Message, MessageId, MessageLocator, MessageSummary,
    OutboundMessage, Page, PageRequest, RestoredDraft, SearchRequest, SendOutcome,
};

/// Opaque identifier carried by every backend request and result (plan §5:
/// "Every request and result carries an `OperationId`"). Constructed only
/// by the registry; test code may synthesize unknown ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(pub u64);

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "op-{}", self.0)
    }
}

/// The typed intent of one backend operation (plan §5: "Effects launch
/// typed backend requests").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationKind {
    /// Fetch the mailbox listing.
    LoadMailboxes,
    /// Fetch one page of message summaries.
    LoadPage(PageRequest),
    /// Fetch one page of search results (plan §16/§19 Phase 9): the query
    /// travels unchanged, scoped to the mailbox the search was launched
    /// from. Results shape like `LoadPage`.
    Search(SearchRequest),
    /// Fetch one full message (plan §19 Phase 4: reader).
    LoadMessage(MessageLocator),
    /// Fetch one draft copy from the Drafts mailbox to reopen it in the
    /// composer (plan §14): same backend call as `LoadMessage`, different
    /// consumer — the fetched message becomes a `Draft` instead of a
    /// reader document.
    OpenDraft(MessageLocator),
    /// Fetch one full message purely for the list's faded body preview
    /// (ticket wxtx): background work that fills `MessageSummary.snippet`
    /// and the message cache. Independent of every other operation — no
    /// supersession — and never surfaced: failures are logged only.
    Preview(MessageLocator),
    /// Fetch one full message from the list to seed a composer draft
    /// (reply/reply-all/forward from NORMAL, user request): the same
    /// read path as `LoadMessage`, and the reducer turns the fetched
    /// copy into a seeded draft per `seed`. Supersedes its own kind for
    /// the same locator: only the newest intent may open the composer.
    SeedComposer {
        locator: MessageLocator,
        kind: SeedKind,
    },
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
    /// `Sent` is the tmail-send cleanup (ADR 0002: best-effort — delivery
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
    /// Validate one composer attachment source (plan §15, Phase 8): the
    /// path as typed (`~` unexpanded); the backend expands and checks it.
    /// No bytes travel — only the resulting metadata.
    ReadAttachment { path: std::path::PathBuf },
    /// List one directory for the attachment file chooser (plan §15,
    /// ticket 95x0): builds the explorer state for the target directory.
    /// `None` opens the chooser in the user's home directory (fallback:
    /// the working directory). Filesystem work in the manager keeps the
    /// reducer I/O-free.
    ListAttachmentFiles { path: Option<std::path::PathBuf> },
    /// Save one incoming attachment to disk (plan §15, Phase 8.4). The
    /// request freezes the target message, part, name, and directory;
    /// retries replay it verbatim. `open_after` chains the platform
    /// opener on the saved path (Phase 8.5).
    SaveAttachment {
        request: crate::domain::AttachmentRequest,
        open_after: bool,
    },
    /// Open a saved file with the platform handler (`open`/`xdg-open`,
    /// plan §15, Phase 8.5): spawned directly, never through a shell.
    OpenPath { path: std::path::PathBuf },
    /// Open a link from an HTML body in the platform browser (ticket
    /// hc9n): spawned directly, never through a shell, and only for the
    /// web schemes the opener policy accepts.
    OpenUrl { url: String },
    /// Hand the draft body to the configured external editor (plan §14,
    /// Phase 11): the runtime suspends the TUI, spawns `program` (argv
    /// only, no shell) on a secure temporary file, and waits for exit.
    /// Boxed strings keep the variant small.
    EditExternally { program: Vec<String>, body: String },
    /// Discover IMAP/SMTP settings for an email address with the
    /// io-pim-discovery adapter (ADR 0003 §3.3). Runs on a worker thread,
    /// bounded by the adapter's deadline; POP/JMAP results never appear.
    DiscoverConfig { email: String },
    /// Validate the wizard's draft account with a real `himalaya mailbox
    /// list` against a temporary 0600 config file (ADR 0003 §3.4). The
    /// real config is untouched; the temp file is deleted in all
    /// outcomes. Boxed: the draft carries the credentials.
    TestAccount {
        draft: Box<crate::app::wizard::DraftAccountConfig>,
    },
    /// Merge the confirmed draft account into the resolved config file
    /// (ADR 0003 §3.6): format-preserving toml_edit edit, fresh files
    /// created 0600. Runs in the manager (file I/O) so the reducer stays
    /// I/O-free.
    SaveAccount {
        path: std::path::PathBuf,
        draft: Box<crate::app::wizard::DraftAccountConfig>,
    },
    /// Deliver one new-mail notification (`[tmail].notifications`, ticket
    /// b28p). No mail travels; the manager delivers it without blocking
    /// the UI loop (the desktop path on the blocking pool).
    Notify { request: NotifyRequest },
    /// Serve one cached page of summaries (ticket haeb, off-thread I/O):
    /// a cache hit renders the rows instantly and a fresh load follows
    /// (background when `fresh_background_on_hit`, foreground otherwise);
    /// a miss starts the fresh load in the foreground. Runs on the
    /// blocking pool in the manager — the reducer never touches disk.
    CacheListLoad {
        mailbox: MailboxId,
        query: Option<String>,
        offset: usize,
        limit: usize,
        fresh_background_on_hit: bool,
    },
    /// Persist one page of summaries (ticket haeb, off-thread I/O). A
    /// store result carries nothing to apply: the cache is an
    /// optimization, never a source of truth.
    CacheListStore {
        mailbox: MailboxId,
        query: Option<String>,
        page: Box<Page<MessageSummary>>,
    },
    /// Drop one cached page (ticket kkaq): after a confirmed move the
    /// stored copy lists a message that left the mailbox, and the local
    /// post-move page cannot be stored truthfully (backend ids shift, so
    /// the follow-up re-sync owns the next write). Evicting makes a warm
    /// start re-fetch instead of resurrecting the moved row. The file name
    /// carries no limit, so one identity (mailbox + query + offset)
    /// evicts every limit variant.
    CacheListEvict {
        mailbox: MailboxId,
        query: Option<String>,
        offset: usize,
    },
    /// Serve the cached mailbox listing (ticket haeb, off-thread I/O): a
    /// hit renders the sidebar instantly and the fresh listing still
    /// loads in the background.
    CacheMailboxesLoad,
    /// Persist the mailbox listing (ticket haeb, off-thread I/O).
    CacheMailboxesStore { mailboxes: Vec<Mailbox> },
    /// Serve one cached full message for the reader (ticket haeb,
    /// off-thread I/O): a hit renders the body instantly and a silent
    /// background convergence fetch follows; a miss keeps the spinner and
    /// loads in the foreground.
    CacheMessageLoad { locator: MessageLocator },
    /// Serve one cached full message for a list preview (ticket wxtx,
    /// off-thread I/O): a hit fills the row's snippet without any fetch;
    /// a miss may start a background preview fetch within the rolling
    /// window.
    CachePreviewLoad { locator: MessageLocator },
    /// Persist one full message (ticket haeb, off-thread I/O). Best
    /// effort: the result carries nothing to apply.
    CacheMessageStore {
        mailbox: MailboxId,
        id: String,
        message: Box<Message>,
    },
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

/// Why a draft is being removed (Phase 7.6): it selects the failure UX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftRemovalReason {
    /// Confirmed discard (plan §14): failures open Retry/Dismiss.
    Discard,
    /// Tmail-send cleanup (ADR 0002 consequences): best-effort, logged only.
    Sent,
}

/// Which draft a list-initiated [`OperationKind::SeedComposer`] opens
/// (user request): the reply variants seed from the fetched message's
/// headers/body the same way the reader's reply does; forward quotes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedKind {
    /// Reply to the sender.
    Reply,
    /// Reply to sender and every recipient (the account address excluded,
    /// plan §14 Phase 7.5).
    ReplyAll,
    /// Forward the message unseeded of recipients.
    Forward,
}

impl OperationKind {
    /// Human-readable label for the status bar and error modal.
    pub fn summary(&self) -> &'static str {
        match self {
            OperationKind::LoadMailboxes => "Loading mailboxes",
            OperationKind::LoadPage(_) => "Loading messages",
            OperationKind::Search(_) => "Searching",
            OperationKind::LoadMessage(_) => "Loading message",
            OperationKind::OpenDraft(_) => "Opening draft",
            OperationKind::Preview(_) => "Fetching preview",
            OperationKind::SeedComposer { kind, .. } => match kind {
                SeedKind::Reply => "Replying",
                SeedKind::ReplyAll => "Replying all",
                SeedKind::Forward => "Forwarding",
            },
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
            OperationKind::ReadAttachment { .. } => "Checking file",
            OperationKind::ListAttachmentFiles { .. } => "Listing files",
            OperationKind::SaveAttachment { .. } => "Saving attachment",
            OperationKind::OpenPath { .. } => "Opening attachment",
            OperationKind::OpenUrl { .. } => "Opening link",
            OperationKind::EditExternally { .. } => "Editing externally",
            OperationKind::DiscoverConfig { .. } => "Detecting settings",
            OperationKind::TestAccount { .. } => "Testing account",
            OperationKind::SaveAccount { .. } => "Saving account",
            OperationKind::Notify { .. } => "Notifying",
            OperationKind::CacheListLoad { .. }
            | OperationKind::CacheMailboxesLoad
            | OperationKind::CacheMessageLoad { .. }
            | OperationKind::CachePreviewLoad { .. } => "Reading cache",
            OperationKind::CacheListStore { .. }
            | OperationKind::CacheListEvict { .. }
            | OperationKind::CacheMailboxesStore { .. }
            | OperationKind::CacheMessageStore { .. } => "Caching",
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
            // A new search of the same mailbox replaces the previous run:
            // only the newest query's results can ever be shown.
            (OperationKind::Search(newer), OperationKind::Search(older)) => {
                newer.mailbox_id == older.mailbox_id
            }
            (OperationKind::LoadMessage(newer), OperationKind::LoadMessage(older)) => {
                newer.mailbox == older.mailbox
            }
            // Draft fetches likewise: only the newest Enter can win.
            (OperationKind::OpenDraft(newer), OperationKind::OpenDraft(older)) => {
                newer.mailbox == older.mailbox
            }
            // List-initiated reply/forward seeds: the last pressed key wins
            // (the composer is one-shot; an older fetch must not open it).
            (
                OperationKind::SeedComposer { locator: newer, .. },
                OperationKind::SeedComposer { locator: older, .. },
            ) => newer.id == older.id,
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
            // A repeated validation of the same entry supersedes the one in
            // flight: only the newest submit can win.
            (
                OperationKind::ReadAttachment { path: newer },
                OperationKind::ReadAttachment { path: older },
            ) => newer == older,
            // A newer directory listing supersedes the one in flight:
            // navigation keeps moving, only the newest target can land.
            (
                OperationKind::ListAttachmentFiles { path: newer },
                OperationKind::ListAttachmentFiles { path: older },
            ) => newer == older,
            // Wizard work supersedes its own kind: a re-run discovery (`r`)
            // or a retried credential test replaces the still-running
            // previous attempt (ADR 0003 §3.2).
            (
                OperationKind::DiscoverConfig { email: newer },
                OperationKind::DiscoverConfig { email: older },
            ) => newer == older,
            (OperationKind::TestAccount { .. }, OperationKind::TestAccount { .. }) => true,
            // Cache work supersedes its own identity: only the newest
            // read or write of a page/message/listing can matter (the
            // cache is advisory; a dropped older store just keeps the
            // previous copy on disk).
            (
                OperationKind::CacheListLoad {
                    mailbox: newer_mailbox,
                    query: newer_query,
                    ..
                },
                OperationKind::CacheListLoad {
                    mailbox: older_mailbox,
                    query: older_query,
                    ..
                },
            )
            | (
                OperationKind::CacheListStore {
                    mailbox: newer_mailbox,
                    query: newer_query,
                    ..
                },
                OperationKind::CacheListStore {
                    mailbox: older_mailbox,
                    query: older_query,
                    ..
                },
            )
            | (
                OperationKind::CacheListEvict {
                    mailbox: newer_mailbox,
                    query: newer_query,
                    ..
                },
                OperationKind::CacheListEvict {
                    mailbox: older_mailbox,
                    query: older_query,
                    ..
                },
            ) => newer_mailbox == older_mailbox && newer_query == older_query,
            (OperationKind::CacheMailboxesLoad, OperationKind::CacheMailboxesLoad)
            | (
                OperationKind::CacheMailboxesStore { .. },
                OperationKind::CacheMailboxesStore { .. },
            ) => true,
            (
                OperationKind::CacheMessageLoad { locator: newer },
                OperationKind::CacheMessageLoad { locator: older },
            )
            | (
                OperationKind::CachePreviewLoad { locator: newer },
                OperationKind::CachePreviewLoad { locator: older },
            ) => newer.mailbox == older.mailbox && newer.id == older.id,
            (
                OperationKind::CacheMessageStore {
                    mailbox: newer_mailbox,
                    id: newer_id,
                    ..
                },
                OperationKind::CacheMessageStore {
                    mailbox: older_mailbox,
                    id: older_id,
                    ..
                },
            ) => newer_mailbox == older_mailbox && newer_id == older_id,
            // Saves never supersede: a confirmed save must report exactly
            // what it wrote.
            // Sends never supersede anything and are never superseded:
            // every delivery attempt must run to its classified outcome.
            _ => false,
        }
    }

    /// Whether `Esc` may cancel this operation (plan §11: "cancel the
    /// currently foregrounded cancellable operation"). Sends and opens are
    /// never cancellable: killing himalaya mid-DATA leaves the delivery
    /// state unknown while the suppressed `Cancelled` result could claim
    /// neither failure nor success — exactly the ambiguity plan §12 forbids
    /// hiding (an already-spawned handler app, or an already-launched
    /// browser, is likewise let alone). The external editor is likewise
    /// untouchable: it owns the terminal and the body file until it exits
    /// (plan §14 step 5). The user can still leave the composer; the send
    /// completes (or is classified) in the background.
    pub fn is_cancellable(&self) -> bool {
        !matches!(
            self,
            OperationKind::Send { .. }
                | OperationKind::OpenPath { .. }
                | OperationKind::OpenUrl { .. }
                | OperationKind::EditExternally { .. }
        )
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
    /// The wizard's credential test passed (ADR 0003 §3.4): the mailbox
    /// names the draft account can list.
    TestAccountCompleted {
        mailboxes: Vec<String>,
    },
    /// The wizard account was merged into the config file (ADR 0003
    /// §3.6): the file path, whether it was freshly created, and the
    /// permissions warning when the file was group/world-readable.
    AccountSaved {
        path: std::path::PathBuf,
        created: bool,
        permissions_warning: Option<String>,
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
            OperationKind::LoadPage(request) if &request.mailbox_id == mailbox_id => {
                Some(request.clone())
            }
            _ => None,
        })
    }

    /// The search request currently in flight for `mailbox_id`, if any
    /// (Phase 9). Superseding guarantees at most one.
    pub fn search_in_flight(&self, mailbox_id: &crate::domain::MailboxId) -> Option<SearchRequest> {
        self.entries.values().find_map(|op| match &op.kind {
            OperationKind::Search(request) if &request.mailbox_id == mailbox_id => {
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
            .filter(|op| matches!(op.kind, OperationKind::Preview(_)))
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

    #[test]
    fn cancel_all_fires_every_token_and_clears_the_registry() {
        // Ticket c0n0: the confirmed account switch cancels everything at
        // once — tokens fire so the children die, entries clear so late
        // results are rejected.
        let mut registry = OperationRegistry::default();
        let first = registry.start(page("inbox", 0));
        let second = registry.start(page("sent", 0));
        let third = registry.start(OperationKind::LoadMailboxes);
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
        registry.start_background(OperationKind::Preview(locator.clone()));
        registry.start_background(OperationKind::Preview(locator));
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
    use crate::domain::MailboxId;

    fn page(mailbox: &str, offset: usize) -> OperationKind {
        OperationKind::LoadPage(PageRequest {
            mailbox_id: MailboxId(String::from(mailbox)),
            offset,
            limit: 20,
        })
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
        let foreground = registry.start(OperationKind::LoadMailboxes);
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
        registry.start_background(OperationKind::Preview(locator.clone()));
        registry.start_background(OperationKind::Preview(locator));
        registry.start_background(page("inbox", 0));
        assert_eq!(registry.previews_in_flight(), 2);
        // And they do not block the foreground slot:
        assert!(!registry.has_foreground());
    }
}
