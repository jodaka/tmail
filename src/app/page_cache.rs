//! Tmail-owned cache (ticket haeb, cache.md §2): the last loaded page of
//! message summaries per mailbox, the mailbox listing, and viewed full
//! messages — persisted so warm starts and mailbox switches render
//! instantly and refresh in the background.
//!
//! Conservative by design: an entry is overwritten by every successful
//! load (never written on failures), a read is validated against the
//! identity recorded inside the file (mailbox, offset, limit, query /
//! message id), and anything unparsable or mismatched is ignored — a
//! stale cache degrades to today's spinner, never to wrong mail.
//! Summaries and rendered messages contain no credentials (plan §21:
//! nothing secret is stored or logged).
//!
//! Storage is bounded: per-mailbox pages are capped at
//! [`MAX_FILES_PER_MAILBOX`] files; the viewed-message cache is capped by
//! [`CacheLimits`] (entry count and total bytes, from `[tmail.cache]`).
//! The oldest modifications are evicted first.

use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::{Mailbox, MailboxId, Message, MessageSummary, Page};

/// Version of the on-disk format; bumping it invalidates old caches.
/// 2: attachment `part_id` became the 1-based id `attachment download`
/// expects; v1 entries cache the old 0-based index and must not be served.
const CACHE_VERSION: u32 = 2;

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

/// The on-disk mailbox listing (ticket haeb: instant start needs the
/// sidebar before the first backend round trip).
#[derive(Serialize, Deserialize)]
struct CachedMailboxes {
    version: u32,
    mailboxes: Vec<Mailbox>,
}

/// The on-disk viewed message, keyed by its locator. `last_used_ms`
/// drives least-recently-used eviction (ticket haeb).
#[derive(Serialize, Deserialize)]
struct CachedMessage {
    version: u32,
    mailbox: String,
    id: String,
    last_used_ms: u64,
    message: Message,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Upper bound on stored pages per mailbox (offset/limit/query variants
/// share the quota; the oldest by modification time go first).
const MAX_FILES_PER_MAILBOX: usize = 8;

/// Caps for the viewed-message cache (ticket haeb): entries and total
/// bytes, both configurable via `[tmail.cache]`. When either is exceeded,
/// the oldest modifications are evicted first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLimits {
    /// Maximum number of cached viewed messages; `0` disables message
    /// caching entirely.
    pub max_messages: usize,
    /// Maximum total size of the viewed-message cache in bytes.
    pub max_bytes: u64,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_messages: crate::config::DEFAULT_CACHE_MAX_MESSAGES,
            // 10 MiB: roughly hundreds of typical mail bodies.
            max_bytes: crate::config::DEFAULT_CACHE_MAX_BYTES,
        }
    }
}

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

/// The summary + mailbox + message cache rooted at a per-account
/// directory. `None` disables caching (unknown data dir) — every call
/// site treats that as a miss.
#[derive(Debug, Clone)]
pub struct PageCache {
    root: PathBuf,
    limits: CacheLimits,
}

impl PageCache {
    /// Cache rooted at an explicit directory (tests, explicit wiring).
    pub fn open(root: PathBuf, limits: CacheLimits) -> Self {
        Self { root, limits }
    }

    /// The default cache root, scoped to the driven account (or
    /// `"default"`): `$TMAIL_DATA_DIR/cache/<account>` when set, else the
    /// platform user-data dir (`~/Library/Application Support/tmail/cache`
    /// on macOS, `~/.local/share/tmail/cache` elsewhere). `None` when no
    /// home is known — caching stays off.
    pub fn open_default(account: Option<&str>, limits: CacheLimits) -> Option<Self> {
        if let Some(dir) = std::env::var_os("TMAIL_DATA_DIR") {
            return Some(Self::scoped(
                PathBuf::from(dir).join("cache"),
                account,
                limits,
            ));
        }
        let home = std::env::var_os("HOME")?;
        let mut dir = PathBuf::from(home);
        dir.push(if cfg!(target_os = "macos") {
            "Library/Application Support"
        } else {
            ".local/share"
        });
        dir.push("tmail");
        dir.push("cache");
        Some(Self::scoped(dir, account, limits))
    }

    /// Root plus the account scope segment.
    fn scoped(root: PathBuf, account: Option<&str>, limits: CacheLimits) -> Self {
        Self {
            root: root.join(key_part(account.unwrap_or("default"))),
            limits,
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
        self.write_json(&path, &cached);
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

    // ── Mailbox listing (ticket haeb: instant start) ────────────────────

    /// Load the cached mailbox listing, or `None` when absent/unparsable.
    pub fn load_mailboxes(&self) -> Option<Vec<Mailbox>> {
        let bytes = fs::read(self.root.join("mailboxes.json")).ok()?;
        let cached: CachedMailboxes = serde_json::from_slice(&bytes).ok()?;
        (cached.version == CACHE_VERSION).then_some(cached.mailboxes)
    }

    /// Persist the mailbox listing after a successful load.
    pub fn store_mailboxes(&self, mailboxes: &[Mailbox]) {
        let cached = CachedMailboxes {
            version: CACHE_VERSION,
            mailboxes: mailboxes.to_vec(),
        };
        self.write_json(&self.root.join("mailboxes.json"), &cached);
    }

    // ── Viewed messages (ticket haeb: instant reader) ───────────────────

    /// Where a viewed message lives on disk.
    fn message_path(&self, mailbox: &MailboxId, id: &str) -> PathBuf {
        self.root
            .join("messages")
            .join(key_part(&mailbox.0))
            .join(format!("{}.json", key_part(id)))
    }

    /// Load a cached viewed message, or `None` when absent/unparsable/
    /// for a different identity. A hit refreshes the entry's LRU stamp.
    pub fn load_message(&self, mailbox: &MailboxId, id: &str) -> Option<Message> {
        let path = self.message_path(mailbox, id);
        let bytes = fs::read(&path).ok()?;
        let mut cached: CachedMessage = serde_json::from_slice(&bytes).ok()?;
        if cached.version != CACHE_VERSION || cached.mailbox != mailbox.0 || cached.id != id {
            return None;
        }
        // LRU bookkeeping: rewrite with a fresh stamp so recently viewed
        // messages outlive older ones under the size caps.
        cached.last_used_ms = now_ms();
        self.write_json(&path, &cached);
        Some(cached.message)
    }

    /// Persist a viewed message after a successful load, enforcing the
    /// configured limits: messages larger than the total byte budget are
    /// skipped entirely; otherwise the least-recently-used entries beyond
    /// the caps are evicted.
    pub fn store_message(&self, mailbox: &MailboxId, id: &str, message: &Message) {
        if self.limits.max_messages == 0 {
            return;
        }
        let path = self.message_path(mailbox, id);
        let cached = CachedMessage {
            version: CACHE_VERSION,
            mailbox: mailbox.0.clone(),
            id: String::from(id),
            last_used_ms: now_ms(),
            message: message.clone(),
        };
        let payload = match serde_json::to_vec(&cached) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::debug!(%err, "message cache: serialize failed");
                return;
            }
        };
        if payload.len() as u64 > self.limits.max_bytes {
            tracing::debug!(
                bytes = payload.len(),
                "message cache: message exceeds the total byte budget; not cached"
            );
            return;
        }
        self.write_bytes(&path, &payload);
        self.prune_messages();
    }

    /// Enforce [`CacheLimits`] across all mailboxes' viewed messages
    /// (`<root>/messages/<mailbox>/<id>.json`): evict the
    /// least-recently-used entries until the entry count and total byte
    /// size fit. Entries without a parsable LRU stamp are evicted first.
    fn prune_messages(&self) {
        let dir = self.root.join("messages");
        let Ok(mailbox_dirs) = fs::read_dir(&dir) else {
            return;
        };
        let mut files: Vec<(u64, PathBuf, u64)> = mailbox_dirs
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|entry| fs::read_dir(entry.path()).ok())
            .flatten()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .map(|path| {
                let stamp = fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<CachedMessage>(&bytes).ok())
                    .map(|cached| cached.last_used_ms)
                    .unwrap_or(0);
                let len = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                (stamp, path, len)
            })
            .collect();
        // Least recently used first.
        files.sort_by(|(astamp, apath, _), (bstamp, bpath, _)| {
            astamp.cmp(bstamp).then_with(|| apath.cmp(bpath))
        });
        let mut total: u64 = files.iter().map(|(_, _, len)| *len).sum();
        let mut remaining = files.len();
        for (_, path, len) in &files {
            if remaining <= self.limits.max_messages && total <= self.limits.max_bytes {
                break;
            }
            if fs::remove_file(path).is_ok() {
                remaining -= 1;
                total -= len;
            }
        }
    }

    /// Best-effort atomic JSON write: sibling temp file, then rename.
    fn write_json<T: serde::Serialize>(&self, path: &Path, value: &T) {
        let payload = match serde_json::to_vec(value) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::debug!(%err, "cache: serialize failed");
                return;
            }
        };
        self.write_bytes(path, &payload);
    }

    fn write_bytes(&self, path: &Path, payload: &[u8]) {
        if let Some(parent) = path.parent()
            && let Err(err) = fs::create_dir_all(parent)
        {
            tracing::debug!(%err, "cache: mkdir failed");
            return;
        }
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, payload)
            .and_then(|()| fs::rename(&tmp, path))
            .is_err()
        {
            tracing::debug!(path = %path.display(), "cache: write failed");
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
            message_id: Some(String::from("<1@tmail.local>")),
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
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
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
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
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
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
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
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
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
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
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

#[cfg(test)]
mod mailbox_message_tests {
    use super::*;
    use crate::domain::{MailboxRole, MessageId};

    fn mailbox(name: &str) -> Mailbox {
        Mailbox {
            id: MailboxId(String::from("/root/maildir/INBOX")),
            name: String::from(name),
            role: Some(MailboxRole::Inbox),
            unread_count: Some(3),
            total_count: None,
        }
    }

    fn message(subject: &str) -> Message {
        Message {
            id: MessageId(String::from("env-1")),
            mailbox_id: MailboxId(String::from("/root/maildir/INBOX")),
            headers: crate::domain::MessageHeaders {
                subject: String::from(subject),
                ..Default::default()
            },
            plain_body: Some(String::from("body\n")),
            html_body: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn mailboxes_round_trip() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        assert!(cache.load_mailboxes().is_none(), "empty cache");
        let listing = vec![mailbox("INBOX"), mailbox("Second")];
        cache.store_mailboxes(&listing);
        let loaded = cache.load_mailboxes().expect("cached listing");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "INBOX");
        assert_eq!(loaded[0].role, Some(MailboxRole::Inbox));
    }

    #[test]
    fn viewed_messages_round_trip_and_validate_identity() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        let inbox = MailboxId(String::from("/root/maildir/INBOX"));
        let archive = MailboxId(String::from("/root/maildir/Archive"));

        assert!(cache.load_message(&inbox, "env-1").is_none(), "empty cache");
        cache.store_message(&inbox, "env-1", &message("Hello again"));
        let loaded = cache.load_message(&inbox, "env-1").expect("cached message");
        assert_eq!(loaded.headers.subject, "Hello again");
        assert_eq!(loaded.plain_body.as_deref(), Some("body\n"));

        // A different mailbox or id is a miss even if a file existed.
        assert!(cache.load_message(&archive, "env-1").is_none());
        assert!(cache.load_message(&inbox, "env-2").is_none());
    }

    #[test]
    fn zero_message_limit_disables_message_caching() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(
            dir.path().to_path_buf(),
            CacheLimits {
                max_messages: 0,
                max_bytes: 1024,
            },
        );
        let inbox = MailboxId(String::from("INBOX"));
        cache.store_message(&inbox, "env-1", &message("X"));
        assert!(cache.load_message(&inbox, "env-1").is_none());
    }

    #[test]
    fn message_count_limit_evicts_oldest() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(
            dir.path().to_path_buf(),
            CacheLimits {
                max_messages: 2,
                max_bytes: u64::MAX,
            },
        );
        let inbox = MailboxId(String::from("INBOX"));
        for id in ["a", "b", "c"] {
            std::thread::sleep(std::time::Duration::from_millis(15));
            cache.store_message(&inbox, id, &message(id));
        }
        // "a" is the oldest write and must have been evicted.
        assert!(cache.load_message(&inbox, "a").is_none());
        assert!(cache.load_message(&inbox, "b").is_some());
        assert!(cache.load_message(&inbox, "c").is_some());
    }

    #[test]
    fn byte_budget_evicts_oldest_until_it_fits() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(
            dir.path().to_path_buf(),
            CacheLimits {
                max_messages: usize::MAX,
                max_bytes: 4096,
            },
        );
        let inbox = MailboxId(String::from("INBOX"));
        for id in ["a", "b", "c"] {
            std::thread::sleep(std::time::Duration::from_millis(15));
            cache.store_message(&inbox, id, &message(id));
        }
        // Each entry is small, all three fit — nothing evicted.
        assert!(cache.load_message(&inbox, "a").is_some());
    }

    #[test]
    fn oversized_messages_are_skipped_entirely() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(
            dir.path().to_path_buf(),
            CacheLimits {
                max_messages: 10,
                max_bytes: 16,
            },
        );
        let inbox = MailboxId(String::from("INBOX"));
        cache.store_message(&inbox, "big", &message("way beyond sixteen bytes"));
        assert!(cache.load_message(&inbox, "big").is_none());
    }
}
