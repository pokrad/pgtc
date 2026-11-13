//! The database statistics tab (D-062): opening it, its own connection, reading the
//! statistics (only on `F5`), its keys.

use pgc_db::{ConnId, ConnectParams, DbError, DbEvent};
use pgc_sql::RenderContext;
use ratatui::layout::Position;

use super::{App, Focus, Tab, TabKind, Target, TextView};
use crate::dashboard::DashConn;
use crate::db_stats::DbStats;
use crate::keymap::Command;
use crate::menu::{self, Popup};
use crate::tree::ConnState;
use crate::view::Hits;

const SQL_FOLDER: &str = "db_stats";

impl App {
    /// The statistics of the active tab.
    pub(super) fn db_stats(&self) -> Option<&DbStats> {
        self.tabs[self.active_tab].db_stats.as_ref()
    }

    fn db_stats_mut(&mut self) -> Option<&mut DbStats> {
        self.tabs[self.active_tab].db_stats.as_mut()
    }

    fn db_stats_of(&mut self, tab: u64) -> Option<&mut DbStats> {
        self.tab_mut(tab).and_then(|t| t.db_stats.as_mut())
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
            t.db_stats
                .as_ref()
                .is_some_and(|s| s.uid == uid && s.database == db)
        }) {
            self.active_tab = i;
            self.focus = Focus::Results;
            return;
        }
        let label = format!("Stats: {} · {db}", server.conn.name);
        let view = TextView::new(true, self.tab_width());
        let id = self.add_tab(TabKind::DbStats, view);
        if let Some(t) = self.tab_mut(id) {
            t.server = Some(uid);
            t.database = Some(db.clone());
            t.db_stats = Some(DbStats::new(uid, db, label));
        }
        self.focus = Focus::Results;
        self.db_stats_connect(id);
    }

    /// Opens the tab's own connection to the database (D-011, D-062).
    fn db_stats_connect(&mut self, tab: u64) {
        let timeout = self.timeout();
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(s) = self.tabs[i].db_stats.as_ref() else {
            return;
        };
        let (uid, database) = (s.uid, s.database.clone());
        let Some(server) = self
            .tree
            .servers
            .iter()
            .find(|s| s.uid == uid && s.state == ConnState::Connected)
        else {
            if let Some(s) = self.tabs[i].db_stats.as_mut() {
                s.problem = Some("The server is not connected.".to_owned());
            }
            return;
        };
        let password = server
            .password
            .clone()
            .or_else(|| Self::pgpass_password(&server.conn, &database));
        let params = ConnectParams::new(&server.conn, &database, password, timeout);
        self.next_conn += 1;
        let id = self.next_conn;
        let handle = pgc_db::spawn(id, params, self.sink.clone());
        self.connections.insert(id, Target::DbStats(tab));
        if let Some(s) = self.tabs[i].db_stats.as_mut() {
            s.conn = Some(DashConn {
                id,
                handle,
                connected: false,
            });
            s.problem = None;
            s.error = None;
            s.key = None;
        }
    }

    pub(super) fn on_db_stats_event(&mut self, conn: ConnId, tab: u64, event: DbEvent) {
        if self.db_stats_of(tab).is_none() {
            self.connections.remove(&conn);
            return;
        }
        match event {
            DbEvent::Connected { version, .. } => {
                if let Some(s) = self.db_stats_of(tab) {
                    s.version = Some(version);
                    if let Some(c) = s.conn.as_mut() {
                        c.connected = true;
                    }
                }
                self.db_stats_refresh(tab);
            }
            DbEvent::ConnectFailed { error, .. } => {
                self.connections.remove(&conn);
                if let Some(s) = self.db_stats_of(tab) {
                    s.conn = None;
                    s.problem = Some(format!("Cannot connect: {error}"));
                }
            }
            DbEvent::Closed { error, .. } => {
                self.connections.remove(&conn);
                if let Some(s) = self.db_stats_of(tab) {
                    s.conn = None;
                    s.key = None;
                    s.problem = Some(match error {
                        Some(e) => format!("Connection lost: {e}"),
                        None => "Connection closed".to_owned(),
                    });
                }
            }
            DbEvent::Lookup { key, result, .. } => self.db_stats_loaded(tab, key, result),
            _ => {}
        }
    }

    /// Reads the statistics, unless a read is on its way already.
    pub(super) fn db_stats_refresh(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(s) = self.tabs[i].db_stats.as_ref() else {
            return;
        };
        let (Some(version), true, None) = (s.version, s.connected(), s.key) else {
            return;
        };
        let ctx = RenderContext {
            version,
            show_system: self.settings.ui.show_system_objects,
        };
        let sql = self
            .sql
            .render(SQL_FOLDER, "stats.sql", ctx)
            .map_err(|e| e.to_string());
        self.next_key += 1;
        let key = self.next_key;
        let Some(s) = self.tabs[i].db_stats.as_mut() else {
            return;
        };
        match sql {
            Ok(sql) => {
                if let Some(conn) = s.conn.as_ref() {
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
        let page = self.grid_size.0.saturating_sub(1).max(1) as isize;
        if self.db_stats().is_none() {
            return false;
        }
        let step = match cmd {
            Command::Up => Some((-1, 0)),
            Command::Down => Some((1, 0)),
            Command::Left => Some((0, -1)),
            Command::Right => Some((0, 1)),
            Command::PageUp => Some((-page, 0)),
            Command::PageDown => Some((page, 0)),
            Command::Home => Some((0, isize::MIN / 2)),
            Command::End => Some((0, isize::MAX / 2)),
            Command::DocStart => Some((isize::MIN / 2, isize::MIN / 2)),
            Command::DocEnd => Some((isize::MAX / 2, 0)),
            _ => None,
        };
        if let Some((dr, dc)) = step {
            if let Some(s) = self.db_stats_mut() {
                s.grid.move_cursor(dr, dc, 0);
            }
            self.db_stats_scroll();
            return true;
        }
        match cmd {
            Command::DbStatsRefresh => {
                if self.db_stats().is_some_and(|s| s.conn.is_none()) {
                    self.db_stats_connect(tab);
                    self.status = "Connecting…".to_owned();
                } else {
                    self.db_stats_refresh(tab);
                }
            }
            Command::ContextMenu => {
                let a = self.grid_area;
                self.open_db_stats_menu((a.x + 2, a.y + 1));
            }
            Command::Copy => {
                let text = self.db_stats().and_then(|s| s.grid.copy_text());
                if let Some(text) = text
                    && self.clipboard.copy(&text)
                {
                    self.effects.push(super::Effect::Osc52(text));
                }
            }
            Command::SelectAll => {
                if let Some(s) = self.db_stats_mut() {
                    s.grid.all_selected = true;
                }
            }
            _ => return false,
        }
        true
    }

    /// Keeps the grid's cursor visible.
    fn db_stats_scroll(&mut self) {
        let (h, w) = self.grid_size;
        if let Some(s) = self.db_stats_mut() {
            s.grid.ensure_visible(h.saturating_sub(1), w);
        }
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

    /// Mouse in the statistics tab. Returns true if the event was used.
    pub(super) fn db_stats_mouse(
        &mut self,
        kind: crossterm::event::MouseEventKind,
        p: Position,
        hits: &Hits,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        if self.db_stats().is_none() {
            return false;
        }
        let sh = &hits.db_stats;
        match kind {
            MouseEventKind::Down(b @ (MouseButton::Left | MouseButton::Right))
                if sh.grid.contains(p) =>
            {
                self.focus = Focus::Results;
                let cell = sh
                    .cells
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(rc, _)| *rc);
                if let Some(s) = self.db_stats_mut()
                    && let Some((row, col)) = cell
                {
                    s.grid.row = row;
                    s.grid.col = col;
                    s.grid.all_selected = false;
                }
                if b == MouseButton::Right {
                    self.open_db_stats_menu((p.x, p.y + 1));
                }
                true
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if sh.grid.contains(p) => {
                let delta = if kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if let Some(s) = self.db_stats_mut() {
                    s.grid.move_cursor(delta, 0, 0);
                }
                self.db_stats_scroll();
                true
            }
            _ => false,
        }
    }

    /// `server · user@database` and when the values were read (D-043, D-062).
    pub(crate) fn db_stats_status(&self, tab: &Tab) -> Option<(String, String)> {
        let s = tab.db_stats.as_ref()?;
        let place = self.place_label(s.uid, Some(&s.database));
        let left = match &s.conn {
            Some(c) if c.connected => place,
            Some(_) => format!("{place} – connecting…"),
            None => format!("{place} – not connected"),
        };
        let right = if s.key.is_some() {
            "Reading…".to_owned()
        } else {
            match s.read_at() {
                Some(t) => format!("Read at {t} (F5 refreshes)"),
                None => String::new(),
            }
        };
        Some((left, right))
    }
}
