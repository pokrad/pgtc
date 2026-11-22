//! The database statistics tab (D-062): opening it, its own connection, reading the
//! statistics (only on `F5`), its keys.

use pgc_db::{ConnId, DbError, DbEvent};

use super::connections::MonitorEvent;
use super::{App, Focus, TabBody, Target, TextView};
use crate::db_stats::DbStats;
use crate::keymap::Command;
use crate::menu::{self, Popup};
use crate::tree::ConnState;

const SQL_FOLDER: &str = "db_stats";

impl App {
    /// The statistics of the active tab.
    pub(super) fn db_stats(&self) -> Option<&DbStats> {
        self.tabs[self.active_tab].db_stats()
    }

    fn db_stats_of(&mut self, tab: u64) -> Option<&mut DbStats> {
        self.tab_mut(tab).and_then(|t| t.db_stats_mut())
    }

    /// Server (index) and database of a selected database row of a connected server.
    pub(super) fn db_stats_target(&self) -> Option<(usize, String)> {
        let row = self.tree.selected()?;
        if self.tree.servers[row.server].state != ConnState::Connected {
            return None;
        }
        let (_, db) = self.selected_database()?;
        Some((row.server, db))
    }

    /// `Alt+I` / context menu: switches to an open statistics tab of the same database,
    /// else opens a new one (D-062).
    pub(super) fn open_db_stats(&mut self) {
        let Some((si, db)) = self.db_stats_target() else {
            self.status = "Select a database of a connected server first".to_owned();
            return;
        };
        let server = &self.tree.servers[si];
        let uid = server.uid;
        if let Some(i) = self.tabs.iter().position(|t| {
            t.db_stats()
                .is_some_and(|s| s.own.uid == uid && s.own.database == db)
        }) {
            self.active_tab = i;
            self.focus = Focus::Results;
            return;
        }
        let label = format!("Stats: {} · {db}", server.conn.name);
        let view = TextView::new(true, self.tab_width());
        let stats = DbStats::new(uid, db.clone(), label);
        let id = self.add_tab(TabBody::DbStats(Box::new(stats)), view);
        if let Some(t) = self.tab_mut(id) {
            t.server = Some(uid);
            t.database = Some(db);
        }
        self.focus = Focus::Results;
        self.db_stats_connect(id);
    }

    /// Opens the tab's own connection to the database (D-011, D-062).
    fn db_stats_connect(&mut self, tab: u64) {
        if self.monitor_connect(tab, Target::DbStats(tab))
            && let Some(s) = self.db_stats_of(tab)
        {
            s.error = None;
            s.key = None;
        }
    }

    pub(super) fn on_db_stats_event(&mut self, conn: ConnId, tab: u64, event: DbEvent) {
        match self.monitor_event(conn, tab, &event) {
            MonitorEvent::Connected => self.db_stats_refresh(tab),
            MonitorEvent::Closed => {
                if let Some(s) = self.db_stats_of(tab) {
                    s.key = None;
                }
            }
            MonitorEvent::Other => {
                if let DbEvent::Lookup { key, result, .. } = event {
                    self.db_stats_loaded(tab, key, result);
                }
            }
            MonitorEvent::Gone | MonitorEvent::Failed => {}
        }
    }

    /// Reads the statistics, unless a read is on its way already.
    pub(super) fn db_stats_refresh(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(s) = self.tabs[i].db_stats() else {
            return;
        };
        let (Some(version), true, None) = (s.own.version, s.own.connected(), s.key) else {
            return;
        };
        let sql = self.render_monitor_sql(SQL_FOLDER, version, "stats.sql");
        self.next_key += 1;
        let key = self.next_key;
        let Some(s) = self.tabs[i].db_stats_mut() else {
            return;
        };
        match sql {
            Ok(sql) => {
                if let Some(conn) = s.own.conn.as_ref() {
                    conn.handle.lookup(key, sql);
                    s.key = Some(key);
                }
            }
            Err(e) => {
                tracing::error!("{e}");
                s.error = Some(e);
            }
        }
    }

    fn db_stats_loaded(&mut self, tab: u64, key: u64, result: Result<Vec<Vec<String>>, DbError>) {
        let Some(s) = self.db_stats_of(tab) else {
            return;
        };
        if s.key != Some(key) {
            return;
        }
        s.key = None;
        match result {
            Ok(rows) => s.apply_rows(&rows),
            Err(e) => s.error = Some(e.to_string()),
        }
    }

    /// Keys of the statistics tab. Returns false if the command is not for it.
    pub(super) fn db_stats_command(&mut self, cmd: Command) -> bool {
        let tab = self.tabs[self.active_tab].id;
        if self.db_stats().is_none() {
            return false;
        }
        // the list: cursor, copy, select all
        if self.monitor_command(cmd) {
            return true;
        }
        match cmd {
            Command::DbStatsRefresh => {
                if self.db_stats().is_some_and(|s| s.own.conn.is_none()) {
                    self.db_stats_connect(tab);
                    self.status = "Connecting…".to_owned();
                } else {
                    self.db_stats_refresh(tab);
                }
            }
            Command::ContextMenu => {
                let a = self.frame.grid_area;
                self.open_db_stats_menu((a.x + 2, a.y + 1));
            }
            _ => return false,
        }
        true
    }

    /// Context menu of the statistics tab (D-062).
    pub(super) fn open_db_stats_menu(&mut self, anchor: (u16, u16)) {
        let entries = vec![
            menu::item("&Refresh", Command::DbStatsRefresh),
            menu::item("&Copy", Command::Copy),
            menu::item("Select &all", Command::SelectAll),
        ];
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }
}
