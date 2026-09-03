//! Post-owned crash-safe draft journal (ADR 0002 §D.1).
//!
//! This is draft *state*, not a mail cache: every revision is recorded here
//! before any remote call, so a crash mid-remote-save can never lose text
//! the user typed. Writes are versioned, temp-file + atomic-rename
//! (`<local-id>.json`), one file per draft under the Post data directory.
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

    /// The default journal directory: `$POST_DATA_DIR/drafts` when set,
    /// else the platform user-data dir under `$HOME`
    /// (`~/Library/Application Support/post/drafts` on macOS,
    /// `~/.local/share/post/drafts` elsewhere). `None` when no home is
    /// known; saving then fails loudly instead of silently vanishing.
    pub fn open_default() -> Option<Self> {
        if let Some(dir) = std::env::var_os("POST_DATA_DIR") {
            return Some(Self::open(PathBuf::from(dir).join("drafts")));
        }
        let home = std::env::var_os("HOME")?;
        let mut dir = PathBuf::from(home);
        dir.push(if cfg!(target_os = "macos") {
            "Library/Application Support"
        } else {
            ".local/share"
        });
        dir.push("post");
        dir.push("drafts");
        Some(Self::open(dir))
    }

    /// Where the journal lives (for logs and tests).
    pub fn dir(&self) -> &Path {
        &self.dir
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
                Err(_) => continue,
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
    fn write_atomic(&self, path: &Path, entry: &JournalEntry) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
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
            message_id: Some(format!("<{local_id}@post.local>")),
            in_reply_to: None,
            references: None,
            remote_id: None,
            to: String::from("dest@example.com"),
            cc: String::new(),
            bcc: String::new(),
            subject: String::from("draft"),
            body: String::from(body),
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
