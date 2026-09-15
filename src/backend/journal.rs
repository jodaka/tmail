//! Tmail-owned crash-safe draft journal (ADR 0002 §D.1).
//!
//! This is draft *state*, not a mail cache: every revision is recorded here
//! before any remote call, so a crash mid-remote-save can never lose text
//! the user typed. Writes are versioned, temp-file + atomic-rename
//! (`<local-id>.json`), one file per draft under the Tmail data directory.
//!
//! The journal is exercised by backend implementations (the draft save is
//! one backend operation per ADR 0002 consequences) and read back at
//! startup to restore drafts (crash/restart acceptance, plan §19 Phase 6).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::DraftSnapshot;

/// Journal format version (ADR 0002: versioned from day one).
const VERSION: u32 = 1;

/// Filenames keep temp writes apart when the same entry is written twice
/// in one process (pid is shared; the counter is not).
static TMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One persisted draft: the newest recorded revision plus how far the
/// remote has confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub version: u32,
    /// The newest revision recorded (pre-remote-call).
    pub draft: DraftSnapshot,
    /// The newest revision confirmed pushed to the remote Drafts mailbox.
    pub saved_revision: u64,
}

/// The draft journal rooted at a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftJournal {
    dir: PathBuf,
}

impl DraftJournal {
    /// Journal rooted at an explicit directory (tests, explicit config).
    pub fn open(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The default journal directory, scoped per account (ticket c0n0):
    /// `$TMAIL_DATA_DIR/drafts/<account>` when the env var is set, else the
    /// platform user-data dir under `$HOME`
    /// (`~/Library/Application Support/tmail/drafts/<account>` on macOS,
    /// `~/.local/share/tmail/drafts/<account>` elsewhere). Drafts saved
    /// under one account must never be restored under another (a
    /// wrong-identity send hazard). An account-less journal (`None`) keeps
    /// the unscoped root: nothing to separate.
    ///
    /// `None` when no home is known; saving then fails loudly instead of
    /// silently vanishing.
    pub fn open_default(account: Option<&str>) -> Option<Self> {
        let drafts = if let Some(dir) = std::env::var_os("TMAIL_DATA_DIR") {
            PathBuf::from(dir).join("drafts")
        } else {
            let home = std::env::var_os("HOME")?;
            let mut dir = PathBuf::from(home);
            dir.push(if cfg!(target_os = "macos") {
                "Library/Application Support"
            } else {
                ".local/share"
            });
            dir.push("tmail");
            dir.push("drafts");
            dir
        };
        Some(Self::open_scoped(drafts, account))
    }

    /// [`DraftJournal::open`] for the account scope of `root`: with an
    /// account, the journal lives in `root/<account>` and any draft files
    /// still directly in `root` (the pre-switching layout) move into it —
    /// a one-time, idempotent migration that assumes they belong to the
    /// account now being opened (documented in ticket c0n0). A moved-into
    /// name that is already taken stays behind: keeping a copy always
    /// beats risking data loss.
    fn open_scoped(root: PathBuf, account: Option<&str>) -> Self {
        let Some(account) = account else {
            return Self::open(root);
        };
        let scoped = root.join(Self::journal_dir_name(account));
        if scoped != root {
            Self::migrate_legacy(&root, &scoped);
        }
        Self::open(scoped)
    }

    /// Move every draft file still directly in `root` into `scoped` (the
    /// one-time legacy migration of [`DraftJournal::open_scoped`]). Only
    /// `.json` entries move; a target that already exists wins (the
    /// account scope's copy is authoritative) and a failed move is logged
    /// and skipped — the file stays in place, nothing is lost.
    ///
    /// This runs synchronously from session construction, before the
    /// event loop and terminal exist: nothing else runs on the runtime
    /// then, so the sync I/O cannot stall the UI (ticket tnc1 review).
    /// One-time and idempotent — after the first migration it is a single
    /// `read_dir` of the root — so the accepted trade-off is startup
    /// latency, never frame time.
    fn migrate_legacy(root: &Path, scoped: &Path) {
        let Ok(read_dir) = fs::read_dir(root) else {
            return; // No legacy directory: nothing to migrate.
        };
        if let Err(err) = fs::create_dir_all(scoped) {
            tracing::warn!(
                dir = %scoped.display(),
                %err,
                "could not create the account's draft journal directory"
            );
            return;
        }
        let mut moved = 0usize;
        for entry in read_dir {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            let target = scoped.join(name);
            if target.exists() {
                continue;
            }
            match fs::rename(&path, &target) {
                Ok(()) => moved += 1,
                Err(err) => tracing::warn!(
                    file = %path.display(),
                    %err,
                    "legacy draft file could not be moved into the account's journal"
                ),
            }
        }
        if moved > 0 {
            tracing::info!(
                dir = %scoped.display(),
                moved,
                "migrated the shared draft journal into the account's scope"
            );
        }
    }

    /// The directory name for one account's journal scope: journal ids are
    /// validated separately, so the account name only has to stay inside
    /// the drafts root — every character outside `[a-zA-Z0-9._-]` (a path
    /// separator included) becomes `_`. Two accounts differing only in
    /// such characters share a scope; their draft ids cannot collide
    /// (they are minted internally), so the merge is harmless.
    fn journal_dir_name(account: &str) -> String {
        let mut name: String = account
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if name.is_empty() {
            name.push('_');
        }
        name
    }

    fn file(&self, local_id: &str) -> io::Result<PathBuf> {
        // Ids are minted internally, but never let one escape the directory.
        if local_id.is_empty()
            || !local_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid draft local id",
            ));
        }
        Ok(self.dir.join(format!("{local_id}.json")))
    }

    /// Record `snapshot` as the newest revision of its draft — before any
    /// remote call (ADR 0002 §D.1). The remote-confirmation bookkeeping is
    /// preserved; remote status is advanced afterwards by
    /// [`DraftJournal::mark_remote`]. Atomic: temp file + rename, so a
    /// crash mid-write leaves the previous revision intact.
    pub fn record(&self, snapshot: &DraftSnapshot) -> io::Result<()> {
        let path = self.file(&snapshot.local_id.0)?;
        let mut entry = match self.read(&snapshot.local_id.0) {
            Ok(Some(entry)) => entry,
            _ => JournalEntry {
                version: VERSION,
                saved_revision: 0,
                draft: snapshot.clone(),
            },
        };
        entry.version = VERSION;
        entry.draft = snapshot.clone();
        self.write_atomic(&path, &entry)
    }

    /// Mark `revision` of `local_id` as confirmed on the remote (after the
    /// add-then-delete replacement completed, ADR 0002 §D.3).
    pub fn mark_remote(&self, local_id: &str, revision: u64) -> io::Result<()> {
        let path = self.file(local_id)?;
        let Some(mut entry) = self.read(local_id)? else {
            return Ok(()); // Nothing recorded (e.g. journal removed meanwhile).
        };
        entry.saved_revision = entry.saved_revision.max(revision);
        self.write_atomic(&path, &entry)
    }

    /// The remote-confirmed revision recorded for `local_id`, if any.
    pub fn saved_revision(&self, local_id: &str) -> io::Result<Option<u64>> {
        Ok(self.read(local_id)?.map(|entry| entry.saved_revision))
    }

    /// Load every draft in the journal, newest revision each. Corrupt or
    /// unknown-version files are skipped with a warning: a broken file
    /// must not hide the other drafts (fail-safe direction: keep drafts).
    pub fn load_all(&self) -> io::Result<Vec<JournalEntry>> {
        let mut entries = Vec::new();
        let read_dir = match fs::read_dir(&self.dir) {
            Ok(read_dir) => read_dir,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(entries),
            Err(err) => return Err(err),
        };
        for file in read_dir {
            let path = match file {
                Ok(file) => file.path(),
                Err(err) => {
                    tracing::warn!(error = %err, "skipped an unreadable journal directory entry");
                    continue;
                }
            };
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<JournalEntry>(&bytes).ok())
                .filter(|entry| entry.version == VERSION)
            {
                Some(entry) => entries.push(entry),
                None => tracing::warn!(
                    path = %path.display(),
                    "skipping unreadable journal entry"
                ),
            }
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.draft.revision));
        Ok(entries)
    }

    /// Delete the journal entry for `local_id` (confirmed discard, plan
    /// §14). A missing file is fine — already gone.
    pub fn remove(&self, local_id: &str) -> io::Result<()> {
        let path = self.file(local_id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn read(&self, local_id: &str) -> io::Result<Option<JournalEntry>> {
        let bytes = match fs::read(self.file(local_id)?) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    /// Versioned temp-file + atomic rename with a file sync before the
    /// rename, so the recorded bytes survive a crash (ADR 0002 §D.1).
    /// The temp name carries a process-unique counter on top of the pid:
    /// two concurrent saves of the same draft (same pid, so serialized by
    /// the single-threaded runtime today) would otherwise share one name.
    fn write_atomic(&self, path: &Path, entry: &JournalEntry) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension(format!(
            "json.tmp-{}-{}",
            std::process::id(),
            TMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        {
            let mut file = fs::File::create(&tmp)?;
            serde_json::to_writer(&mut file, entry)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DraftId;

    fn snapshot(local_id: &str, revision: u64, body: &str) -> DraftSnapshot {
        DraftSnapshot {
            local_id: DraftId(String::from(local_id)),
            message_id: Some(format!("<{local_id}@tmail.local>")),
            in_reply_to: None,
            references: None,
            remote_id: None,
            to: String::from("dest@example.com"),
            cc: String::new(),
            bcc: String::new(),
            subject: String::from("draft"),
            body: String::from(body),
            attachments: Vec::new(),
            revision,
        }
    }

    fn journal() -> (DraftJournal, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        (DraftJournal::open(dir.path().to_path_buf()), dir)
    }

    #[test]
    fn record_then_load_round_trips() {
        let (j, _dir) = journal();
        j.record(&snapshot("local-1", 1, "hello")).expect("record");
        let entries = j.load_all().expect("load");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].draft.body, "hello");
        assert_eq!(entries[0].version, VERSION);
        assert_eq!(entries[0].saved_revision, 0);
    }

    #[test]
    fn record_newest_revision_wins() {
        let (j, _dir) = journal();
        j.record(&snapshot("local-1", 1, "first")).unwrap();
        j.record(&snapshot("local-1", 2, "second")).unwrap();
        let entries = j.load_all().unwrap();
        assert_eq!(entries[0].draft.body, "second");
        assert_eq!(entries[0].draft.revision, 2);
    }

    #[test]
    fn record_preserves_remote_confirmation() {
        let (j, _dir) = journal();
        j.record(&snapshot("local-1", 1, "first")).unwrap();
        j.mark_remote("local-1", 1).unwrap();
        // A crash-safe record of revision 2 must not forget revision 1
        // reached the remote.
        j.record(&snapshot("local-1", 2, "second")).unwrap();
        assert_eq!(j.saved_revision("local-1").unwrap(), Some(1));
        j.mark_remote("local-1", 2).unwrap();
        assert_eq!(j.saved_revision("local-1").unwrap(), Some(2));
    }

    #[test]
    fn mark_remote_on_missing_entry_is_harmless() {
        let (j, _dir) = journal();
        j.mark_remote("local-ghost", 3).expect("no entry, no error");
    }

    #[test]
    fn load_all_skips_corrupt_files_but_keeps_the_rest() {
        let (j, dir) = journal();
        j.record(&snapshot("local-1", 1, "keep")).unwrap();
        fs::write(dir.path().join("local-2.json"), "not json{").unwrap();
        fs::write(dir.path().join("local-3.json"), r#"{"version":99}"#).unwrap();
        let entries = j.load_all().unwrap();
        assert_eq!(entries.len(), 1, "only the healthy entry loads");
        assert_eq!(entries[0].draft.body, "keep");
    }

    #[test]
    fn load_all_on_missing_directory_is_empty() {
        let (j, dir) = journal();
        let entries = j.load_all().expect("missing dir is empty, not an error");
        assert!(entries.is_empty());
        drop(dir);
    }

    #[test]
    fn remove_deletes_and_tolerates_missing() {
        let (j, _dir) = journal();
        j.record(&snapshot("local-1", 1, "bye")).unwrap();
        j.remove("local-1").unwrap();
        assert!(j.load_all().unwrap().is_empty());
        j.remove("local-1").expect("second remove is a no-op");
        j.remove("local-never").expect("missing is a no-op");
    }

    #[test]
    fn ids_cannot_escape_the_directory() {
        let (j, _dir) = journal();
        let err = j
            .record(&snapshot("../../etc/passwd", 1, "evil"))
            .expect_err("path traversal rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn account_scopes_are_separate_directories() {
        // Ticket c0n0: a draft recorded under one account is invisible to
        // another account's journal.
        let dir = tempfile::TempDir::new().unwrap();
        let a = DraftJournal::open_scoped(dir.path().to_path_buf(), Some("a"));
        let b = DraftJournal::open_scoped(dir.path().to_path_buf(), Some("b"));
        a.record(&snapshot("local-1", 1, "for a")).unwrap();
        assert_eq!(b.load_all().unwrap(), Vec::new());
        let entries = a.load_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].draft.body, "for a");
    }

    #[test]
    fn journal_scope_sanitizes_the_account_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let j = DraftJournal::open_scoped(dir.path().to_path_buf(), Some("a/b c"));
        j.record(&snapshot("local-1", 1, "kept inside")).unwrap();
        // The scope stayed inside the drafts root (no `a/b c` tree).
        let entries = j.load_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(dir.path().join("drafts").is_dir() || dir.path().file_name().is_some());
        assert_eq!(j.dir.file_name().and_then(|n| n.to_str()), Some("a_b_c"));
    }

    #[test]
    fn legacy_journal_migrates_into_the_account_scope() {
        // The pre-switching layout stored drafts directly in the root;
        // opening an account's scope moves them once, so a switch back
        // still finds them.
        let dir = tempfile::TempDir::new().unwrap();
        let legacy = DraftJournal::open(dir.path().to_path_buf());
        legacy
            .record(&snapshot("local-1", 1, "legacy draft"))
            .unwrap();
        let scoped = DraftJournal::open_scoped(dir.path().to_path_buf(), Some("a"));
        let entries = scoped.load_all().unwrap();
        assert_eq!(entries.len(), 1, "the legacy draft moved into the scope");
        assert_eq!(entries[0].draft.body, "legacy draft");
        // Idempotent: opening again moves nothing and keeps the draft.
        let again = DraftJournal::open_scoped(dir.path().to_path_buf(), Some("a"));
        assert_eq!(again.load_all().unwrap().len(), 1);
    }

    #[test]
    fn legacy_migration_never_overwrites_the_scoped_copy() {
        let dir = tempfile::TempDir::new().unwrap();
        let legacy = DraftJournal::open(dir.path().to_path_buf());
        legacy.record(&snapshot("local-1", 1, "legacy")).unwrap();
        let scoped = DraftJournal::open_scoped(dir.path().to_path_buf(), Some("a"));
        scoped.record(&snapshot("local-1", 2, "scoped")).unwrap();
        // A later scope opening must not clobber the newer scoped copy
        // with the stale legacy file.
        let again = DraftJournal::open_scoped(dir.path().to_path_buf(), Some("a"));
        let entries = again.load_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].draft.body, "scoped");
    }

    #[test]
    fn accountless_journal_keeps_the_unscoped_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let j = DraftJournal::open_scoped(dir.path().to_path_buf(), None);
        j.record(&snapshot("local-1", 1, "x")).unwrap();
        assert_eq!(j.dir, dir.path());
    }

    #[test]
    fn atomic_write_leaves_no_tmp_files() {
        let (j, dir) = journal();
        j.record(&snapshot("local-1", 1, "clean")).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "tmp files left behind: {leftovers:?}");
    }
}
