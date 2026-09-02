//! File-based tracing setup that never writes into the TUI (plan §19
//! Phase 1). All output goes to a rotating file under the temp directory;
//! `RUST_LOG` overrides the level filter.

use std::fs;
use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Returned guard must live for the whole process: it flushes the
/// non-blocking writer on drop.
pub struct LoggingGuard {
    _worker: WorkerGuard,
}

pub fn log_dir() -> PathBuf {
    std::env::temp_dir().join("tmail").join("log")
}

pub fn init() -> LoggingGuard {
    let dir = log_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        // No stderr noise beyond a single line; this must never break startup.
        eprintln!("post: could not create log dir {}: {err}", dir.display());
    }
    let appender = tracing_appender::rolling::daily(&dir, "tmail.log");
    let (writer, worker) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    LoggingGuard { _worker: worker }
}
