//! Shared basics used by several pgtc crates (D-018).
//!
//! Rule: only small, stable types and functions used by at least two crates; no heavy
//! dependencies (no tokio-postgres, ratatui, minijinja).

pub mod error;
pub mod logging;
pub mod paths;
pub mod quote;
pub mod settings;
pub mod version;

pub use error::{Error, Result};
pub use version::{Feature, ServerVersion};

/// Short name: executable, folders, environment variables (D-030).
pub const APP_NAME: &str = "pgtc";

/// Long application name shown in `--help` and About (D-030).
pub const APP_LONG_NAME: &str = "Postgres terminal commander";

/// Version of the application (all crates share the workspace version).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
