//! Logging to stderr and a daily rolling file in the platform log directory.
//!
//! Never log tokens, credentials or Secret data.

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// `~/Library/Logs/Kubyl` on macOS, `<local data dir>/kubyl/logs` elsewhere.
pub fn log_dir() -> PathBuf {
    if cfg!(target_os = "macos")
        && let Some(home) = dirs::home_dir()
    {
        return home.join("Library/Logs/Kubyl");
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("kubyl")
        .join("logs")
}

/// Installs the global subscriber. Keep the guard alive until exit so file logs are flushed.
/// `RUST_LOG` overrides the default `info` level.
pub fn init() -> Option<WorkerGuard> {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter());

    let dir = log_dir();
    let file = std::fs::create_dir_all(&dir).ok().map(|_| {
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("kubyl")
            .filename_suffix("log")
            .max_log_files(7)
            .build(&dir);
        appender.map(tracing_appender::non_blocking)
    });
    match file {
        Some(Ok((writer, guard))) => {
            tracing_subscriber::registry()
                .with(stderr)
                .with(
                    fmt::layer()
                        .with_ansi(false)
                        .with_writer(writer)
                        .with_filter(filter()),
                )
                .init();
            Some(guard)
        }
        _ => {
            tracing_subscriber::registry().with(stderr).init();
            tracing::warn!("file logging disabled: cannot write to {}", dir.display());
            None
        }
    }
}
