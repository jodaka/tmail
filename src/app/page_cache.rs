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
//! Confirmed mutations keep it truthful (ticket kkaq): flag changes
//! re-store the corrected page, and moves evict it so a warm start
//! re-fetches instead of resurrecting a moved row.
//! Summaries and rendered messages contain no credentials (plan §21:
//! nothing secret is stored or logged).
//!
//! Storage is bounded: per-mailbox pages are capped at
//! [`MAX_FILES_PER_MAILBOX`] files; the viewed-message cache is capped by
//! [`CacheLimits`] (entry count and total bytes, from `[tmail.cache]`).
//! The oldest modifications are evicted first.
//!
//! The cache is private by construction (ticket ty57): directories are
//! created owner-only (`0700`) and files `0600`, and entries written by
//! an older version under a looser umask are repaired on the next read or
//! write — no other local user can list or read what was fetched.

use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::{Mailbox, MailboxId, Message, MessageSummary, Page};

/// Version of the on-disk format; bumping it invalidates old caches.
/// 3: LRU stamps moved to file modification times — pruning no longer
/// parses file contents, and a cache hit bumps the stamp with a metadata
/// write instead of a full rewrite. v2 entries (stamp inside the file)
/// fail the version check and are re-fetched.
const CACHE_VERSION: u32 = 3;

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

/// The on-disk viewed message, keyed by its locator. Recency is tracked
/// by the file's modification time (bumped on every hit), so eviction
/// never has to parse file contents.
#[derive(Serialize, Deserialize)]
struct CachedMessage {
    version: u32,
    mailbox: String,
    id: String,
    message: Message,
}

/// Bump a cache entry's recency stamp: the file's modification time is
/// the LRU key, so a hit re-stamps it with a metadata write instead of a
/// full rewrite. Best-effort — a failed touch only makes the entry look
/// older than it is.
fn touch(path: &Path) {
    let Ok(file) = fs::OpenOptions::new().append(true).open(path) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let times = fs::FileTimes::new().set_accessed(now).set_modified(now);
    let _ = file.set_times(times);
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
    /// The highest directory the cache owns: permission repair walks from
    /// a file's parent up to (and including) this, never above it. With
    /// [`PageCache::open`] the caller-provided root is the boundary;
    /// [`PageCache::open_default`] owns the `cache` container above the
    /// per-account scopes as well.
    repair_root: PathBuf,
    limits: CacheLimits,
}

impl PageCache {
    /// Cache rooted at an explicit directory (tests, explicit wiring).
    /// The root is also the upper bound for permission repair: nothing
    /// above it is ever touched.
    pub fn open(root: PathBuf, limits: CacheLimits) -> Self {
        Self {
            repair_root: root.clone(),
            root,
            limits,
        }
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

    /// Root plus the account scope segment. `container` (the shared
    /// `cache` directory) is Tmail-owned too, so repair may restrict it.
    fn scoped(container: PathBuf, account: Option<&str>, limits: CacheLimits) -> Self {
        Self {
            root: container.join(key_part(account.unwrap_or("default"))),
            repair_root: container,
            limits,
        }
    }

    /// Where a page lives on disk. The `limit` is deliberately *not*
    /// part of the file name (ticket kkaq): `page_size_auto` resizes
    /// change the limit, and keying the name by it would linger one file
    /// per historical limit per mailbox instead of overwriting in place.
    /// The identity stored inside the file still validates the limit, so
    /// a mismatched limit is a miss — re-fetched, never wrong mail.
    fn path(&self, mailbox: &MailboxId, query: Option<&str>, offset: usize) -> PathBuf {
        let query_key = query.map_or_else(String::new, key_part);
        self.root
            .join(key_part(&mailbox.0))
            .join(format!("{query_key}-{offset:06}.json"))
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
        let bytes = self.read_cached(&self.path(mailbox, query, offset))?;
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
        let path = self.path(mailbox, query, page.offset);
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

    /// Drop one cached page (ticket kkaq): a confirmed move makes the
    /// stored copy list a message that left the mailbox, and the local
    /// post-move page cannot be re-stored truthfully (backend ids shift),
    /// so the file is removed — a warm start then re-fetches instead of
    /// resurrecting the moved row. The name carries no limit, so every
    /// limit variant for the identity goes with it. A missing file is
    /// already "evicted"; like every cache write, failure is logged and
    /// swallowed.
    pub fn evict(&self, mailbox: &MailboxId, query: Option<&str>, offset: usize) {
        let path = self.path(mailbox, query, offset);
        if let Err(err) = fs::remove_file(&path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(path = %path.display(), %err, "cache: evict failed");
        }
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
        let bytes = self.read_cached(&self.root.join("mailboxes.json"))?;
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
    /// for a different identity. A hit bumps the entry's recency stamp
    /// (the file's modification time) with a metadata write — no
    /// re-serialization — so recently viewed messages outlive older ones
    /// under the size caps.
    pub fn load_message(&self, mailbox: &MailboxId, id: &str) -> Option<Message> {
        let path = self.message_path(mailbox, id);
        let bytes = self.read_cached(&path)?;
        let cached: CachedMessage = serde_json::from_slice(&bytes).ok()?;
        if cached.version != CACHE_VERSION || cached.mailbox != mailbox.0 || cached.id != id {
            return None;
        }
        touch(&path);
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
    /// size fit. Recency is the file's modification time (bumped on every
    /// hit), so pruning reads only directory listings and metadata —
    /// never file contents. Unparsable stamps (a legacy or foreign file)
    /// sort as oldest and go first.
    fn prune_messages(&self) {
        let dir = self.root.join("messages");
        let Ok(mailbox_dirs) = fs::read_dir(&dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf, u64)> = mailbox_dirs
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|entry| fs::read_dir(entry.path()).ok())
            .flatten()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| {
                let meta = fs::metadata(&path).ok()?;
                Some((meta.modified().ok()?, path, meta.len()))
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
        // Crash leftovers from a torn write: the temp files never carry
        // data the cache would serve, so they are only litter.
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let Ok(files) = fs::read_dir(entry.path()) else {
                    continue;
                };
                for file in files.flatten() {
                    if file.path().extension().is_some_and(|ext| ext == "tmp") {
                        let _ = fs::remove_file(file.path());
                    }
                }
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
        if let Some(parent) = path.parent() {
            if let Err(err) = create_private_dir_all(parent) {
                tracing::debug!(%err, "cache: mkdir failed");
                return;
            }
            // Directories an older version left behind may be looser than
            // what this process creates; fix them on the way past.
            restrict_dir_chain(&self.repair_root, parent);
        }
        let tmp = path.with_extension("json.tmp");
        if write_private(&tmp, payload)
            .and_then(|()| fs::rename(&tmp, path))
            .is_err()
        {
            tracing::debug!(path = %path.display(), "cache: write failed");
        }
    }

    /// Read a cache file, repairing the file and its parent directories to
    /// owner-only on the way: this is where an entry written by an older
    /// version under a looser umask is fixed, so privacy never depends on
    /// a full tree sweep at startup. A read never fails the caller, and a
    /// failed repair never fails the read.
    fn read_cached(&self, path: &Path) -> Option<Vec<u8>> {
        let bytes = fs::read(path).ok()?;
        restrict_file(path);
        if let Some(parent) = path.parent() {
            restrict_dir_chain(&self.repair_root, parent);
        }
        Some(bytes)
    }
}

/// Create `dir` and any missing ancestor, owner-only on Unix. The mode
/// applies to the directories this call creates; existing ancestors keep
/// theirs and are repaired by [`restrict_dir_chain`] instead.
fn create_private_dir_all(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Best-effort repair of a directory chain: `dir`, then each parent up to
/// and including `repair_root`, is restricted to the owner. Nothing above
/// `repair_root` is touched, and a path outside it stops the walk.
fn restrict_dir_chain(repair_root: &Path, dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut current = Some(dir);
        while let Some(path) = current {
            if !path.starts_with(repair_root) {
                break;
            }
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
            if path == repair_root {
                break;
            }
            current = path.parent();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (repair_root, dir);
    }
}

/// Best-effort chmod of one cache file to owner-only.
fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Write `payload` to `path`, created owner-only from the first byte and
/// repaired when a crash left a looser temp file behind. The caller
/// renames the result into place, so a failed write never becomes an
/// entry the cache would serve.
fn write_private(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `mode` applies only to a fresh file; a crash leftover keeps its
        // old permissions, so set them explicitly.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(payload)
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

    pub(super) fn page(offset: usize) -> Page<MessageSummary> {
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
    fn an_evicted_page_is_no_longer_served() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        let mailbox = MailboxId(String::from("inbox"));

        cache.store(&mailbox, None, &page(0));
        assert!(cache.load(&mailbox, None, 0, 20).is_some());
        cache.evict(&mailbox, None, 0);
        assert!(cache.load(&mailbox, None, 0, 20).is_none(), "evicted");
        // Evicting an absent entry is a no-op.
        cache.evict(&mailbox, None, 0);
        // A different identity is untouched.
        cache.store(&mailbox, None, &page(0));
        cache.evict(&mailbox, Some("query"), 0);
        assert!(
            cache.load(&mailbox, None, 0, 20).is_some(),
            "unrelated identity kept"
        );
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
        let path = cache.path(&mailbox, None, 0);
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

    pub(super) fn mailbox(name: &str) -> Mailbox {
        Mailbox {
            id: MailboxId(String::from("/root/maildir/INBOX")),
            name: String::from(name),
            role: Some(MailboxRole::Inbox),
            unread_count: Some(3),
            total_count: None,
        }
    }

    pub(super) fn message(subject: &str) -> Message {
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
    fn a_hit_refreshes_recency_without_a_rewrite() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(
            dir.path().to_path_buf(),
            CacheLimits {
                max_messages: 2,
                max_bytes: u64::MAX,
            },
        );
        let inbox = MailboxId(String::from("INBOX"));
        for id in ["a", "b"] {
            std::thread::sleep(std::time::Duration::from_millis(15));
            cache.store_message(&inbox, id, &message(id));
        }
        // A hit re-stamps "a" (the file's modification time), so the next
        // store evicts "b" — the least recently *used*, not written.
        std::thread::sleep(std::time::Duration::from_millis(15));
        assert!(cache.load_message(&inbox, "a").is_some());
        std::thread::sleep(std::time::Duration::from_millis(15));
        cache.store_message(&inbox, "c", &message("c"));
        assert!(cache.load_message(&inbox, "a").is_some(), "hit survives");
        assert!(cache.load_message(&inbox, "b").is_none(), "b evicted");
        assert!(cache.load_message(&inbox, "c").is_some());
    }

    #[test]
    fn torn_write_tempfiles_are_swept() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        let inbox = MailboxId(String::from("INBOX"));
        cache.store_message(&inbox, "a", &message("a"));
        // A crash mid-write leaves the sibling temp file behind.
        let tmp = cache.message_path(&inbox, "a").with_extension("json.tmp");
        std::fs::write(&tmp, b"torn write").expect("litter the cache");
        cache.store_message(&inbox, "b", &message("b"));
        assert!(!tmp.exists(), "the temp file is swept on the next prune");
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

/// Permissions of the on-disk cache (ticket ty57): the cache holds message
/// summaries and full bodies, so only its owner may list or read it.
#[cfg(all(test, unix))]
mod permission_tests {
    use super::mailbox_message_tests::{mailbox as sample_mailbox, message};
    use super::tests::page;
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("cache path exists")
            .permissions()
            .mode()
            & 0o777
    }

    fn assert_private_file(path: &Path) {
        assert_eq!(mode(path), 0o600, "{} must be 0600", path.display());
    }

    /// The directory and every parent up to the cache root must be 0700.
    fn assert_private_dirs(root: &Path, dir: &Path) {
        let mut current = Some(dir);
        while let Some(path) = current {
            assert_eq!(mode(path), 0o700, "{} must be 0700", path.display());
            if path == root {
                break;
            }
            current = path.parent();
        }
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    }

    fn inbox_id() -> MailboxId {
        MailboxId(String::from("/root/maildir/INBOX"))
    }

    fn mailbox_dir(cache: &PageCache, inbox: &MailboxId) -> PathBuf {
        cache.root.join(key_part(&inbox.0))
    }

    fn message_dir(cache: &PageCache, inbox: &MailboxId) -> PathBuf {
        cache.root.join("messages").join(key_part(&inbox.0))
    }

    #[test]
    fn stored_entries_and_their_directories_are_owner_only() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        let inbox = inbox_id();

        cache.store(&inbox, None, &page(0));
        cache.store_mailboxes(&[sample_mailbox("INBOX")]);
        cache.store_message(&inbox, "env-1", &message("Hello"));

        assert_private_file(&cache.path(&inbox, None, 0));
        assert_private_file(&cache.root.join("mailboxes.json"));
        assert_private_file(&cache.message_path(&inbox, "env-1"));
        assert_private_dirs(dir.path(), &cache.root);
        assert_private_dirs(dir.path(), &mailbox_dir(&cache, &inbox));
        assert_private_dirs(dir.path(), &message_dir(&cache, &inbox));
    }

    #[test]
    fn the_shared_cache_container_is_repaired_too() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let container = dir.path().join("cache");
        fs::create_dir_all(&container).expect("container");
        chmod(&container, 0o755);
        let cache = PageCache::scoped(container.clone(), Some("account"), CacheLimits::default());

        cache.store(&inbox_id(), None, &page(0));

        assert_eq!(mode(&container), 0o700, "cache container must be 0700");
        assert_private_dirs(&container, &cache.root);
    }

    #[test]
    fn writes_repair_loose_modes_from_an_older_version() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        let inbox = inbox_id();
        cache.store(&inbox, None, &page(0));

        // Simulate a cache written by a version that respected only the
        // umask: 0755 directories, 0644 files.
        chmod(&cache.root, 0o755);
        chmod(&mailbox_dir(&cache, &inbox), 0o755);
        let path = cache.path(&inbox, None, 0);
        chmod(&path, 0o644);

        cache.store(&inbox, None, &page(0));

        assert_private_file(&path);
        assert_private_dirs(dir.path(), &cache.root);
        assert_private_dirs(dir.path(), &mailbox_dir(&cache, &inbox));
    }

    #[test]
    fn reads_repair_loose_modes_from_an_older_version() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        let inbox = inbox_id();
        cache.store(&inbox, None, &page(0));
        cache.store_message(&inbox, "env-1", &message("Hello"));

        let page_path = cache.path(&inbox, None, 0);
        let message_path = cache.message_path(&inbox, "env-1");
        for path in [&page_path, &message_path] {
            chmod(path, 0o644);
        }
        let messages_root = cache.root.join("messages");
        for dir in [
            &cache.root,
            &mailbox_dir(&cache, &inbox),
            &messages_root,
            &message_dir(&cache, &inbox),
        ] {
            chmod(dir, 0o755);
        }

        assert!(cache.load(&inbox, None, 0, 20).is_some(), "page hit");
        assert!(cache.load_message(&inbox, "env-1").is_some(), "message hit");

        assert_private_file(&page_path);
        assert_private_file(&message_path);
        assert_private_dirs(dir.path(), &mailbox_dir(&cache, &inbox));
        assert_private_dirs(dir.path(), &message_dir(&cache, &inbox));
    }
}
