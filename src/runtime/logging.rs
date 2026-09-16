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

pub fn init(debug: bool) -> LoggingGuard {
    let dir = log_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        // No stderr noise beyond a single line; this must never break startup.
        eprintln!("tmail: could not create log dir {}: {err}", dir.display());
    }
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
}
