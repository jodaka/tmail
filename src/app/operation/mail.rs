//! Mail operations: mailbox/message reads and mutations (plan §19
//! Phase 4, tickets aavy/j9bq/wxtx). One family of the operation
//! vocabulary — see [`super::OperationKind`].

use crate::domain::{MessageLocator, PageRequest, SearchRequest};

/// Which draft a list-initiated [`MailOperation::SeedComposer`] opens
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

/// Fetch the mailbox listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailOperation {
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
    /// One batched read-flag change over the whole selection (ticket
    /// aavy): the backend applies every locator in a single call, so a
    /// bulk mark costs one IMAP session instead of one per message. The
    /// locators share the mailbox the bulk action was pressed in.
    SetReadBulk {
        locators: Vec<MessageLocator>,
        read: bool,
    },
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
    /// One batched archive of the whole selection (ticket j9bq): the
    /// backend moves every locator in a single call, so a bulk archive
    /// costs one IMAP session instead of one per message. Locators share
    /// the mailbox the bulk action was pressed in.
    ArchiveBulk(Vec<MessageLocator>),
    /// One batched trash of the whole selection (ticket j9bq): see
    /// [`MailOperation::ArchiveBulk`]; himalaya stays trash-first.
    TrashBulk(Vec<MessageLocator>),
}

impl MailOperation {
    /// Human-readable label for the status bar and error modal.
    pub(crate) fn summary(&self) -> &'static str {
        match self {
            MailOperation::LoadMailboxes => "Loading mailboxes",
            MailOperation::LoadPage(_) => "Loading messages",
            MailOperation::Search(_) => "Searching",
            MailOperation::LoadMessage(_) => "Loading message",
            MailOperation::OpenDraft(_) => "Opening draft",
            MailOperation::Preview(_) => "Fetching preview",
            MailOperation::SeedComposer { kind, .. } => match kind {
                SeedKind::Reply => "Replying",
                SeedKind::ReplyAll => "Replying all",
                SeedKind::Forward => "Forwarding",
            },
            MailOperation::SetRead { read: true, .. } => "Marking read",
            MailOperation::SetRead { read: false, .. } => "Marking unread",
            MailOperation::SetReadBulk { read: true, .. } => "Marking read",
            MailOperation::SetReadBulk { read: false, .. } => "Marking unread",
            MailOperation::SetStarred { starred: true, .. } => "Starring",
            MailOperation::SetStarred { starred: false, .. } => "Unstarring",
            MailOperation::Archive(_) => "Archiving",
            MailOperation::Trash(_) => "Moving to trash",
            MailOperation::ArchiveBulk(_) => "Archiving",
            MailOperation::TrashBulk(_) => "Moving to trash",
        }
    }

    /// Whether `newer` supersedes `older` (plan §11): mailbox loads
    /// supersede each other; page loads supersede page loads for the same
    /// mailbox; message loads supersede the same message across mailboxes
    /// (only the newest opened message can win); repeated flag toggles on
    /// the same message supersede each other. Mutations that move mail
    /// never supersede — a lost archive would be unrecoverable from
    /// state. (They instead coalesce: see
    /// [`MailOperation::duplicates_of`].)
    pub(crate) fn supersedes(newer: &MailOperation, older: &MailOperation) -> bool {
        match (newer, older) {
            (MailOperation::LoadMailboxes, MailOperation::LoadMailboxes) => true,
            (MailOperation::LoadPage(newer), MailOperation::LoadPage(older)) => {
                newer.mailbox_id == older.mailbox_id
            }
            // A new search of the same mailbox replaces the previous run:
            // only the newest query's results can ever be shown. Same-query
            // pagination *keeps* the older page (different offset: both
            // pages may be wanted), and the apply-side currency check
            // (`complete_search`) still guards by mailbox + query, so a
            // superseded or stale page never lands on the wrong state.
            (MailOperation::Search(newer), MailOperation::Search(older)) => {
                newer.mailbox_id == older.mailbox_id
            }
            (MailOperation::LoadMessage(newer), MailOperation::LoadMessage(older)) => {
                // Same message, even across mailboxes (the listed id may
                // also exist elsewhere): only the newest fetch can win,
                // so the registry keeps one child per message.
                newer.id == older.id
            }
            // Draft fetches likewise: only the newest Enter can win.
            (MailOperation::OpenDraft(newer), MailOperation::OpenDraft(older)) => {
                newer.mailbox == older.mailbox
            }
            // List-initiated reply/forward seeds: the last pressed key wins
            // (the composer is one-shot; an older fetch must not open it).
            (
                MailOperation::SeedComposer { locator: newer, .. },
                MailOperation::SeedComposer { locator: older, .. },
            ) => newer.id == older.id,
            (
                MailOperation::SetRead { locator: newer, .. },
                MailOperation::SetRead { locator: older, .. },
            )
            | (
                MailOperation::SetStarred { locator: newer, .. },
                MailOperation::SetStarred { locator: older, .. },
            ) => newer.id == older.id,
            _ => false,
        }
    }

    /// Whether `self` is a *duplicate* of an in-flight `older` operation:
    /// the identical intent on the identical target. Only the move
    /// mutations (Archive/Trash of one message) qualify: a double-press
    /// spawns two concurrent backend moves for the same id, and after the
    /// first one renames the maildir file the second fails confusingly
    /// against the new id — the registry drops the duplicate instead
    /// (idempotency for the user: the first press already owns the work).
    /// The bulk shapes coalesce the same way (ticket j9bq): a
    /// double-press of one bulk archive/trash must not re-run the backend
    /// move for messages whose ids already changed under the first run.
    pub(crate) fn duplicates_of(&self, older: &MailOperation) -> bool {
        match (self, older) {
            (MailOperation::Archive(newer), MailOperation::Archive(older))
            | (MailOperation::Trash(newer), MailOperation::Trash(older)) => newer == older,
            (MailOperation::ArchiveBulk(newer), MailOperation::ArchiveBulk(older))
            | (MailOperation::TrashBulk(newer), MailOperation::TrashBulk(older)) => newer == older,
            _ => false,
        }
    }

    /// Whether `Esc` may cancel the operation (plan §11): every mail
    /// operation is cancellable — killing the child loses at most a fetch
    /// or a move the user can retry.
    pub(crate) fn is_cancellable(&self) -> bool {
        true
    }

    /// Whether the dispatch runs a mail-backend child process and
    /// therefore queues behind the bounded permit pool (issue 1v38).
    pub(crate) fn uses_backend_process(&self) -> bool {
        true
    }
}
