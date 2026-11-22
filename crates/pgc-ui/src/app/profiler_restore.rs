//! Logging settings the SQL profiler left changed (D-058): the note about them is
//! kept while they are changed, and when the server connects again pgtc offers to
//! put them back.

use std::collections::HashMap;
use std::path::PathBuf;

use pgc_db::DbError;

use super::App;
use crate::dialog::{Button, Dialog, DialogKind};
use crate::profiler::Change;
use crate::profiler_restore::{RestoreFile, Setting, utc_text};

/// A restore running on a server's connection: the answers still expected, and the
/// first error.
#[derive(Debug, Default)]
struct Run {
    keys: Vec<u64>,
    error: Option<String>,
}

/// The note and what is on its way because of it.
#[derive(Debug)]
pub(super) struct Recovery {
    path: PathBuf,
    file: RestoreFile,
    /// Reads of the current settings: lookup key → server uid.
    checks: HashMap<u64, u64>,
    /// Settings still changed on a server, as asked about: server uid → settings.
    asked: HashMap<u64, Vec<Setting>>,
    /// Restores on their way: server uid → run.
    runs: HashMap<u64, Run>,
}

impl Recovery {
    pub(super) fn load(path: PathBuf) -> Self {
        Self {
            file: RestoreFile::load(&path),
            path,
            checks: HashMap::new(),
            asked: HashMap::new(),
            runs: HashMap::new(),
        }
    }

    fn save(&self) {
        if let Err(e) = self.file.save(&self.path) {
            tracing::error!("cannot write {}: {e}", self.path.display());
        }
    }
}

impl App {
    /// Host and port of server `uid`.
    fn server_address(&mut self, uid: u64) -> Option<(String, u16)> {
        let node = self.tree.by_uid(uid)?;
        Some((node.conn.host.clone(), node.conn.port))
    }

    /// Start changed `changes` on server `uid`: noted until they are put back.
    pub(super) fn record_profiler_changes(&mut self, uid: u64, changes: &[Change]) {
        let Some((host, port)) = self.server_address(uid) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.recovery
            .file
            .record(&host, port, &utc_text(now), changes);
        self.recovery.save();
    }

    /// The settings of server `uid` are back (or the user does not want the note).
    pub(super) fn forget_profiler_changes(&mut self, uid: u64) {
        self.recovery.asked.remove(&uid);
        let Some((host, port)) = self.server_address(uid) else {
            return;
        };
        if self.recovery.file.remove(&host, port) {
            self.recovery.save();
        }
    }

    /// Server `uid` connected: when there is a note for it, its current logging
    /// settings are read (the answer decides whether to ask).
    pub(super) fn check_profiler_restore(&mut self, uid: u64) {
        let Some((host, port)) = self.server_address(uid) else {
            return;
        };
        if self.recovery.file.get(&host, port).is_none() {
            return;
        }
        // a profiler of this pgtc is running there: the settings are its own, and it
        // puts them back itself
        if self.tabs.iter().any(|t| {
            t.profiler()
                .is_some_and(|p| p.own.uid == uid && p.needs_restore())
        }) {
            return;
        }
        let Some(version) = self.tree.by_uid(uid).and_then(|n| n.version) else {
            return;
        };
        let sql = match self.render_profiler_sql(version, "check.sql") {
            Ok(sql) => sql,
            Err(e) => {
                tracing::error!("{e}");
                return;
            }
        };
        self.next_key += 1;
        let key = self.next_key;
        if let Some(handle) = self.tree.by_uid(uid).and_then(|n| n.handle.as_ref()) {
            handle.lookup(key, sql);
            self.recovery.checks.insert(key, uid);
        }
    }

    /// An answer on the tree connection of server `uid`; false if `key` is not one of
    /// the recovery's lookups.
    pub(super) fn profiler_restore_loaded(
        &mut self,
        uid: u64,
        key: u64,
        result: &Result<Vec<Vec<String>>, DbError>,
    ) -> bool {
        if self.recovery.checks.remove(&key).is_some() {
            self.profiler_restore_checked(uid, result);
            return true;
        }
        let Some(run) = self.recovery.runs.get_mut(&uid) else {
            return false;
        };
        let Some(at) = run.keys.iter().position(|k| *k == key) else {
            return false;
        };
        run.keys.remove(at);
        if let Err(e) = result {
            run.error.get_or_insert_with(|| e.to_string());
        }
        if run.keys.is_empty() {
            let error = self.recovery.runs.remove(&uid).and_then(|r| r.error);
            self.profiler_restore_done(uid, error);
        }
        true
    }

    /// The current settings of server `uid` (the row of `check.sql`): the note is
    /// dropped when nothing is changed any more, else the question is asked.
    fn profiler_restore_checked(&mut self, uid: u64, result: &Result<Vec<Vec<String>>, DbError>) {
        let row = match result {
            Ok(rows) => rows.first().cloned().unwrap_or_default(),
            Err(e) => {
                // asked again at the next connect
                tracing::warn!("cannot read the logging settings: {e}");
                return;
            }
        };
        let (Some(destination), Some(min_duration)) = (row.get(1), row.get(3)) else {
            return;
        };
        let Some((host, port)) = self.server_address(uid) else {
            return;
        };
        let Some(entry) = self.recovery.file.get(&host, port) else {
            return;
        };
        let still = entry.still_changed(destination, min_duration);
        if still.is_empty() {
            self.forget_profiler_changes(uid);
            return;
        }
        let name = self
            .tree
            .by_uid(uid)
            .map(|n| n.conn.name.clone())
            .unwrap_or_default();
        let list: String = still.iter().map(|s| format!("  {}\n", s.text())).collect();
        let text = format!(
            "pgtc ended without putting back these logging settings\n\
             on {name} (changed {}):\n\n{list}\n\
             Put them back now? (ALTER SYSTEM and a configuration reload)\n\n\
             Not now asks again at the next connect. Forget removes the\n\
             note; it is kept in\n{}",
            entry.changed,
            self.recovery.path.display()
        );
        self.recovery.asked.insert(uid, still);
        self.dialogs.push(Dialog::confirm_buttons(
            DialogKind::ConfirmProfilerRestore { server: uid },
            "SQL profiler",
            &text,
            &[
                (Button::Yes, "&Yes"),
                (Button::No, "&Not now"),
                (Button::Forget, "&Forget"),
            ],
        ));
    }

    /// Yes: the settings asked about are put back on the tree connection of server
    /// `uid`, each statement on its own (ALTER SYSTEM cannot run in a transaction
    /// block), then the configuration is reloaded.
    pub(super) fn run_profiler_restore(&mut self, uid: u64) {
        let Some(settings) = self.recovery.asked.remove(&uid) else {
            return;
        };
        let mut statements: Vec<String> = settings
            .iter()
            .filter_map(Setting::change)
            .map(|c| c.restore_sql())
            .collect();
        let reload = self
            .tree
            .by_uid(uid)
            .and_then(|n| n.version)
            .map(|v| self.render_profiler_sql(v, "reload.sql"));
        match reload {
            Some(Ok(sql)) => statements.push(sql),
            Some(Err(e)) => tracing::error!("{e}"),
            None => {}
        }
        let keys: Vec<u64> = statements
            .iter()
            .map(|_| {
                self.next_key += 1;
                self.next_key
            })
            .collect();
        let Some(handle) = self.tree.by_uid(uid).and_then(|n| n.handle.as_ref()) else {
            self.status = "The server is not connected; the settings are not put back".to_owned();
            return;
        };
        for (key, sql) in keys.iter().zip(statements) {
            handle.lookup(*key, sql);
        }
        self.recovery.runs.insert(uid, Run { keys, error: None });
        self.status = "Restoring logging settings…".to_owned();
    }

    fn profiler_restore_done(&mut self, uid: u64, error: Option<String>) {
        match error {
            None => {
                self.forget_profiler_changes(uid);
                self.status = "Logging settings restored".to_owned();
            }
            Some(e) => {
                tracing::error!("logging settings not restored: {e}");
                self.status.clear();
                self.error(&format!(
                    "The logging settings could not be put back — this needs a\n\
                     superuser (or ALTER SYSTEM rights):\n{e}\n\n\
                     pgtc asks again when the server connects."
                ));
            }
        }
    }
}
