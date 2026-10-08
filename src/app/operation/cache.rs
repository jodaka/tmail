//! Cache operations (ticket haeb, off-thread I/O): serve and persist the
//! summary/message/mailbox caches. The cache lives in the operation
//! manager; the reducer never touches disk. One family of the operation
//! vocabulary — see [`super::OperationKind`].

use std::sync::Arc;

use crate::domain::{Mailbox, MailboxId, Message, MessageLocator, MessageSummary, Page};

/// The cache operation to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheOperation {
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
    /// optimization, never a source of truth. Shared (`Arc`, issue
    /// cbkz): the store path (effect -> manager -> serialization)
    /// travels by reference count, never a deep page copy.
    CacheListStore {
        mailbox: MailboxId,
        query: Option<String>,
        page: Arc<Page<MessageSummary>>,
    },
    /// Drop cached pages (ticket kkaq): after a confirmed move the
    /// stored copy lists a message that left the mailbox, and the local
    /// post-move page cannot be stored truthfully (backend ids shift, so
    /// the follow-up re-sync owns the next write). Evicting makes a warm
    /// start re-fetch instead of resurrecting the moved row. The file
    /// name carries no limit, so one identity (mailbox + query + offset)
    /// evicts every limit variant. `Some(offset)` names the one stored
    /// page under `query`; `None` sweeps every stored page of the
    /// mailbox, all query namespaces included (review 14): a confirmed
    /// move evicts per *source* mailbox — taken from the locators — and
    /// a mailbox switch or page change after the move started means the
    /// reducer can no longer name the offsets (or the search-query
    /// namespace) that held the moved row.
    CacheListEvict {
        mailbox: MailboxId,
        query: Option<String>,
        offset: Option<usize>,
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
    /// effort: the result carries nothing to apply. Shared (`Arc`, ticket
    /// pa64): the effect must not deep-copy a possibly multi-megabyte body
    /// on its way to the store.
    CacheMessageStore {
        mailbox: MailboxId,
        id: String,
        message: Arc<Message>,
    },
}

impl CacheOperation {
    /// Human-readable label for the status bar and error modal: reads
    /// announce themselves, writes/evictions collapse into one label.
    pub(crate) fn summary(&self) -> &'static str {
        match self {
            CacheOperation::CacheListLoad { .. }
            | CacheOperation::CacheMailboxesLoad
            | CacheOperation::CacheMessageLoad { .. }
            | CacheOperation::CachePreviewLoad { .. } => "Reading cache",
            CacheOperation::CacheListStore { .. }
            | CacheOperation::CacheListEvict { .. }
            | CacheOperation::CacheMailboxesStore { .. }
            | CacheOperation::CacheMessageStore { .. } => "Caching",
        }
    }

    /// Whether `newer` supersedes `older` (plan §11): cache work
    /// supersedes its own identity — only the newest read or write of a
    /// page/message/listing can matter (the cache is advisory; a dropped
    /// older store just keeps the previous copy on disk).
    pub(crate) fn supersedes(newer: &CacheOperation, older: &CacheOperation) -> bool {
        match (newer, older) {
            (
                CacheOperation::CacheListLoad {
                    mailbox: newer_mailbox,
                    query: newer_query,
                    ..
                },
                CacheOperation::CacheListLoad {
                    mailbox: older_mailbox,
                    query: older_query,
                    ..
                },
            )
            | (
                CacheOperation::CacheListStore {
                    mailbox: newer_mailbox,
                    query: newer_query,
                    ..
                },
                CacheOperation::CacheListStore {
                    mailbox: older_mailbox,
                    query: older_query,
                    ..
                },
            ) => newer_mailbox == older_mailbox && newer_query == older_query,
            (
                CacheOperation::CacheListEvict {
                    mailbox: newer_mailbox,
                    query: newer_query,
                    offset: newer_offset,
                },
                CacheOperation::CacheListEvict {
                    mailbox: older_mailbox,
                    query: older_query,
                    offset: older_offset,
                },
            ) => {
                newer_mailbox == older_mailbox
                    && match (newer_offset, older_offset) {
                        // A mailbox-wide sweep covers every query and
                        // offset; a named page never supersedes it (the
                        // sweep may be the only one that would have
                        // removed the other files).
                        (Some(_), None) => false,
                        // The sweep supersedes anything for the mailbox.
                        (None, _) => true,
                        (Some(newer), Some(older)) => newer == older && newer_query == older_query,
                    }
            }
            (CacheOperation::CacheMailboxesLoad, CacheOperation::CacheMailboxesLoad)
            | (
                CacheOperation::CacheMailboxesStore { .. },
                CacheOperation::CacheMailboxesStore { .. },
            ) => true,
            (
                CacheOperation::CacheMessageLoad { locator: newer },
                CacheOperation::CacheMessageLoad { locator: older },
            )
            | (
                CacheOperation::CachePreviewLoad { locator: newer },
                CacheOperation::CachePreviewLoad { locator: older },
            ) => newer.mailbox == older.mailbox && newer.id == older.id,
            (
                CacheOperation::CacheMessageStore {
                    mailbox: newer_mailbox,
                    id: newer_id,
                    ..
                },
                CacheOperation::CacheMessageStore {
                    mailbox: older_mailbox,
                    id: older_id,
                    ..
                },
            ) => newer_mailbox == older_mailbox && newer_id == older_id,
            _ => false,
        }
    }

    /// No cache operation coalesces duplicates.
    pub(crate) fn duplicates_of(&self, _older: &CacheOperation) -> bool {
        false
    }

    /// Whether `Esc` may cancel the operation (plan §11): yes — a broken
    /// or cancelled cache read degrades to a miss, never to wrong mail.
    pub(crate) fn is_cancellable(&self) -> bool {
        true
    }

    /// No cache operation runs a mail-backend child (issue 1v38): all of
    /// it is local file I/O on the blocking pool.
    pub(crate) fn uses_backend_process(&self) -> bool {
        false
    }
}
