//! Application settings stored in `<config>/settings.toml` (D-028).
//!
//! Only settings agreed in DESIGN.md are listed. Missing keys get their default values
//! and unknown keys are ignored, so the file stays compatible across versions.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// All settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub ui: UiSettings,
    pub query: QuerySettings,
    pub export: ExportSettings,
    pub clipboard: ClipboardSettings,
    pub connection: ConnectionSettings,
    pub editor: EditorSettings,
    /// Saved server connections (D-011, D-035); written as `[[servers]]`.
    pub servers: Vec<SavedConnection>,
}

/// SQL editor (D-038).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// Display width of a tab character.
    pub tab_width: u32,
    /// Line number gutter.
    pub line_numbers: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            line_numbers: true,
        }
    }
}

/// Connection defaults (D-035).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionSettings {
    pub connect_timeout_secs: u32,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 10,
        }
    }
}

/// libpq SSL modes (D-033).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl SslMode {
    pub const ALL: [SslMode; 5] = [
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyCa,
        SslMode::VerifyFull,
    ];

    /// libpq name (`verify-full`, ...).
    pub fn name(self) -> &'static str {
        match self {
            SslMode::Disable => "disable",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
            SslMode::VerifyCa => "verify-ca",
            SslMode::VerifyFull => "verify-full",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// A saved server connection (D-011). The password is never stored here (D-008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedConnection {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Maintenance database.
    pub database: String,
    pub username: String,
    pub ssl_mode: SslMode,
    /// Root certificate file (`sslrootcert`, D-033).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssl_root_cert: Option<String>,
}

impl Default for SavedConnection {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: "localhost".to_owned(),
            port: 5432,
            database: "postgres".to_owned(),
            username: "postgres".to_owned(),
            ssl_mode: SslMode::Prefer,
            ssl_root_cert: None,
        }
    }
}

impl SavedConnection {
    /// Same server, database, user and SSL mode (D-031: "identical" connection).
    pub fn same_target(&self, other: &SavedConnection) -> bool {
        self.host == other.host
            && self.port == other.port
            && self.database == other.database
            && self.username == other.username
            && self.ssl_mode == other.ssl_mode
    }
}

/// Look of the application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Nerd Font icons on/off; off by default (D-005).
    pub icons: bool,
    /// Name of the colour scheme; `Classic` by default (D-014).
    pub color_scheme: String,
    /// Show system objects in the tree; off by default (D-036, D-044).
    pub show_system_objects: bool,
    /// Width of the object tree pane in columns, set by dragging the pane border;
    /// `None` = a third of the window (D-045).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree_width: Option<u16>,
    /// Frames and lines drawn with `+ - = |` instead of box-drawing characters
    /// (D-002, D-051); off by default.
    pub ascii_frames: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            icons: false,
            color_scheme: "Classic".to_owned(),
            show_system_objects: false,
            tree_width: None,
            ascii_frames: false,
        }
    }
}

/// Query tab defaults (D-019).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuerySettings {
    /// Rows fetched per batch.
    pub batch_rows: u32,
    /// Auto commit for new query tabs.
    pub auto_commit: bool,
    /// Auto rollback on error for new query tabs.
    pub auto_rollback: bool,
    /// Seconds to wait after a cancel request before offering to close the
    /// connection (D-041).
    pub cancel_wait_secs: u32,
    /// Ask to save unsaved query text when a query tab closes (D-043).
    pub ask_save: bool,
}

impl Default for QuerySettings {
    fn default() -> Self {
        Self {
            batch_rows: 1000,
            auto_commit: true,
            auto_rollback: false,
            cancel_wait_secs: 5,
            ask_save: true,
        }
    }
}

/// Result export options, remembered from the last export (D-021).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// Last used format (D-042).
    pub format: ExportFormat,
    pub csv_separator: CsvSeparator,
    /// Write a UTF-8 BOM at the start of CSV files.
    pub csv_bom: bool,
}

/// Result export format (D-021).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    #[default]
    Csv,
    Json,
    Xml,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [ExportFormat::Csv, ExportFormat::Json, ExportFormat::Xml];

    /// File name extension.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Xml => "xml",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Json => "JSON",
            ExportFormat::Xml => "XML",
        }
    }
}

/// CSV field separator (D-021).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CsvSeparator {
    #[default]
    Comma,
    Semicolon,
    Tab,
}

impl CsvSeparator {
    pub fn as_byte(self) -> u8 {
        match self {
            CsvSeparator::Comma => b',',
            CsvSeparator::Semicolon => b';',
            CsvSeparator::Tab => b'\t',
        }
    }
}

/// Clipboard channels (D-023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardSettings {
    /// Use the system clipboard (`arboard`).
    pub system: bool,
    /// Use OSC 52 (SSH sessions or no system clipboard).
    pub osc52: bool,
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            system: true,
            osc52: true,
        }
    }
}

impl Settings {
    /// Loads settings from `path`. A missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(Error::io(path, e)),
        };
        toml::from_str(&text).map_err(|source| Error::SettingsParse {
            path: path.to_owned(),
            source,
        })
    }

    /// Saves settings to `path`, creating the parent directory if needed.
    /// Writes a temporary file first and renames it, so a crash never leaves a
    /// half-written file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings::load(&dir.path().join("settings.toml")).unwrap();
        assert_eq!(s, Settings::default());
        assert!(!s.ui.icons);
        assert_eq!(s.ui.color_scheme, "Classic");
        assert_eq!(s.query.batch_rows, 1000);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.toml");
        let mut s = Settings::default();
        s.ui.icons = true;
        s.ui.color_scheme = "Dark".into();
        s.export.csv_separator = CsvSeparator::Semicolon;
        s.export.csv_bom = true;
        s.servers.push(SavedConnection {
            name: "local".into(),
            ssl_mode: SslMode::VerifyFull,
            ssl_root_cert: Some("/etc/ca.pem".into()),
            ..SavedConnection::default()
        });
        s.servers.push(SavedConnection::default());
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[servers]]"), "{text}");
        assert!(text.contains("ssl_mode = \"verify-full\""), "{text}");
        assert_eq!(Settings::load(&path).unwrap(), s);
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn partial_file_fills_defaults_and_ignores_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[ui]\nicons = true\nfuture_key = 1\n").unwrap();
        let s = Settings::load(&path).unwrap();
        assert!(s.ui.icons);
        assert_eq!(s.ui.color_scheme, "Classic");
        assert_eq!(s.query, QuerySettings::default());
    }

    #[test]
    fn invalid_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[ui\n").unwrap();
        assert!(matches!(
            Settings::load(&path),
            Err(Error::SettingsParse { .. })
        ));
    }
}
