//! File logging (D-007, D-029): daily rolling `pgtc.YYYY-MM-DD.log`, last 7 files kept,
//! level from `PGTC_LOG` (default `info`). Never writes to stdout/stderr.

use std::path::Path;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

use crate::{APP_NAME, Error, Result};

/// Environment variable with a `tracing-subscriber` filter, e.g. `debug` or `pgc_db=trace`.
pub const LOG_ENV_VAR: &str = "PGTC_LOG";

const DEFAULT_LEVEL: &str = "info";
const KEEP_FILES: usize = 7;

/// Keeps the background log writer alive; logs are flushed when it is dropped.
/// Hold it until the application exits.
#[must_use = "dropping the guard stops logging"]
pub struct LogGuard(#[allow(dead_code)] tracing_appender::non_blocking::WorkerGuard);

/// Initialises the global `tracing` subscriber writing into `logs_dir`.
///
/// The directory is created if it does not exist.
pub fn init_logging(logs_dir: &Path) -> Result<LogGuard> {
    std::fs::create_dir_all(logs_dir).map_err(|e| Error::io(logs_dir, e))?;

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(APP_NAME)
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(logs_dir)
        .map_err(|e| Error::Logging(e.to_string()))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let (filter, bad_filter) = match EnvFilter::try_from_env(LOG_ENV_VAR) {
        Ok(f) => (f, None),
        Err(e) => {
            // Unset is normal; a set but invalid value is reported once logging works.
            let bad = std::env::var(LOG_ENV_VAR).ok().map(|v| (v, e.to_string()));
            (EnvFilter::new(DEFAULT_LEVEL), bad)
        }
    };

    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init()
        .map_err(|e| Error::Logging(e.to_string()))?;

    if let Some((value, err)) = bad_filter {
        tracing::warn!("invalid {LOG_ENV_VAR}={value:?} ({err}); using '{DEFAULT_LEVEL}'");
    }

    Ok(LogGuard(guard))
}
