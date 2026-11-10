//! Common error type of `pgc-core`.

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by `pgc-core`.
#[derive(Debug, Error)]
pub enum Error {
    /// The OS did not provide a home directory, so no user paths can be computed.
    #[error("cannot determine the user's home directory")]
    NoHomeDir,

    /// File system operation failed.
    #[error("I/O error on {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// `settings.toml` exists but cannot be parsed.
    #[error("invalid settings file {}", path.display())]
    SettingsParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// Settings cannot be serialised to TOML.
    #[error("cannot serialise settings")]
    SettingsSerialize(#[from] toml::ser::Error),

    /// Logging could not be initialised.
    #[error("cannot initialise logging: {0}")]
    Logging(String),
}

impl Error {
    /// Helper for wrapping an [`std::io::Error`] together with the path it concerns.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

/// Result type of `pgc-core`.
pub type Result<T> = std::result::Result<T, Error>;
