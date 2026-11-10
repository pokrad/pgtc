//! Database errors, formatted for the user.

use std::error::Error as _;

use thiserror::Error;
use tokio_postgres::error::SqlState;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DbError {
    /// The server asked for a password and none was given.
    #[error("password required")]
    PasswordRequired,
    /// Password authentication failed.
    #[error("{0}")]
    AuthFailed(String),
    #[error("{0}")]
    Other(String),
}

impl DbError {
    pub fn from_pg(e: &tokio_postgres::Error) -> Self {
        if let Some(db) = e.as_db_error()
            && *db.code() == SqlState::INVALID_PASSWORD
        {
            return DbError::AuthFailed(describe(e));
        }
        // tokio-postgres reports a missing password as a configuration error whose
        // source says "password missing".
        let text = describe(e);
        if is_password_missing(&text) {
            return DbError::PasswordRequired;
        }
        DbError::Other(text)
    }
}

fn is_password_missing(described: &str) -> bool {
    described.contains("password missing")
}

/// `ERROR: message` + `DETAIL:` / `HINT:` lines for server errors; otherwise the
/// error with its source chain (`error connecting to server: ...`).
pub fn describe(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        let mut s = format!("{}: {}", db.severity(), db.message());
        if let Some(d) = db.detail() {
            s.push_str(&format!("\nDETAIL: {d}"));
        }
        if let Some(h) = db.hint() {
            s.push_str(&format!("\nHINT: {h}"));
        }
        return s;
    }
    let mut s = e.to_string();
    let mut source = e.source();
    while let Some(inner) = source {
        let text = inner.to_string();
        if !s.contains(&text) {
            s.push_str(": ");
            s.push_str(&text);
        }
        source = inner.source();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text produced by `describe` for tokio-postgres' missing-password error
    /// (seen against a real server).
    #[test]
    fn missing_password_is_recognised() {
        assert!(is_password_missing(
            "invalid configuration: password missing"
        ));
        assert!(!is_password_missing("error connecting to server: refused"));
    }
}
