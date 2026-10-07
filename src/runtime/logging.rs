//! File-based tracing setup that never writes into the TUI (plan §19
//! Phase 1). All output goes to a rotating file under the private Tmail
//! data container (`domain::paths::tmail_data_dir` + `log`); `RUST_LOG`
//! overrides the level filter.
//!
//! Three privacy rules (ticket 49gj): the location is outside the
//! world-readable shared temp dir, the directory chain is created and
//! repaired owner-only (`private_fs`), and every log file is opened
//! owner-only from creation — `tracing_appender`'s rolling appender is
//! replaced by a small private daily writer, so its umask-driven `0644`
//! and the pre-planted-symlink renames in a shared temp dir never turn
//! account names, paths, or error text into a local co-user's reading.
//! The tree from the old temp-dir location is removed best-effort at
//! startup.
//!
//! By default the verbosity depends on the build profile and the
//! `--debug` CLI flag ([`init`]): a `--debug` invocation or a debug
//! build traces at `debug`, while a plain release binary logs nothing.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::domain::private_fs;

/// Default tracing directives without `RUST_LOG`: explicit debug beats
/// the build profile, and a plain release binary stays silent.
pub fn default_filter(debug: bool) -> &'static str {
    if debug || cfg!(debug_assertions) {
        "debug"
    } else {
        "off"
    }
}

/// Returned guard must live for the whole process: it flushes the
/// non-blocking writer on drop.
pub struct LoggingGuard {
    _worker: WorkerGuard,
}

pub fn log_dir() -> PathBuf {
    match crate::domain::paths::tmail_data_dir() {
        Some(container) => container.join("log"),
        // No home to derive from: fall back to the temp dir and rely on
        // the owner-only directory mode instead (no readability by
        // co-users either way, and sticky-bit /tmp keeps other users out
        // of a directory they do not own).
        None => std::env::temp_dir().join("tmail").join("log"),
    }
}

/// Remove a directory tree, tolerating absence and logging failures
/// (never blocks startup; runs before any tracing subscriber exists, so
/// the one failure line is plain stderr).
fn remove_tree_best_effort(path: &Path) {
    if fs::metadata(path).is_err() {
        return;
    }
    if let Err(err) = fs::remove_dir_all(path) {
        eprintln!(
            "tmail: could not remove the old log location {}: {err}",
            path.display()
        );
    }
}

/// The tree the pre-49gj appender wrote to (the world-shared temp dir):
/// wiped when an upgraded version starts, so previously-accumulated
/// account names and paths stop lingering in a public location. The
/// current tree is never its own cleanup target.
fn cleanup_legacy_temp_logs(current: &Path) {
    cleanup_into(current, &std::env::temp_dir().join("tmail"));
}

/// The testable core: remove `legacy` unless it *is* the location the
/// writer now lives at.
fn cleanup_into(current: &Path, legacy: &Path) {
    if current == legacy {
        return;
    }
    remove_tree_best_effort(legacy);
}

/// Daily log files older than this many days are pruned at startup
/// (review pbcn: the temp-dir logs were never cleaned and grew without
/// bound across sessions).
const LOG_RETENTION_DAYS: i64 = 14;

/// Best-effort removal of daily log files past [`LOG_RETENTION_DAYS`].
/// Runs before the writer is created; a failure never blocks startup
/// (the same contract as the directory creation above).
fn prune_old_logs(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = chrono::Utc::now().date_naive() - chrono::Duration::days(LOG_RETENTION_DAYS);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(date) = name
            .to_str()
            .and_then(|name| name.strip_prefix("tmail.log."))
        else {
            continue;
        };
        let Ok(date) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
            continue;
        };
        if date < cutoff
            && let Err(err) = fs::remove_file(entry.path())
        {
            eprintln!(
                "tmail: could not prune old log {}: {err}",
                entry.path().display()
            );
        }
    }
}

/// A daily-rotating log file inside `dir`, where every file is opened
/// owner-only from creation (ticket 49gj: `tracing_appender`'s own
/// rolling appender follows the process umask, so a permissive umask
/// produces `0644` logs). Rotation compares the wall date on each write:
/// the first write of a new day closes the old file and opens
/// `tmail.log.<YYYY-MM-DD>` append+create. A failed open (unwritable
/// container) is retried on later writes and drops lines in between —
/// logging is best-effort by this module's contract and must never
/// break startup.
struct PrivateDailyWriter {
    dir: PathBuf,
    /// The date of the currently open file (`None` until, and after any
    /// failed reopen).
    open_date: Option<chrono::NaiveDate>,
    file: Option<fs::File>,
}

impl PrivateDailyWriter {
    /// Construction never fails: the file opens lazily on the first
    /// write, so a transiently unavailable container only costs lines.
    fn open(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            open_date: None,
            file: None,
        }
    }

    /// The open file for `date`, reopening whenever the date moved on
    /// (or nothing is open yet, or a previous reopen failed). Appends to
    /// an existing file — a same-day restart must never truncate the
    /// accumulated log — and repairs a stale loose mode (`0600` re-set
    /// explicitly, like [`private_fs::open`]).
    fn ensure_open_for(&mut self, date: chrono::NaiveDate) -> io::Result<()> {
        if self.open_date == Some(date) && self.file.is_some() {
            return Ok(());
        }
        let mut options = fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(private_fs::OWNER_FILE_MODE);
        }
        let path = self.dir.join(format!("tmail.log.{}", format_date(date)));
        let file = options.open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // `mode` applies only to a fresh file; an existing one keeps
            // its old permissions, so set them explicitly.
            file.set_permissions(fs::Permissions::from_mode(private_fs::OWNER_FILE_MODE))?;
        }
        self.file = Some(file);
        self.open_date = Some(date);
        Ok(())
    }

    /// The [`io::Write`] funnel through the injected date (tests pass a
    /// synthetic one; production passes today's local date from the
    /// trait impl). A failed ensure drops just this payload and resets
    /// the handle so the next write retries the open.
    fn write_for_date(&mut self, date: chrono::NaiveDate, buf: &[u8]) -> io::Result<usize> {
        match self.ensure_open_for(date) {
            Ok(()) => self
                .file
                .as_mut()
                .expect("ensure_open_for left a file open")
                .write(buf),
            Err(err) => {
                self.file = None;
                self.open_date = None;
                Err(err)
            }
        }
    }
}

/// The appender's file-name suffix (the pattern [`prune_old_logs`]
/// recognizes and the same day-file name the shared appender used).
fn format_date(date: chrono::NaiveDate) -> String {
    format!("{}-{:02}-{:02}", date.year(), date.month(), date.day())
}

impl io::Write for PrivateDailyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_for_date(Local::now().date_naive(), buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

pub fn init(debug: bool) -> LoggingGuard {
    let dir = log_dir();
    if let Err(err) = private_fs::create_dir_all(&dir) {
        // No stderr noise beyond a single line; this must never break
        // startup.
        eprintln!("tmail: could not create log dir {}: {err}", dir.display());
    }
    // The modes of an existing tree may be loose (a previous version's
    // umask): restrict from the log dir up to the container, exactly
    // like every other private storage does on its next access.
    if let Some(container) = crate::domain::paths::tmail_data_dir() {
        private_fs::restrict_dir_chain(&container, &dir);
    }
    cleanup_legacy_temp_logs(&dir);
    prune_old_logs(&dir);
    let (writer, worker) = tracing_appender::non_blocking(PrivateDailyWriter::open(&dir));
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter(debug)));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    LoggingGuard { _worker: worker }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_debug_flag_enables_debug_tracing() {
        assert_eq!(default_filter(true), "debug");
    }

    #[test]
    fn a_plain_run_matches_the_build_profile() {
        // In a debug build (test harness) tracing is on; a release
        // binary logs nothing by default.
        assert_eq!(
            default_filter(false),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "off"
            }
        );
    }

    #[test]
    fn logs_past_the_retention_window_are_pruned() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = |name: &str| dir.path().join(name);
        let ancient =
            chrono::Utc::now().date_naive() - chrono::Duration::days(LOG_RETENTION_DAYS + 1);
        fs::write(path(&format!("tmail.log.{ancient}")), b"old").expect("ancient log");
        let boundary = chrono::Utc::now().date_naive() - chrono::Duration::days(LOG_RETENTION_DAYS);
        fs::write(path(&format!("tmail.log.{boundary}")), b"boundary").expect("boundary log");
        fs::write(path("tmail.log.garbage"), b"unparsable").expect("unparsable name");
        fs::write(path("unrelated.txt"), b"unrelated").expect("unrelated file");

        prune_old_logs(dir.path());

        assert!(!path(&format!("tmail.log.{ancient}")).exists(), "pruned");
        assert!(
            path(&format!("tmail.log.{boundary}")).exists(),
            "the retention boundary itself is kept"
        );
        assert!(
            path("tmail.log.garbage").exists(),
            "unparsable names are left alone"
        );
        assert!(path("unrelated.txt").exists(), "other files are untouched");
    }

    /// The cleanup moves a stale public tree out of the way; the current
    /// location is never its own target.
    #[test]
    fn legacy_temp_logs_are_removed_and_the_current_tree_is_kept() {
        let legacy = std::env::temp_dir().join("tmail-legacy-test");
        fs::create_dir_all(legacy.join("log")).expect("seed");
        fs::write(
            legacy.join("log").join("tmail.log.2020-01-01"),
            b"ancient public log",
        )
        .expect("seed file");
        let current = tempfile::TempDir::new().expect("tempdir");

        cleanup_into(current.path(), &legacy);
        assert!(!legacy.exists(), "the public tree is wiped");
        assert!(current.path().exists(), "the current tree is untouched");

        // The current tree is never its own cleanup target (a same-path
        // run deletes nothing, even a successful one).
        cleanup_into(current.path(), current.path());
        assert!(
            current.path().exists(),
            "identifying the location as legacy deletes nothing"
        );

        // And a failure to remove is contained (nothing to remove, no
        // panic).
        remove_tree_best_effort(&legacy);
    }

    #[cfg(all(test, unix))]
    mod private_file_tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn mode(path: &Path) -> u32 {
            fs::metadata(path)
                .expect("path exists")
                .permissions()
                .mode()
                & 0o777
        }

        fn date(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
            chrono::NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
        }

        fn log_file(dir: &Path, day: chrono::NaiveDate) -> PathBuf {
            dir.join(format!("tmail.log.{}", format_date(day)))
        }

        /// Every log file is opened owner-only from creation, and a
        /// same-day restart repairs a file an older, looser version
        /// left behind (ticket 49gj: the umask-driven `0644` never
        /// recurs and existing loose files are tightened on reopen).
        #[test]
        fn log_files_are_owner_only_and_repaired_on_reopen() {
            let dir = tempfile::TempDir::new().expect("tempdir");
            let first = date(2026, 10, 6);
            let mut writer = PrivateDailyWriter::open(dir.path());
            writer.write_for_date(first, b"fresh\n").expect("append");

            let path = log_file(dir.path(), first);
            assert_eq!(mode(&path), private_fs::OWNER_FILE_MODE);
            assert_eq!(fs::read(&path).expect("read back"), b"fresh\n");

            // Same-day restart with a looser mode on disk: the writer
            // appends AND repairs.
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen");
            drop(writer);
            let mut writer = PrivateDailyWriter::open(dir.path());
            writer.write_for_date(first, b"more\n").expect("append");
            assert_eq!(mode(&path), private_fs::OWNER_FILE_MODE);
            assert_eq!(
                fs::read(&path).expect("read back"),
                b"fresh\nmore\n",
                "append, never truncate"
            );
        }

        /// Rotation opens a new owner-only file per day and keeps every
        /// earlier file intact.
        #[test]
        fn writes_rotate_at_the_date_boundary() {
            let dir = tempfile::TempDir::new().expect("tempdir");
            let mut writer = PrivateDailyWriter::open(dir.path());
            let first = date(2026, 10, 6);
            let second = date(2026, 10, 7);
            writer.write_for_date(first, b"day one\n").expect("one");
            writer.write_for_date(second, b"day two\n").expect("two");
            // The date moving back reopens (and appends to) the first
            // file too — the open-date is bookkeeping, not a one-way
            // ratchet.
            writer.write_for_date(first, b"back\n").expect("three");

            assert_eq!(
                fs::read(log_file(dir.path(), first)).expect("read back"),
                b"day one\nback\n"
            );
            assert_eq!(
                fs::read(log_file(dir.path(), second)).expect("read back"),
                b"day two\n"
            );
            assert_eq!(
                mode(&log_file(dir.path(), first)),
                private_fs::OWNER_FILE_MODE
            );
            assert_eq!(
                mode(&log_file(dir.path(), second)),
                private_fs::OWNER_FILE_MODE
            );
        }
    }
}
