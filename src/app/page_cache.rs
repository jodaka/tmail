//! Post-owned summary cache (ticket haeb, cache.md §2): the last loaded
//! page of message summaries per mailbox, persisted so warm starts and
//! mailbox switches render instantly and refresh in the background.
//!
//! Conservative by design: a page is overwritten by every successful load
//! (never written on failures), a read is validated against the identity
//! recorded inside the file (mailbox, offset, limit, query), and anything
//! unparsable or mismatched is ignored — a stale cache degrades to today's
//! spinner, never to wrong mail. Summaries contain no credentials (plan
//! §21: nothing secret is stored or logged).
//!
//! Only the pages the reducer asks for are stored, and per-mailbox storage
//! is bounded: the oldest files beyond [`MAX_FILES_PER_MAILBOX`] are
//! pruned after each write.

use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::{MailboxId, MessageSummary, Page};

/// Version of the on-disk format; bumping it invalidates old caches.
const CACHE_VERSION: u32 = 1;

/// The on-disk shape: the page payload plus the identity it must match.
#[derive(Serialize, Deserialize)]
struct CachedPage {
    version: u32,
    mailbox: String,
    query: Option<String>,
    offset: usize,
    limit: usize,
    total: Option<usize>,
    items: Vec<MessageSummary>,
}

/// Upper bound on stored pages per mailbox (offset/limit/query variants
/// share the quota; the oldest by modification time go first).
const MAX_FILES_PER_MAILBOX: usize = 8;

/// A `mailbox`/`query` string reduced to a filesystem-safe key: safe
/// characters kept (bounded), everything else (including `/` in maildir
/// absolute ids) hashed in so distinct ids never collide.
fn key_part(value: &str) -> String {
    let safe: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(48)
        .collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{safe}-{:016x}", hasher.finish())
}

/// The summary cache rooted at a per-account directory. `None` disables
/// caching (unknown data dir) — every call site treats that as a miss.
#[derive(Debug, Clone)]
pub struct PageCache {
    root: PathBuf,
}

impl PageCache {
    /// Cache rooted at an explicit directory (tests, explicit wiring).
    pub fn open(root: PathBuf) -> Self {
        Self { root }
    }

    /// The default cache root, scoped to the driven account (or
    /// `"default"`): `$POST_DATA_DIR/cache/<account>` when set, else the
    /// platform user-data dir (`~/Library/Application Support/post/cache`
    /// on macOS, `~/.local/share/post/cache` elsewhere). `None` when no
    /// home is known — caching stays off.
    pub fn open_default(account: Option<&str>) -> Option<Self> {
        if let Some(dir) = std::env::var_os("POST_DATA_DIR") {
            return Some(Self::scoped(PathBuf::from(dir).join("cache"), account));
        }
        let home = std::env::var_os("HOME")?;
        let mut dir = PathBuf::from(home);
        dir.push(if cfg!(target_os = "macos") {
            "Library/Application Support"
        } else {
            ".local/share"
        });
        dir.push("post");
        dir.push("cache");
        Some(Self::scoped(dir, account))
    }

    /// Root plus the account scope segment.
    fn scoped(root: PathBuf, account: Option<&str>) -> Self {
        Self {
            root: root.join(key_part(account.unwrap_or("default"))),
        }
    }

    /// Where a page lives on disk.
    fn path(
        &self,
        mailbox: &MailboxId,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> PathBuf {
        let query_key = query.map_or_else(String::new, key_part);
        self.root
            .join(key_part(&mailbox.0))
            .join(format!("{query_key}-{offset:06}-{limit}.json"))
    }

    /// Load the cached page, or `None` when absent, unparsable, stale
    /// (older format version), or for a different identity. A read never
    /// fails the caller.
    pub fn load(
        &self,
        mailbox: &MailboxId,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Option<Page<MessageSummary>> {
        let bytes = fs::read(self.path(mailbox, query, offset, limit)).ok()?;
        let cached: CachedPage = serde_json::from_slice(&bytes).ok()?;
        if cached.version != CACHE_VERSION
            || cached.mailbox != mailbox.0
            || cached.query.as_deref() != query
            || cached.offset != offset
            || cached.limit != limit
        {
            return None;
        }
        Some(Page {
            items: cached.items,
            offset: cached.offset,
            limit: cached.limit,
            total: cached.total,
        })
    }

    /// Persist a successful page. Failures are logged and swallowed: the
    /// cache is an optimization, never a source of truth.
    pub fn store(&self, mailbox: &MailboxId, query: Option<&str>, page: &Page<MessageSummary>) {
        let path = self.path(mailbox, query, page.offset, page.limit);
        let cached = CachedPage {
            version: CACHE_VERSION,
            mailbox: mailbox.0.clone(),
            query: query.map(str::to_owned),
            offset: page.offset,
            limit: page.limit,
            total: page.total,
            items: page.items.clone(),
        };
        if let Some(parent) = path.parent()
            && let Err(err) = fs::create_dir_all(parent)
        {
            tracing::debug!(%err, "summary cache: mkdir failed");
            return;
        }
        // Atomic-ish: write to a sibling temp name, then rename over.
        let tmp = path.with_extension("json.tmp");
        let payload = match serde_json::to_vec(&cached) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::debug!(%err, "summary cache: serialize failed");
                return;
            }
        };
        if fs::write(&tmp, &payload)
            .and_then(|()| fs::rename(&tmp, &path))
            .is_err()
        {
            tracing::debug!(path = %path.display(), "summary cache: write failed");
        }
        self.prune(mailbox);
    }

    /// Keep at most [`MAX_FILES_PER_MAILBOX`] files for this mailbox,
    /// dropping the oldest modifications first.
    fn prune(&self, mailbox: &MailboxId) {
        let dir = self.root.join(key_part(&mailbox.0));
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| {
                let modified = fs::metadata(&path).and_then(|meta| meta.modified()).ok()?;
                Some((modified, path))
            })
            .collect();
        if files.len() <= MAX_FILES_PER_MAILBOX {
            return;
        }
        files.sort_by_key(|(modified, _)| *modified);
        let excess = files.len() - MAX_FILES_PER_MAILBOX;
        for (_, path) in files.into_iter().take(excess) {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MessageId;

    fn summary(id: &str, subject: &str) -> MessageSummary {
        MessageSummary {
            id: MessageId(String::from(id)),
            mailbox_id: MailboxId(String::from("/root/maildir/INBOX")),
            message_id: Some(String::from("<1@post.local>")),
            from: Vec::new(),
            to: Vec::new(),
            subject: String::from(subject),
            snippet: None,
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:00+00:00")
                .expect("fixed ts"),
            is_read: false,
            is_starred: false,
            has_attachments: false,
        }
    }

    fn page(offset: usize) -> Page<MessageSummary> {
        Page {
            items: vec![summary("m1", "Hello"), summary("m2", "There")],
            offset,
            limit: 20,
            total: None,
        }
    }

    #[test]
    fn store_then_load_round_trips_the_page() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf());
        let mailbox = MailboxId(String::from("/root/maildir/INBOX"));

        assert!(cache.load(&mailbox, None, 0, 20).is_none(), "empty cache");
        cache.store(&mailbox, None, &page(0));
        let loaded = cache.load(&mailbox, None, 0, 20).expect("cached page");
        assert_eq!(loaded.items.len(), 2);
        assert_eq!(loaded.items[0].subject, "Hello");
        assert_eq!(loaded.offset, 0);
        assert_eq!(loaded.limit, 20);
    }

    #[test]
    fn a_page_for_one_identity_is_not_served_for_another() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf());
        let inbox = MailboxId(String::from("/root/maildir/INBOX"));
        let archive = MailboxId(String::from("/root/maildir/Archive"));

        cache.store(&inbox, None, &page(0));
        assert!(cache.load(&archive, None, 0, 20).is_none());
        assert!(cache.load(&inbox, Some("hello"), 0, 20).is_none());
        assert!(cache.load(&inbox, None, 20, 20).is_none());
        // Same identity loads again.
        assert!(cache.load(&inbox, None, 0, 20).is_some());
    }

    #[test]
    fn maildir_ids_with_slashes_stay_distinct_keys() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf());
        let a = MailboxId(String::from("/root/maildir/INBOX"));
        let b = MailboxId(String::from("/root/maildir/INBOX/sub"));

        cache.store(&a, None, &page(0));
        assert!(cache.load(&b, None, 0, 20).is_none(), "no cross-hit");
        cache.store(&b, None, &page(0));
        assert!(cache.load(&a, None, 0, 20).is_some());
        assert!(cache.load(&b, None, 0, 20).is_some());
    }

    #[test]
    fn unparsable_files_are_ignored() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf());
        let mailbox = MailboxId(String::from("inbox"));
        cache.store(&mailbox, None, &page(0));
        // Corrupt the file in place.
        let path = cache.path(&mailbox, None, 0, 20);
        std::fs::write(&path, b"not json at all").expect("corrupt");
        assert!(cache.load(&mailbox, None, 0, 20).is_none());
    }

    #[test]
    fn storage_is_bounded_per_mailbox() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf());
        let mailbox = MailboxId(String::from("inbox"));
        // Far more distinct pages than the cap.
        for offset in (0..MAX_FILES_PER_MAILBOX * 3).step_by(20) {
            cache.store(&mailbox, None, &page(offset));
        }
        let dir_entries = fs::read_dir(dir.path().join(key_part(&mailbox.0))).expect("dir");
        let count = dir_entries.count();
        assert!(
            count <= MAX_FILES_PER_MAILBOX,
            "{count} files exceeds the bound"
        );
    }
}
