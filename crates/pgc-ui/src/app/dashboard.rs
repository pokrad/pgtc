//! The server dashboard tab (D-057): opening it, its own connection, the refresh
//! cycle, its keys, cancel query / terminate session.

use std::time::Duration;

use pgc_db::{ConnId, DbError, DbEvent};
use pgc_sql::RenderContext;
use ratatui::layout::Position;

use super::connections::MonitorEvent;
use super::{App, Effect, Focus, TabBody, Target, TextView};
use crate::dashboard::{Dashboard, List, Session, Signal, Signals, interval_text};
use crate::dialog::{Dialog, DialogKind};
use crate::keymap::Command;
use crate::menu::{self, MenuEntry, Popup};
use crate::tree::{ConnState, RowKind};
use crate::view::Hits;

const SQL_FOLDER: &str = "dashboard";

impl App {
    /// The dashboard of the active tab.
    pub(super) fn dashboard(&self) -> Option<&Dashboard> {
        self.tabs[self.active_tab].dashboard()
    }

    fn dashboard_mut(&mut self) -> Option<&mut Dashboard> {
        self.tabs[self.active_tab].dashboard_mut()
    }

    fn dashboard_of(&mut self, tab: u64) -> Option<&mut Dashboard> {
        self.tab_mut(tab).and_then(|t| t.dashboard_mut())
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
            t.dashboard()
                .is_some_and(|d| d.uid == uid && d.database == db)
        }) {
            self.active_tab = i;
            // the list first (D-059)
            self.focus = Focus::Results;
            return;
        }
        let label = match &db {
            Some(db) => format!("Dashboard: {} · {db}", server.conn.name),
            None => format!("Dashboard: {}", server.conn.name),
        };
        let conn_db = db.clone().unwrap_or_else(|| server.conn.database.clone());
        let view = TextView::new(true, self.tab_width());
        let dashboard = Dashboard::new(uid, db.clone(), conn_db, label);
        let id = self.add_tab(TabBody::Dashboard(Box::new(dashboard)), view);
        if let Some(t) = self.tab_mut(id) {
            t.server = Some(uid);
            t.database = db;
        }
        self.focus = Focus::Results;
        self.dashboard_connect(id);
    }

    /// Opens the dashboard's own connection (D-011, D-057).
    fn dashboard_connect(&mut self, tab: u64) {
        if self.monitor_connect(tab, Target::Dashboard(tab))
            && let Some(d) = self.dashboard_of(tab)
        {
            d.error = None;
            d.stats_key = None;
            d.size_key = None;
            d.list_key = None;
            d.signals = Default::default();
        }
    }

    pub(super) fn on_dashboard_event(&mut self, conn: ConnId, tab: u64, event: DbEvent) {
        match self.monitor_event(conn, tab, &event) {
            MonitorEvent::Connected => self.dashboard_refresh(tab, true),
            MonitorEvent::Other => {
                if let DbEvent::Lookup { key, result, .. } = event {
                    self.dashboard_loaded(tab, key, result);
                }
            }
            MonitorEvent::Gone | MonitorEvent::Failed | MonitorEvent::Closed => {}
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
        let Some(d) = self.tabs[i].dashboard() else {
            return;
        };
        let (Some(version), true) = (d.version, d.connected()) else {
            return;
        };
        let list = d.list;
        let filter = d.database.clone();
        let send_stats = stats && d.stats_key.is_none();
        let send_size = stats && d.size_key.is_none() && d.size_due();
        let send_list = d.list_key.is_none();
        let stats_sql = send_stats.then(|| self.render_dashboard_sql(version, "stats.sql"));
        let size_sql = send_size.then(|| self.render_dashboard_sql(version, "size.sql"));
        let list_sql = send_list.then(|| self.render_dashboard_sql(version, list.file()));
        let mut keys = (None, None);
        let mut size_key = None;
        let mut error = None;
        if let Some(sql) = size_sql {
            match sql {
                Ok(sql) => {
                    self.next_key += 1;
                    size_key = Some((self.next_key, sql));
                }
                Err(e) => error = Some(e),
            }
        }
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
        let Some(d) = self.tabs[i].dashboard_mut() else {
            return;
        };
        let Some(conn) = d.conn.as_ref() else {
            return;
        };
        if let Some((key, sql)) = keys.0 {
            conn.handle.lookup_with(key, sql, vec![filter.clone()]);
            d.stats_key = Some(key);
        }
        if let Some((key, sql)) = size_key {
            conn.handle.lookup_with(key, sql, vec![filter.clone()]);
            d.size_key = Some(key);
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
        } else if d.size_key == Some(key) {
            d.size_key = None;
            match result {
                Ok(rows) => {
                    let parsed = rows.first().and_then(|r| {
                        let bytes = r.first()?.trim().parse::<u64>().ok()?;
                        Some((bytes, r.get(1)?.trim().parse::<f64>().ok()?))
                    });
                    if let Some((bytes, t)) = parsed {
                        d.apply_size(bytes, t);
                    }
                }
                Err(e) => {
                    // not worth a broken tab: the strip shows "-"
                    tracing::warn!("dashboard size: {e}");
                    d.size_forced = false;
                }
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
        } else if let Some(i) = d.signals.pending.iter().position(|(k, _)| *k == key) {
            let (_, session) = d.signals.pending.remove(i);
            let answer = match result {
                Ok(rows) => Ok(rows
                    .first()
                    .and_then(|r| r.first())
                    .is_some_and(|v| v == "true")),
                Err(e) => Err(e.to_string()),
            };
            d.signals.done.push((session, answer));
            if d.signals.pending.is_empty() {
                let signals = std::mem::take(&mut d.signals);
                if let Some(signal) = signals.signal {
                    self.signals_done(signal, signals.done);
                }
                self.dashboard_refresh(tab, false);
            }
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
        let effect = Effect::Timer(super::Timer::Tab {
            tab,
            seq: d.timer_seq,
            after: Duration::from_millis(d.interval_ms()),
        });
        self.effects.push(effect);
    }

    /// `Msg::TabTimer`: time for the next refresh.
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
        if self.dashboard().is_none() {
            return false;
        }
        // the list: cursor, copy, select all
        if self.monitor_command(cmd) {
            return true;
        }
        match cmd {
            Command::DashboardRefresh => {
                if self.dashboard().is_some_and(|d| d.conn.is_none()) {
                    self.dashboard_connect(tab);
                    self.status = "Connecting…".to_owned();
                } else {
                    if let Some(d) = self.dashboard_mut() {
                        d.size_forced = true;
                    }
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
                if let Some(ms) = self.dashboard_mut().map(Dashboard::next_interval) {
                    self.status = format!("Dashboard refreshes every {}", interval_text(ms));
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
                self.monitor_scroll();
            }
            Command::DurationMode => {
                if let Some(mode) = self.dashboard_mut().map(Dashboard::next_mode) {
                    self.status = format!("Session duration: {} time", mode.name());
                }
                self.monitor_scroll();
            }
            Command::TagSession => {
                if self.dashboard_mut().is_some_and(Dashboard::toggle_tag) {
                    self.monitor_scroll();
                } else {
                    self.status = "Tag sessions in the Sessions list".to_owned();
                }
            }
            Command::ClearTags => {
                if let Some(d) = self.dashboard_mut()
                    && !d.tags.is_empty()
                {
                    d.tags.clear();
                    self.status = "Tags cleared".to_owned();
                }
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
                let a = self.frame.grid_area;
                self.open_dashboard_menu((a.x + 2, a.y + 1));
            }
            _ => return false,
        }
        true
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
        let mut dialog = crate::help::viewer(&title, crate::help::Lines { lines, headers });
        dialog.fit_text(self.frame.screen);
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
        let targets = d.signal_targets();
        let Some(first) = targets.first() else {
            self.status = "Select a session in the Sessions list first".to_owned();
            return;
        };
        let one = targets.len() == 1;
        let who = if one {
            session_text(first)
        } else {
            format!("{} tagged sessions", targets.len())
        };
        let (kind, title, text) = match signal {
            Signal::Cancel if one => (
                DialogKind::ConfirmCancelBackend { tab },
                "Cancel query",
                format!("Cancel the running query of {who}?"),
            ),
            Signal::Cancel => (
                DialogKind::ConfirmCancelBackend { tab },
                "Cancel queries",
                format!("Cancel the running queries of {who}?"),
            ),
            Signal::Terminate if one => (
                DialogKind::ConfirmTerminateBackend { tab },
                "Terminate session",
                format!("Terminate {who}?\nIts open transaction is rolled back."),
            ),
            Signal::Terminate => (
                DialogKind::ConfirmTerminateBackend { tab },
                "Terminate sessions",
                format!("Terminate {who}?\nTheir open transactions are rolled back."),
            ),
        };
        d.asking = Some((signal, targets));
        self.dialogs.push(Dialog::confirm(kind, title, &text));
    }

    /// Yes on the cancel / terminate question: sends the signal.
    pub(super) fn send_signal(&mut self, tab: u64, signal: Signal) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(version) = self.tabs[i].dashboard().and_then(|d| d.version) else {
            return;
        };
        let sql = match self.render_dashboard_sql(version, signal.file()) {
            Ok(sql) => sql,
            Err(e) => {
                self.error(&e);
                return;
            }
        };
        let first_key = self.next_key + 1;
        let Some(d) = self.tabs[i].dashboard_mut() else {
            return;
        };
        let Some((asked, sessions)) = d.asking.take().filter(|(s, _)| *s == signal) else {
            return;
        };
        let Some(conn) = d.conn.as_ref().filter(|c| c.connected) else {
            self.status = "The dashboard is not connected".to_owned();
            return;
        };
        let mut pending = Vec::with_capacity(sessions.len());
        for (k, session) in sessions.into_iter().enumerate() {
            let key = first_key + k as u64;
            conn.handle.lookup_with(
                key,
                sql.clone(),
                vec![Some(session.pid.clone()), session.started.clone()],
            );
            pending.push((key, session));
        }
        if pending.len() > 1 {
            // the signals went out: the tags have done their job
            d.tags.clear();
        }
        let sent = pending.len() as u64;
        d.signals = Signals {
            signal: Some(asked),
            pending,
            done: Vec::new(),
        };
        self.next_key += sent;
    }

    /// All answers of one question are in: the status line, the errors in a dialog.
    fn signals_done(&mut self, signal: Signal, done: Vec<(Session, Result<bool, String>)>) {
        let what = match signal {
            Signal::Cancel => "Cannot cancel the query of",
            Signal::Terminate => "Cannot terminate",
        };
        if let [(s, Ok(sent))] = done.as_slice() {
            let who = session_text(s);
            self.status = match (sent, signal) {
                (true, Signal::Cancel) => format!("Cancel request sent to {who}"),
                (true, Signal::Terminate) => format!("{who} terminated"),
                (false, _) => format!("{who}: no such session any more"),
            };
            return;
        }
        if done.len() > 1 {
            let sent = done.iter().filter(|(_, r)| *r == Ok(true)).count();
            self.status = match signal {
                Signal::Cancel => {
                    format!("Cancel request sent to {sent} of {} sessions", done.len())
                }
                Signal::Terminate => format!("{sent} of {} sessions terminated", done.len()),
            };
        }
        let errors: Vec<String> = done
            .iter()
            .filter_map(|(s, r)| {
                r.as_ref()
                    .err()
                    .map(|e| format!("{what} {}:\n{e}", session_text(s)))
            })
            .collect();
        if !errors.is_empty() {
            self.error(&errors.join("\n\n"));
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
            menu::item("Duration &mode", Command::DurationMode),
            menu::item("T&ag / untag session", Command::TagSession),
            menu::item("&Untag all", Command::ClearTags),
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
                d.connected() && !d.signal_targets().is_empty()
            }
            Command::TagSession => d.list == List::Sessions && d.selected_session().is_some(),
            Command::ClearTags => !d.tags.is_empty(),
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
                    self.focus = Focus::Results;
                    self.show_list(*list);
                }
                true
            }
            _ => match self.monitor_mouse(kind, p, dh.grid, &dh.cells) {
                Some(menu) => {
                    if menu {
                        self.open_dashboard_menu((p.x, p.y + 1));
                    }
                    true
                }
                None => false,
            },
        }
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
