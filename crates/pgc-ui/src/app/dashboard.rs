//! The server dashboard tab (D-057): opening it, its own connection, the refresh
//! cycle, its keys, cancel query / terminate session.

use std::time::Duration;

use pgc_db::{ConnId, ConnectParams, DbError, DbEvent};
use pgc_sql::RenderContext;
use ratatui::layout::Position;

use super::{App, Effect, Focus, Tab, TabKind, Target, TextView};
use crate::dashboard::{DashConn, Dashboard, List, Session, Signal};
use crate::dialog::{Dialog, DialogKind};
use crate::keymap::Command;
use crate::menu::{self, MenuEntry, Popup};
use crate::tree::{ConnState, RowKind};
use crate::view::Hits;

const SQL_FOLDER: &str = "dashboard";

impl App {
    /// The dashboard of the active tab.
    pub(super) fn dashboard(&self) -> Option<&Dashboard> {
        self.tabs[self.active_tab].dashboard.as_ref()
    }

    fn dashboard_mut(&mut self) -> Option<&mut Dashboard> {
        self.tabs[self.active_tab].dashboard.as_mut()
    }

    fn dashboard_of(&mut self, tab: u64) -> Option<&mut Dashboard> {
        self.tab_mut(tab).and_then(|t| t.dashboard.as_mut())
    }

    /// Server (index) and database filter for a dashboard of the tree selection: a
    /// server row or a server-level row → the whole server; a database or a row in it →
    /// that database. The server must be connected (D-057).
    pub(super) fn dashboard_target(&self) -> Option<(usize, Option<String>)> {
        let row = self.tree.selected()?;
        if self.tree.servers[row.server].state != ConnState::Connected {
            return None;
        }
        let db = match row.kind {
            RowKind::Node(nid) | RowKind::Loading(nid) => self.tree.database_name_of(nid),
            RowKind::Server => None,
        };
        Some((row.server, db))
    }

    /// Tools → Server dashboard: switches to an open dashboard of the same server and
    /// database, else opens a new one (D-057).
    pub(super) fn open_dashboard(&mut self) {
        let Some((si, db)) = self.dashboard_target() else {
            self.status = "Select a connected server or database first".to_owned();
            return;
        };
        let server = &self.tree.servers[si];
        let uid = server.uid;
        if let Some(i) = self.tabs.iter().position(|t| {
            t.dashboard
                .as_ref()
                .is_some_and(|d| d.uid == uid && d.database == db)
        }) {
            self.active_tab = i;
            self.focus = Focus::Tabs;
            return;
        }
        let label = match &db {
            Some(db) => format!("Dashboard: {} · {db}", server.conn.name),
            None => format!("Dashboard: {}", server.conn.name),
        };
        let conn_db = db.clone().unwrap_or_else(|| server.conn.database.clone());
        let view = TextView::new(true, self.tab_width());
        let id = self.add_tab(TabKind::Dashboard, view);
        if let Some(t) = self.tab_mut(id) {
            t.server = Some(uid);
            t.database = db.clone();
            t.dashboard = Some(Dashboard::new(uid, db, conn_db, label));
        }
        self.dashboard_connect(id);
    }

    /// Opens the dashboard's own connection (D-011, D-057).
    fn dashboard_connect(&mut self, tab: u64) {
        let timeout = self.timeout();
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(d) = self.tabs[i].dashboard.as_ref() else {
            return;
        };
        let (uid, database) = (d.uid, d.conn_database.clone());
        let Some(server) = self
            .tree
            .servers
            .iter()
            .find(|s| s.uid == uid && s.state == ConnState::Connected)
        else {
            if let Some(d) = self.tabs[i].dashboard.as_mut() {
                d.problem = Some("The server is not connected.".to_owned());
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
        self.connections.insert(id, Target::Dashboard(tab));
        if let Some(d) = self.tabs[i].dashboard.as_mut() {
            d.conn = Some(DashConn {
                id,
                handle,
                connected: false,
            });
            d.problem = None;
            d.error = None;
            d.stats_key = None;
            d.list_key = None;
            d.signal_key = None;
        }
    }

    pub(super) fn on_dashboard_event(&mut self, conn: ConnId, tab: u64, event: DbEvent) {
        if self.dashboard_of(tab).is_none() {
            self.connections.remove(&conn);
            return;
        }
        match event {
            DbEvent::Connected { version, .. } => {
                if let Some(d) = self.dashboard_of(tab) {
                    d.version = Some(version);
                    if let Some(c) = d.conn.as_mut() {
                        c.connected = true;
                    }
                }
                self.dashboard_refresh(tab, true);
            }
            DbEvent::ConnectFailed { error, .. } => {
                self.connections.remove(&conn);
                if let Some(d) = self.dashboard_of(tab) {
                    d.conn = None;
                    d.problem = Some(format!("Cannot connect: {error}"));
                }
            }
            DbEvent::Closed { error, .. } => {
                self.connections.remove(&conn);
                if let Some(d) = self.dashboard_of(tab) {
                    d.conn = None;
                    d.problem = Some(match error {
                        Some(e) => format!("Connection lost: {e}"),
                        None => "Connection closed".to_owned(),
                    });
                }
            }
            DbEvent::Lookup { key, result, .. } => self.dashboard_loaded(tab, key, result),
            _ => {}
        }
    }

    fn render_dashboard_sql(
        &self,
        version: pgc_core::ServerVersion,
        file: &str,
    ) -> Result<String, String> {
        let ctx = RenderContext {
            version,
            show_system: self.settings.ui.show_system_objects,
        };
        self.sql
            .render(SQL_FOLDER, file, ctx)
            .map_err(|e| e.to_string())
    }

    /// Sends the stats query (`stats`) and the shown list's query, unless they are on
    /// their way already.
    pub(super) fn dashboard_refresh(&mut self, tab: u64, stats: bool) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(d) = self.tabs[i].dashboard.as_ref() else {
            return;
        };
        let (Some(version), true) = (d.version, d.connected()) else {
            return;
        };
        let list = d.list;
        let filter = d.database.clone();
        let send_stats = stats && d.stats_key.is_none();
        let send_list = d.list_key.is_none();
        let stats_sql = send_stats.then(|| self.render_dashboard_sql(version, "stats.sql"));
        let list_sql = send_list.then(|| self.render_dashboard_sql(version, list.file()));
        let mut keys = (None, None);
        let mut error = None;
        if let Some(sql) = stats_sql {
            match sql {
                Ok(sql) => {
                    self.next_key += 1;
                    keys.0 = Some((self.next_key, sql));
                }
                Err(e) => error = Some(e),
            }
        }
        if let Some(sql) = list_sql {
            match sql {
                Ok(sql) => {
                    self.next_key += 1;
                    keys.1 = Some((self.next_key, sql));
                }
                Err(e) => error = Some(e),
            }
        }
        let Some(d) = self.tabs[i].dashboard.as_mut() else {
            return;
        };
        let Some(conn) = d.conn.as_ref() else {
            return;
        };
        if let Some((key, sql)) = keys.0 {
            conn.handle.lookup_with(key, sql, vec![filter.clone()]);
            d.stats_key = Some(key);
        }
        if let Some((key, sql)) = keys.1 {
            let params = if list.takes_database() {
                vec![filter]
            } else {
                Vec::new()
            };
            conn.handle.lookup_with(key, sql, params);
            d.list_key = Some((key, list));
        }
        if let Some(e) = error {
            tracing::error!("{e}");
            d.error = Some(e);
        }
    }

    fn dashboard_loaded(&mut self, tab: u64, key: u64, result: Result<Vec<Vec<String>>, DbError>) {
        let Some(d) = self.dashboard_of(tab) else {
            return;
        };
        if d.stats_key == Some(key) {
            d.stats_key = None;
            match result {
                Ok(rows) => {
                    if let Some(s) = rows
                        .first()
                        .and_then(|r| crate::dashboard::Sample::from_row(r))
                    {
                        d.apply_stats(s);
                    }
                }
                Err(e) => d.error = Some(e.to_string()),
            }
        } else if let Some((k, list)) = d.list_key
            && k == key
        {
            d.list_key = None;
            match result {
                Ok(rows) => {
                    d.error = None;
                    d.apply_rows(list, rows);
                }
                Err(e) => d.error = Some(e.to_string()),
            }
            // the user switched lists meanwhile: read the shown one at once
            if d.list != list {
                self.dashboard_refresh(tab, false);
                return;
            }
        } else if let Some((k, signal, session)) = d.signal_key.clone()
            && k == key
        {
            d.signal_key = None;
            self.signal_done(signal, &session, result);
            self.dashboard_refresh(tab, false);
            return;
        } else {
            return;
        }
        self.schedule_dashboard(tab);
    }

    /// The next refresh after the interval, once both answers are in (no pile-up).
    fn schedule_dashboard(&mut self, tab: u64) {
        let Some(d) = self.dashboard_of(tab) else {
            return;
        };
        if d.paused || !d.idle() || !d.connected() {
            return;
        }
        d.timer_seq += 1;
        let effect = Effect::StartDashboardTimer {
            tab,
            seq: d.timer_seq,
            after: Duration::from_secs(d.interval_secs()),
        };
        self.effects.push(effect);
    }

    /// `Msg::DashboardTimer`: time for the next refresh.
    pub(super) fn dashboard_timer(&mut self, tab: u64, seq: u64) {
        let Some(d) = self.dashboard_of(tab) else {
            return;
        };
        if d.timer_seq == seq && !d.paused {
            self.dashboard_refresh(tab, true);
        }
    }

    fn active_tab_id(&self) -> u64 {
        self.tabs[self.active_tab].id
    }

    /// Keys of the dashboard tab. Returns false if the command is not for it.
    pub(super) fn dashboard_command(&mut self, cmd: Command) -> bool {
        let tab = self.active_tab_id();
        let page = self.grid_size.0.saturating_sub(1).max(1) as isize;
        if self.dashboard().is_none() {
            return false;
        }
        // cursor moves in the shown list
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
            if let Some(d) = self.dashboard_mut() {
                d.grid_mut().move_cursor(dr, dc, 0);
            }
            self.dashboard_scroll();
            return true;
        }
        match cmd {
            Command::DashboardRefresh => {
                if self.dashboard().is_some_and(|d| d.conn.is_none()) {
                    self.dashboard_connect(tab);
                    self.status = "Connecting…".to_owned();
                } else {
                    self.dashboard_refresh(tab, true);
                }
            }
            Command::DashboardPause => {
                let paused = self.dashboard_mut().is_some_and(|d| {
                    d.paused = !d.paused;
                    d.paused
                });
                if paused {
                    self.status = "Dashboard paused".to_owned();
                } else {
                    self.status = "Dashboard refresh resumed".to_owned();
                    self.dashboard_refresh(tab, true);
                    self.schedule_dashboard(tab);
                }
            }
            Command::DashboardInterval => {
                if let Some(secs) = self.dashboard_mut().map(Dashboard::next_interval) {
                    self.status = format!("Dashboard refreshes every {secs} s");
                }
                self.schedule_dashboard(tab);
            }
            Command::HideIdleSessions => {
                let hide = self.dashboard_mut().is_some_and(|d| {
                    d.hide_idle = !d.hide_idle;
                    d.rebuild(List::Sessions);
                    d.hide_idle
                });
                self.status = if hide {
                    "Idle sessions hidden".to_owned()
                } else {
                    "Idle sessions shown".to_owned()
                };
                self.dashboard_scroll();
            }
            Command::NextList | Command::PrevList => {
                let n = List::ALL.len();
                let current = self.dashboard().map_or(List::Sessions, |d| d.list);
                let i = List::ALL.iter().position(|l| *l == current).unwrap_or(0);
                let next = if cmd == Command::NextList {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                };
                self.show_list(List::ALL[next]);
            }
            Command::CancelBackend => self.ask_signal(Signal::Cancel),
            Command::TerminateBackend => self.ask_signal(Signal::Terminate),
            Command::ShowRow | Command::Accept => self.show_dashboard_row(),
            Command::ContextMenu => {
                let a = self.grid_area;
                self.open_dashboard_menu((a.x + 2, a.y + 1));
            }
            Command::Copy => {
                let text = self.dashboard().and_then(|d| d.grid().copy_text());
                if let Some(text) = text
                    && self.clipboard.copy(&text)
                {
                    self.effects.push(Effect::Osc52(text));
                }
            }
            Command::SelectAll => {
                if let Some(d) = self.dashboard_mut() {
                    d.grid_mut().all_selected = true;
                }
            }
            _ => return false,
        }
        true
    }
    /// Keeps the dashboard grid's cursor visible.
    fn dashboard_scroll(&mut self) {
        let (h, w) = self.grid_size;
        if let Some(d) = self.dashboard_mut() {
            d.grid_mut().ensure_visible(h.saturating_sub(1), w);
        }
    }

    /// Shows `list` and reads it at once.
    fn show_list(&mut self, list: List) {
        let tab = self.active_tab_id();
        if let Some(d) = self.dashboard_mut() {
            d.list = list;
            d.error = None;
        }
        self.dashboard_refresh(tab, false);
    }

    fn show_dashboard_row(&mut self) {
        let Some(d) = self.dashboard() else {
            return;
        };
        let Some(lines) = d.row_lines() else {
            return;
        };
        let title = d.list.title().trim_end_matches('s').to_owned();
        let lines: Vec<String> = lines
            .iter()
            .flat_map(|l| crate::help::wrap(l, 100))
            .collect();
        let headers = vec![false; lines.len()];
        let mut dialog = crate::help::viewer(
            &title,
            crate::help::Lines {
                lines,
                headers,
                sections: Vec::new(),
            },
        );
        dialog.fit_text(self.screen);
        self.dialogs.push(dialog);
    }

    /// Cancel query / terminate session: asks first (D-057).
    fn ask_signal(&mut self, signal: Signal) {
        let tab = self.active_tab_id();
        let Some(d) = self.dashboard_mut() else {
            return;
        };
        if !d.connected() {
            self.status = "The dashboard is not connected".to_owned();
            return;
        }
        let Some(s) = d.selected_session() else {
            self.status = "Select a session in the Sessions list first".to_owned();
            return;
        };
        d.asking = Some((signal, s.clone()));
        let who = session_text(&s);
        let (kind, title, text) = match signal {
            Signal::Cancel => (
                DialogKind::ConfirmCancelBackend,
                "Cancel query",
                format!("Cancel the running query of {who}?"),
            ),
            Signal::Terminate => (
                DialogKind::ConfirmTerminateBackend,
                "Terminate session",
                format!("Terminate {who}?\nIts open transaction is rolled back."),
            ),
        };
        self.dialogs
            .push(Dialog::confirm(kind, title, &text).with_tag(tab));
    }

    /// Yes on the cancel / terminate question: sends the signal.
    pub(super) fn send_signal(&mut self, tab: u64, signal: Signal) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(version) = self.tabs[i].dashboard.as_ref().and_then(|d| d.version) else {
            return;
        };
        let sql = match self.render_dashboard_sql(version, signal.file()) {
            Ok(sql) => sql,
            Err(e) => {
                self.error(&e);
                return;
            }
        };
        self.next_key += 1;
        let key = self.next_key;
        let Some(d) = self.tabs[i].dashboard.as_mut() else {
            return;
        };
        let Some((asked, session)) = d.asking.take().filter(|(s, _)| *s == signal) else {
            return;
        };
        let Some(conn) = d.conn.as_ref().filter(|c| c.connected) else {
            self.status = "The dashboard is not connected".to_owned();
            return;
        };
        conn.handle
            .lookup_with(key, sql, vec![Some(session.pid.clone())]);
        d.signal_key = Some((key, asked, session));
    }

    fn signal_done(
        &mut self,
        signal: Signal,
        s: &Session,
        result: Result<Vec<Vec<String>>, DbError>,
    ) {
        let who = session_text(s);
        match result {
            Ok(rows)
                if rows
                    .first()
                    .and_then(|r| r.first())
                    .is_some_and(|v| v == "true") =>
            {
                self.status = match signal {
                    Signal::Cancel => format!("Cancel request sent to {who}"),
                    Signal::Terminate => format!("{who} terminated"),
                };
            }
            Ok(_) => {
                self.status = format!("{who}: no such session any more");
            }
            Err(e) => {
                let what = match signal {
                    Signal::Cancel => "Cannot cancel the query of",
                    Signal::Terminate => "Cannot terminate",
                };
                self.error(&format!("{what} {who}:\n{e}"));
            }
        }
    }

    /// Context menu of the dashboard tab (D-057).
    pub(super) fn open_dashboard_menu(&mut self, anchor: (u16, u16)) {
        let entries = vec![
            menu::item("&Refresh now", Command::DashboardRefresh),
            menu::item("&Pause / resume", Command::DashboardPause),
            menu::item("Refresh &interval", Command::DashboardInterval),
            MenuEntry::Separator,
            menu::item("&Next list", Command::NextList),
            menu::item("Hide / show i&dle sessions", Command::HideIdleSessions),
            menu::item("&Show row…", Command::ShowRow),
            MenuEntry::Separator,
            menu::item("&Cancel query…", Command::CancelBackend),
            menu::item("&Terminate session…", Command::TerminateBackend),
        ];
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }

    /// Whether a dashboard command can run (menus, the context menu).
    pub(super) fn dashboard_enabled(&self, cmd: Command) -> bool {
        let Some(d) = self.dashboard() else {
            return false;
        };
        match cmd {
            Command::CancelBackend | Command::TerminateBackend => {
                d.connected() && d.selected_session().is_some()
            }
            Command::ShowRow => d.loaded(d.list) && !d.grid().rows.is_empty(),
            _ => true,
        }
    }

    /// Mouse in the dashboard tab. Returns true if the event was used.
    pub(super) fn dashboard_mouse(
        &mut self,
        kind: crossterm::event::MouseEventKind,
        p: Position,
        hits: &Hits,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        if self.dashboard().is_none() {
            return false;
        }
        let dh = &hits.dashboard;
        match kind {
            MouseEventKind::Down(MouseButton::Left)
                if dh.lists.iter().any(|(_, r)| r.contains(p)) =>
            {
                if let Some((list, _)) = dh.lists.iter().find(|(_, r)| r.contains(p)) {
                    self.focus = Focus::Tabs;
                    self.show_list(*list);
                }
                true
            }
            MouseEventKind::Down(b @ (MouseButton::Left | MouseButton::Right))
                if dh.grid.contains(p) =>
            {
                self.focus = Focus::Tabs;
                let cell = dh
                    .cells
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(rc, _)| *rc);
                if let Some(d) = self.dashboard_mut()
                    && let Some((row, col)) = cell
                {
                    let g = d.grid_mut();
                    g.row = row;
                    g.col = col;
                    g.all_selected = false;
                }
                if b == MouseButton::Right {
                    self.open_dashboard_menu((p.x, p.y + 1));
                }
                true
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if dh.grid.contains(p) => {
                let delta = if kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if let Some(d) = self.dashboard_mut() {
                    d.grid_mut().move_cursor(delta, 0, 0);
                }
                self.dashboard_scroll();
                true
            }
            _ => false,
        }
    }

    /// `server · user@database` and the refresh state of a dashboard tab (D-043).
    pub(crate) fn dashboard_status(&self, tab: &Tab) -> Option<(String, String)> {
        let d = tab.dashboard.as_ref()?;
        let place = self.place_label(d.uid, Some(&d.conn_database));
        let left = match (&d.conn, &d.problem) {
            (Some(c), _) if c.connected => place,
            (Some(_), _) => format!("{place} – connecting…"),
            (None, _) => format!("{place} – not connected"),
        };
        let right = if !d.connected() {
            String::new()
        } else if d.paused {
            "Paused (Ctrl+P resumes)".to_owned()
        } else {
            format!("Refresh every {} s", d.interval_secs())
        };
        Some((left, right))
    }
}

/// `PID 1234 (user on database)`
fn session_text(s: &Session) -> String {
    match (s.user.is_empty(), s.database.is_empty()) {
        (false, false) => format!("PID {} ({} on {})", s.pid, s.user, s.database),
        (false, true) => format!("PID {} ({})", s.pid, s.user),
        _ => format!("PID {}", s.pid),
    }
}
