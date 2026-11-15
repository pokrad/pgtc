//! The note about logging settings the SQL profiler changed and has not put back yet
//! (D-058): `<data>/profiler_restore.toml`, one entry per server. It lets pgtc offer
//! the restore after it ended without doing it (crash, lost connection, no rights).
//! No passwords are stored.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::profiler::Change;

/// The settings the profiler changes; only these are ever restored from the file.
const KNOWN: [&str; 2] = ["log_destination", "log_min_duration_statement"];

/// One changed setting: what the profiler set, and what was there before (`None`:
/// nothing of its own, so it is reset).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<String>,
}

impl Setting {
    /// The change as the profiler knows it; `None` for a name pgtc never changes (a
    /// file edited by hand).
    pub fn change(&self) -> Option<Change> {
        let name = KNOWN.into_iter().find(|n| *n == self.name)?;
        Some(Change {
            name,
            value: self.value.clone(),
            restore: self.restore.clone(),
        })
    }

    /// `name = 'value' (before: …)`, for the question.
    pub fn text(&self) -> String {
        let before = match &self.restore {
            Some(old) => format!("'{old}'"),
            None => "not set".to_owned(),
        };
        format!("{} = '{}'  (before: {before})", self.name, self.value)
    }
}

/// The changed settings of one server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub host: String,
    pub port: u16,
    /// When they were changed (`2026-11-15 14:03 UTC`).
    pub changed: String,
    #[serde(default)]
    pub settings: Vec<Setting>,
}

impl Entry {
    /// The settings that still have the profiler's value on the server, given the
    /// current `log_destination` and `log_min_duration_statement`.
    pub fn still_changed(&self, log_destination: &str, min_duration: &str) -> Vec<Setting> {
        self.settings
            .iter()
            .filter(|s| match s.name.as_str() {
                "log_destination" => list(&s.value) == list(log_destination),
                "log_min_duration_statement" => s.value.trim() == min_duration.trim(),
                _ => false,
            })
            .cloned()
            .collect()
    }
}

/// A comma list as the server compares it: trimmed, lower case, sorted.
fn list(value: &str) -> Vec<String> {
    let mut parts: Vec<String> = value
        .split(',')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    parts.sort();
    parts
}

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreFile {
    #[serde(default, rename = "server")]
    pub servers: Vec<Entry>,
}

impl RestoreFile {
    /// Reads the file; a missing or unreadable one is empty (the problem is logged).
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("cannot read {}: {e}", path.display());
                }
                return Self::default();
            }
        };
        toml::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("{}: {e}", path.display());
            Self::default()
        })
    }

    /// Writes the file; without entries it is removed.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if self.servers.is_empty() {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        pgc_core::fsutil::atomic_write(path, text.as_bytes(), pgc_core::fsutil::Access::Default)
    }

    pub fn get(&self, host: &str, port: u16) -> Option<&Entry> {
        self.servers
            .iter()
            .find(|e| e.host == host && e.port == port)
    }

    /// Notes `changes` for the server. A setting that is noted already keeps its note:
    /// what was there before the first change is what must come back.
    pub fn record(&mut self, host: &str, port: u16, changed: &str, changes: &[Change]) {
        if changes.is_empty() {
            return;
        }
        let at = match self
            .servers
            .iter()
            .position(|e| e.host == host && e.port == port)
        {
            Some(i) => i,
            None => {
                self.servers.push(Entry {
                    host: host.to_owned(),
                    port,
                    changed: changed.to_owned(),
                    settings: Vec::new(),
                });
                self.servers.len() - 1
            }
        };
        let entry = &mut self.servers[at];
        for c in changes {
            if !entry.settings.iter().any(|s| s.name == c.name) {
                entry.settings.push(Setting {
                    name: c.name.to_owned(),
                    value: c.value.clone(),
                    restore: c.restore.clone(),
                });
            }
        }
    }

    /// Removes the server's entry; true if there was one.
    pub fn remove(&mut self, host: &str, port: u16) -> bool {
        let before = self.servers.len();
        self.servers.retain(|e| !(e.host == host && e.port == port));
        self.servers.len() != before
    }
}

/// `2026-11-15 14:03 UTC` for seconds since 1970 (no time zone data needed).
pub fn utc_text(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // civil date from the day number (proleptic Gregorian calendar)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3_600,
        rest % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(name: &'static str, value: &str, restore: Option<&str>) -> Change {
        Change {
            name,
            value: value.to_owned(),
            restore: restore.map(str::to_owned),
        }
    }

    #[test]
    fn file_round_trip_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("profiler_restore.toml");
        assert_eq!(RestoreFile::load(&path), RestoreFile::default());
        let mut f = RestoreFile::default();
        f.record(
            "db1",
            5432,
            "2026-11-15 14:03 UTC",
            &[
                change("log_destination", "stderr,jsonlog", None),
                change("log_min_duration_statement", "0", Some("250")),
            ],
        );
        f.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[server]]"), "{text}");
        assert!(!text.to_lowercase().contains("password"), "{text}");
        assert_eq!(RestoreFile::load(&path), f);
        // another port is another server
        assert!(f.get("db1", 5433).is_none());
        assert!(f.remove("db1", 5432));
        assert!(!f.remove("db1", 5432));
        f.save(&path).unwrap();
        assert!(!path.exists(), "an empty note is no file");
        // a broken file is as no file
        std::fs::write(&path, "[[server").unwrap();
        assert_eq!(RestoreFile::load(&path), RestoreFile::default());
    }

    #[test]
    fn the_first_note_of_a_setting_stays() {
        let mut f = RestoreFile::default();
        f.record(
            "h",
            1,
            "first",
            &[change("log_min_duration_statement", "0", Some("250"))],
        );
        // started again without the restore: "before" is now the profiler's own value
        f.record(
            "h",
            1,
            "second",
            &[
                change("log_min_duration_statement", "0", Some("0")),
                change("log_destination", "stderr,csvlog", None),
            ],
        );
        let e = f.get("h", 1).unwrap();
        assert_eq!(e.changed, "first");
        assert_eq!(e.settings.len(), 2);
        assert_eq!(e.settings[0].restore.as_deref(), Some("250"));
        f.record("h", 1, "third", &[]);
        assert_eq!(f.servers.len(), 1);
    }

    #[test]
    fn still_changed_compares_as_the_server_does() {
        let mut f = RestoreFile::default();
        f.record(
            "h",
            1,
            "t",
            &[
                change("log_destination", "stderr,jsonlog", None),
                change("log_min_duration_statement", "0", Some("250")),
            ],
        );
        let e = f.get("h", 1).unwrap();
        assert_eq!(e.still_changed("jsonlog, STDERR", " 0 ").len(), 2);
        // someone put the duration back by hand
        let left = e.still_changed("stderr,jsonlog", "250");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].name, "log_destination");
        assert!(e.still_changed("stderr", "-1").is_empty());
    }

    #[test]
    fn only_known_settings_become_statements() {
        let known = Setting {
            name: "log_min_duration_statement".into(),
            value: "0".into(),
            restore: None,
        };
        assert_eq!(
            known.change().unwrap().restore_sql(),
            "ALTER SYSTEM RESET log_min_duration_statement"
        );
        assert_eq!(
            known.text(),
            "log_min_duration_statement = '0'  (before: not set)"
        );
        let edited = Setting {
            name: "x; DROP TABLE t".into(),
            value: String::new(),
            restore: None,
        };
        assert!(edited.change().is_none());
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc_text(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_text(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc_text(1_794_751_380), "2026-11-15 14:03 UTC");
    }
}
