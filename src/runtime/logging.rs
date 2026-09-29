//! File-based tracing setup that never writes into the TUI (plan §19
//! Phase 1). All output goes to a rotating file under the temp directory;
//! `RUST_LOG` overrides the level filter.
//!
//! By default the verbosity depends on the build profile and the
//! `--debug` CLI flag ([`init`]): a `--debug` invocation or a debug
//! build traces at `debug`, while a plain release binary logs nothing.

use std::fs;
use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

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
    std::env::temp_dir().join("tmail").join("log")
}

/// Daily log files older than this many days are pruned at startup
/// (review pbcn: the temp-dir logs were never cleaned and grew without
/// bound across sessions).
const LOG_RETENTION_DAYS: i64 = 14;

/// Best-effort removal of daily log files past [`LOG_RETENTION_DAYS`].
/// Runs before the appender is created; a failure never blocks startup
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

pub fn init(debug: bool) -> LoggingGuard {
    let dir = log_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        // No stderr noise beyond a single line; this must never break startup.
        eprintln!("tmail: could not create log dir {}: {err}", dir.display());
    }
    prune_old_logs(&dir);
    let appender = tracing_appender::rolling::daily(&dir, "tmail.log");
    let (writer, worker) = tracing_appender::non_blocking(appender);
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
        std::fs::write(path(&format!("tmail.log.{ancient}")), b"old").expect("ancient log");
        let boundary = chrono::Utc::now().date_naive() - chrono::Duration::days(LOG_RETENTION_DAYS);
        std::fs::write(path(&format!("tmail.log.{boundary}")), b"boundary").expect("boundary log");
        std::fs::write(path("tmail.log.garbage"), b"unparsable").expect("unparsable name");
        std::fs::write(path("unrelated.txt"), b"unrelated").expect("unrelated file");

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
}
