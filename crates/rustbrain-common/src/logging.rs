//! Shared logging initialisation for all rust-brain services.
//!
//! # Environment variables
//!
//! | Variable    | Default  | Description                             |
//! |-------------|----------|-----------------------------------------|
//! | `RUST_LOG`  | (level)  | Standard tracing filter string          |
//! | `LOG_FORMAT`| `pretty` | `pretty` \| `compact` \| `json`         |
//! | `LOG_FILE`  | (none)   | Absolute or relative path for log file  |
//!
//! # Example
//!
//! ```rust,no_run
//! use rustbrain_common::logging::init_logging;
//! use tracing::Level;
//!
//! fn main() {
//!     let _guard = init_logging(Level::INFO);
//!     tracing::info!("service started");
//! }
//! ```

use std::path::Path;
use tracing::{debug, trace, Level};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// Keeps background log-writer threads alive.  Drop at program exit (or store
/// in `main` for the duration of the process).
pub struct LoggingGuard {
    _file_guard: Option<WorkerGuard>,
}

/// Initialise stdout logging.
///
/// Log level is taken from `RUST_LOG`; `default_level` is used when `RUST_LOG`
/// is absent or invalid.  Set `LOG_FORMAT=json` for structured JSON output
/// (default: pretty).  Set `LOG_FILE=/path/to/file.log` to additionally write
/// to a file (compact, no ANSI).
pub fn init_logging(default_level: Level) -> LoggingGuard {
    trace!(default_level = %default_level, "init_logging entry");
    let guard = init_logging_with_directives(default_level, &[]);
    debug!(default_level = %default_level, "init_logging complete");
    guard
}

/// Like [`init_logging`] but merges `extra_directives` into the filter on top
/// of `RUST_LOG`.  Useful for pinning a specific crate to `debug` regardless
/// of the environment.
///
/// ```rust,no_run
/// use rustbrain_common::logging::init_logging_with_directives;
/// use tracing::Level;
///
/// let _guard = init_logging_with_directives(Level::INFO, &["rustbrain_api=debug"]);
/// ```
pub fn init_logging_with_directives(default_level: Level, extra_directives: &[&str]) -> LoggingGuard {
    trace!(
        default_level = %default_level,
        num_extra_directives = extra_directives.len(),
        "init_logging_with_directives entry"
    );
    let make_filter = || {
        let mut f = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(default_level.to_string()));
        for directive in extra_directives {
            if let Ok(d) = directive.parse() {
                f = f.add_directive(d);
            }
        }
        f
    };

    let log_format = std::env::var("LOG_FORMAT")
        .map(|v| v.to_lowercase())
        .unwrap_or_else(|_| "pretty".to_string());

    // Helper macro to avoid repeating the file-layer + init logic for each format branch.
    // Each stdout format produces a different concrete type, so we must call .init() per branch.
    macro_rules! init_subscriber {
        ($stdout_layer:expr) => {
            match std::env::var("LOG_FILE") {
                Ok(ref path) => {
                    let (writer, guard) = make_file_appender(path);
                    let file_layer = fmt::layer()
                        .with_ansi(false)
                        .compact()
                        .with_writer(writer)
                        .with_filter(make_filter());
                    tracing_subscriber::registry()
                        .with($stdout_layer)
                        .with(file_layer)
                        .init();
                    Some(guard)
                }
                Err(_) => {
                    tracing_subscriber::registry()
                        .with($stdout_layer)
                        .init();
                    None
                }
            }
        };
    }

    let file_guard = match log_format.as_str() {
        "json" => init_subscriber!(fmt::layer().json().with_filter(make_filter())),
        "compact" => init_subscriber!(fmt::layer().compact().with_filter(make_filter())),
        _ => init_subscriber!(fmt::layer().pretty().with_filter(make_filter())),
    };

    let guard = LoggingGuard {
        _file_guard: file_guard,
    };
    debug!(
        default_level = %default_level,
        log_format = %log_format,
        has_file_output = guard._file_guard.is_some(),
        "Logging subscriber initialised"
    );
    guard
}

fn make_file_appender(path: &str) -> (tracing_appender::non_blocking::NonBlocking, WorkerGuard) {
    let p = Path::new(path);
    let dir = p.parent().unwrap_or_else(|| Path::new("."));
    let filename = p
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("app.log"));
    let appender = tracing_appender::rolling::never(dir, filename);
    tracing_appender::non_blocking(appender)
}
