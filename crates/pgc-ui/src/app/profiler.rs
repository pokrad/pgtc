//! The SQL profiler tab (D-058): opening it, its own connection, Start (check, ask,
//! change the logging settings), reading the server log, Stop / close / quit putting
//! the settings back, filters and keys.

use std::time::Duration;

use pgc_db::{ConnId, DbError, DbEvent};
use pgc_sql::RenderContext;
use ratatui::layout::Position;

use super::connections::MonitorEvent;
use super::{App, Effect, Focus, Tab, TabBody, Target, TextView};
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::keymap::Command;
use crate::menu::{self, MenuEntry, Popup};
use crate::profiler::{AfterRestore, Check, LogReader, Phase, Profiler, READ_CHUNK};
use crate::view::Hits;

const SQL_FOLDER: &str = "profiler";
/// Pause between log reads.
const READ_EVERY: Duration = Duration::from_secs(1);
/// Reads without a log file of the format before giving up (after the reload the
/// logger may need a moment to open it).
const MISSING_READS: u32 = 10;
const FILTER_LABEL_WIDTH: u16 = 14;

/// Hex text (`encode(…, 'hex')`) to bytes; stops at the first invalid pair.
fn from_hex(text: &str) -> Vec<u8> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    text.as_bytes()
        .chunks_exact(2)
        .map_while(|p| Some((digit(p[0])? << 4) | digit(p[1])?))
        .collect()
}

impl App {
    pub(super) fn profiler(&self) -> Option<&Profiler> {
        self.tabs[self.active_tab].profiler()
    }

    fn profiler_mut(&mut self) -> Option<&mut Profiler> {
        self.tabs[self.active_tab].profiler_mut()
    }

    fn profiler_of(&mut self, tab: u64) -> Option<&mut Profiler> {
        self.tab_mut(tab).and_then(|t| t.profiler_mut())
    }

    /// Tools → SQL profiler: one tab per server; from a database row its database is
    /// the filter of a new tab (D-058).
    pub(super) fn open_profiler(&mut self) {
        let Some((si, db)) = self.dashboard_target() else {
            self.status = "Select a connected server or database first".to_owned();
            return;
        };
        let server = &self.tree.servers[si];
        let uid = server.uid;
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.profiler().is_some_and(|p| p.uid == uid))
        {
            self.active_tab = i;
            // the list first (D-059)
            self.focus = Focus::Results;
            return;
        }
        let label = format!("Profiler: {}", server.conn.name);
        let conn_db = server.conn.database.clone();
        let view = TextView::new(true, self.tab_width());
        let profiler = Profiler::new(uid, conn_db, label, db);
        let id = self.add_tab(TabBody::Profiler(Box::new(profiler)), view);
        if let Some(t) = self.tab_mut(id) {
            t.server = Some(uid);
        }
        self.focus = Focus::Results;
        self.profiler_connect(id);
    }

    fn profiler_connect(&mut self, tab: u64) {
        if self.monitor_connect(tab, Target::Profiler(tab))
            && let Some(p) = self.profiler_of(tab)
        {
            p.error = None;
            p.read_key = None;
            p.step_keys.clear();
            p.phase = Phase::Stopped;
        }
    }

    pub(super) fn on_profiler_event(&mut self, conn: ConnId, tab: u64, event: DbEvent) {
        match self.monitor_event(conn, tab, &event) {
            MonitorEvent::Closed => {
                let Some(p) = self.profiler_of(tab) else {
                    return;
                };
                p.read_key = None;
                p.step_keys.clear();
                let quitting = p.after_restore == AfterRestore::Quit;
                p.phase = Phase::Stopped;
                let lost: Vec<String> = p.applied.drain(..).map(|c| c.restore_sql()).collect();
                if !lost.is_empty() {
                    self.restore_failed(&lost, "the connection was lost");
                }
                if quitting {
                    self.maybe_quit_after_restore();
                }
            }
            MonitorEvent::Other => {
                if let DbEvent::Lookup { key, result, .. } = event {
                    self.profiler_loaded(tab, key, result);
                }
            }
            MonitorEvent::Gone | MonitorEvent::Connected | MonitorEvent::Failed => {}
        }
    }

    fn render_profiler_sql(
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

    /// `F5`: Start when stopped, Stop when running (D-058).
    fn profiler_start_stop(&mut self, tab: u64) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        if p.conn.is_none() {
            self.profiler_connect(tab);
            self.status = "Connecting…".to_owned();
            return;
        }
        if !p.connected() {
            self.status = "Still connecting…".to_owned();
            return;
        }
        match p.phase {
            Phase::Stopped => self.profiler_check(tab),
            Phase::Running => self.profiler_stop(tab, AfterRestore::Stop),
            _ => self.status = "Wait until the profiler is ready".to_owned(),
        }
    }

    /// Start: reads the logging settings (`check.sql`).
    fn profiler_check(&mut self, tab: u64) {
        let Some(version) = self.profiler_of(tab).and_then(|p| p.version) else {
            return;
        };
        let sql = match self.render_profiler_sql(version, "check.sql") {
            Ok(sql) => sql,
            Err(e) => {
                self.error(&e);
                return;
            }
        };
        self.next_key += 1;
        let key = self.next_key;
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        if let Some(c) = p.conn.as_ref() {
            c.handle.lookup(key, sql);
            p.step_keys = vec![key];
            p.step_error = None;
            p.phase = Phase::Checking;
            p.error = None;
        }
    }

    fn profiler_checked(&mut self, tab: u64, result: Result<Vec<Vec<String>>, DbError>) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        p.phase = Phase::Stopped;
        let check = match result {
            Ok(rows) => rows.first().and_then(|r| Check::from_row(r)),
            Err(e) => {
                self.error(&format!("Cannot read the logging settings:\n{e}"));
                return;
            }
        };
        let (Some(check), Some(version)) = (check, p.version) else {
            return;
        };
        if !check.logging_collector {
            self.error(
                "logging_collector is off on this server.\n\n\
                 The profiler reads the server's log files, which need\n\
                 logging_collector = on. That change needs a server restart,\n\
                 so pgtc does not make it. As a superuser:\n\n  \
                 ALTER SYSTEM SET logging_collector = on;\n\n\
                 then restart the server and start the profiler again.",
            );
            return;
        }
        let (format, changes) = check.plan(version);
        p.own_pid = Some(check.own_pid);
        p.reader = Some(LogReader::new(format));
        if changes.is_empty() {
            p.planned.clear();
            self.profiler_running(tab);
            return;
        }
        let list: String = changes
            .iter()
            .map(|c| format!("  {} = '{}'\n", c.name, c.value))
            .collect();
        p.planned = changes;
        p.phase = Phase::Asking;
        let text = format!(
            "To show every statement, pgtc changes these server settings\n\
             (ALTER SYSTEM and a configuration reload, no restart):\n\n{list}\n\
             They are put back on Stop, when the tab closes and when pgtc\n\
             quits. If pgtc ends without that (crash, killed), they stay\n\
             changed. Logging every statement adds load on a busy server.\n\n\
             Change them?"
        );
        self.dialogs.push(
            Dialog::confirm(DialogKind::ConfirmProfilerSettings, "SQL profiler", &text)
                .with_tag(tab),
        );
    }

    /// The question about the settings was answered.
    pub(super) fn profiler_answer(&mut self, tab: u64, yes: bool) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        if p.phase != Phase::Asking {
            return;
        }
        if !yes {
            p.phase = Phase::Stopped;
            p.planned.clear();
            self.status = "Profiler not started".to_owned();
            return;
        }
        let statements: Vec<String> = p.planned.iter().map(|c| c.set_sql()).collect();
        p.phase = Phase::Applying;
        self.profiler_step(tab, statements);
    }

    /// Sends `statements` (each on its own: ALTER SYSTEM cannot run in a transaction
    /// block) and the reload; the answers end the step.
    fn profiler_step(&mut self, tab: u64, statements: Vec<String>) {
        let reload = self
            .profiler_of(tab)
            .and_then(|p| p.version)
            .map(|v| self.render_profiler_sql(v, "reload.sql"));
        let mut keys = Vec::new();
        let mut sqls = statements;
        match reload {
            Some(Ok(sql)) => sqls.push(sql),
            Some(Err(e)) => tracing::error!("{e}"),
            None => {}
        }
        for _ in &sqls {
            self.next_key += 1;
            keys.push(self.next_key);
        }
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        let Some(c) = p.conn.as_ref() else {
            return;
        };
        for (key, sql) in keys.iter().zip(sqls) {
            c.handle.lookup(*key, sql);
        }
        p.step_keys = keys;
        p.step_error = None;
    }

    /// The settings are in place: reading starts at the end of the current log file.
    fn profiler_running(&mut self, tab: u64) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        p.phase = Phase::Running;
        p.file = None;
        p.offset = 0;
        p.missing_reads = 0;
        p.read = 0;
        p.paused = false;
        if let Some(r) = p.reader.as_mut() {
            r.reset();
        }
        self.status = "Profiler started".to_owned();
        self.profiler_read(tab);
    }

    /// Stop (or closing / quitting): puts the settings back (D-058).
    pub(super) fn profiler_stop(&mut self, tab: u64, after: AfterRestore) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        p.after_restore = after;
        p.read_key = None;
        p.timer_seq += 1;
        if p.applied.is_empty() {
            p.phase = Phase::Stopped;
            self.status = "Profiler stopped".to_owned();
            return;
        }
        let statements: Vec<String> = p.applied.iter().map(|c| c.restore_sql()).collect();
        p.phase = Phase::Restoring;
        self.status = "Restoring logging settings…".to_owned();
        self.profiler_step(tab, statements);
    }

    fn profiler_step_done(&mut self, tab: u64) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        let error = p.step_error.take();
        match p.phase {
            Phase::Applying => {
                // whatever went through is put back by Stop
                p.applied = std::mem::take(&mut p.planned);
                if let Some(e) = error {
                    self.error(&format!(
                        "Cannot change the logging settings — this needs a superuser\n\
                         (or ALTER SYSTEM rights):\n{e}"
                    ));
                    self.profiler_stop(tab, AfterRestore::Stop);
                } else {
                    self.profiler_running(tab);
                }
            }
            Phase::Restoring => {
                let restore: Vec<String> = p.applied.drain(..).map(|c| c.restore_sql()).collect();
                p.phase = Phase::Stopped;
                let quitting = p.after_restore == AfterRestore::Quit;
                if let Some(e) = error {
                    self.restore_failed(&restore, &e);
                } else {
                    self.status = "Profiler stopped; logging settings restored".to_owned();
                }
                if quitting {
                    self.maybe_quit_after_restore();
                }
            }
            _ => {}
        }
    }

    fn restore_failed(&mut self, statements: &[String], why: &str) {
        let list: String = statements.iter().map(|s| format!("  {s};\n")).collect();
        tracing::error!("logging settings not restored: {why}");
        self.error(&format!(
            "The logging settings could not be put back ({why}).\n\
             Run as a superuser:\n\n{list}  SELECT pg_reload_conf();"
        ));
    }

    /// Reads the next part of the log (D-058).
    fn profiler_read(&mut self, tab: u64) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        if p.phase != Phase::Running || p.paused || p.read_key.is_some() || !p.connected() {
            return;
        }
        let (Some(version), Some(format)) = (p.version, p.reader.as_ref().map(|r| r.format)) else {
            return;
        };
        let params = vec![
            Some(format.name().to_owned()),
            p.file.clone(),
            Some(p.offset.to_string()),
            Some(READ_CHUNK.to_string()),
        ];
        let sql = match self.render_profiler_sql(version, "read.sql") {
            Ok(sql) => sql,
            Err(e) => {
                self.error(&e);
                return;
            }
        };
        self.next_key += 1;
        let key = self.next_key;
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        if let Some(c) = p.conn.as_ref() {
            c.handle.lookup_with(key, sql, params);
            p.read_key = Some(key);
        }
    }

    fn profiler_read_done(&mut self, tab: u64, result: Result<Vec<Vec<String>>, DbError>) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        p.read_key = None;
        if p.phase != Phase::Running {
            return;
        }
        let row = match result {
            Ok(rows) => rows.into_iter().next().unwrap_or_default(),
            Err(e) => {
                p.error = Some(format!(
                    "Cannot read the server log — this needs a superuser or\n\
                     pg_read_server_files:\n{e}"
                ));
                self.schedule_profiler(tab, READ_EVERY);
                return;
            }
        };
        let col = |i: usize| row.get(i).cloned().unwrap_or_default();
        let (current, current_size, size, data) = (col(0), col(1), col(2), col(3));
        if current.is_empty() {
            p.missing_reads += 1;
            if p.missing_reads > MISSING_READS {
                let name = p.reader.as_ref().map_or("", |r| r.format.name());
                p.error = Some(format!(
                    "The server writes no {name} log (log_destination changed?)."
                ));
            }
            self.schedule_profiler(tab, READ_EVERY);
            return;
        }
        p.missing_reads = 0;
        p.error = None;
        let Some(file) = p.file.clone() else {
            // the first read: start at the end of the current file
            p.file = Some(current);
            p.offset = current_size.parse().unwrap_or(0);
            self.schedule_profiler(tab, READ_EVERY);
            return;
        };
        let bytes = from_hex(&data);
        p.offset += bytes.len() as u64;
        let records = p
            .reader
            .as_mut()
            .map(|r| r.feed(&bytes))
            .unwrap_or_default();
        p.add_records(records);
        if bytes.len() as u64 >= READ_CHUNK {
            // more is waiting
            self.profiler_read(tab);
        } else if size.is_empty() || current != file {
            // a new log file (rotation): after the rest of the old one
            p.file = Some(current);
            p.offset = 0;
            if let Some(r) = p.reader.as_mut() {
                r.reset();
            }
            self.profiler_read(tab);
        } else {
            self.schedule_profiler(tab, READ_EVERY);
        }
    }

    fn schedule_profiler(&mut self, tab: u64, after: Duration) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        p.timer_seq += 1;
        let effect = Effect::Timer(super::Timer::Tab {
            tab,
            seq: p.timer_seq,
            after,
        });
        self.effects.push(effect);
    }

    fn profiler_loaded(&mut self, tab: u64, key: u64, result: Result<Vec<Vec<String>>, DbError>) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        if p.read_key == Some(key) {
            self.profiler_read_done(tab, result);
            return;
        }
        let Some(i) = p.step_keys.iter().position(|k| *k == key) else {
            return;
        };
        p.step_keys.remove(i);
        if p.phase == Phase::Checking {
            p.step_keys.clear();
            self.profiler_checked(tab, result);
            return;
        }
        if let Err(e) = result {
            p.step_error.get_or_insert_with(|| e.to_string());
        }
        if p.step_keys.is_empty() {
            self.profiler_step_done(tab);
        }
    }

    /// `Msg::TabTimer` for dashboard and profiler tabs.
    pub(super) fn tab_timer(&mut self, tab: u64, seq: u64) {
        if self.profiler_of(tab).is_some_and(|p| p.timer_seq == seq) {
            self.profiler_read(tab);
        } else {
            self.dashboard_timer(tab, seq);
        }
    }

    /// Keys of the profiler tab. Returns false if the command is not for it.
    pub(super) fn profiler_command(&mut self, cmd: Command) -> bool {
        let tab = self.tabs[self.active_tab].id;
        let page = self.grid_size.0.saturating_sub(1).max(1) as isize;
        if self.profiler().is_none() {
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
            if let Some(p) = self.profiler_mut() {
                p.grid.move_cursor(dr, dc, 0);
            }
            self.profiler_scroll();
            return true;
        }
        match cmd {
            Command::ProfilerStartStop => self.profiler_start_stop(tab),
            Command::ProfilerPause => {
                let Some(p) = self.profiler_mut() else {
                    return true;
                };
                if p.phase != Phase::Running {
                    self.status = "The profiler is not running".to_owned();
                    return true;
                }
                p.paused = !p.paused;
                if p.paused {
                    p.timer_seq += 1;
                    self.status =
                        "Profiler paused (reading stops; it goes on from there)".to_owned();
                } else {
                    self.status = "Profiler resumed".to_owned();
                    self.profiler_read(tab);
                }
            }
            Command::ProfilerFilter => self.open_profiler_filter(tab),
            Command::ProfilerClear => {
                if let Some(p) = self.profiler_mut() {
                    p.clear();
                }
                self.status = "Profiler entries cleared".to_owned();
            }
            Command::ShowRow | Command::Accept => self.show_profiler_row(),
            Command::ContextMenu => {
                let a = self.grid_area;
                self.open_profiler_menu((a.x + 2, a.y + 1));
            }
            Command::Copy => {
                let text = self.profiler().and_then(|p| p.grid.copy_text());
                if let Some(text) = text
                    && self.clipboard.copy(&text)
                {
                    self.effects.push(Effect::Osc52(text));
                }
            }
            Command::SelectAll => {
                if let Some(p) = self.profiler_mut() {
                    p.grid.all_selected = true;
                }
            }
            _ => return false,
        }
        true
    }

    fn profiler_scroll(&mut self) {
        let (h, w) = self.grid_size;
        if let Some(p) = self.profiler_mut() {
            p.grid.ensure_visible(h.saturating_sub(1), w);
        }
    }

    fn show_profiler_row(&mut self) {
        let Some(lines) = self.profiler().and_then(|p| p.grid.row_lines()) else {
            return;
        };
        let lines: Vec<String> = lines
            .iter()
            .flat_map(|l| crate::help::wrap(l, 100))
            .collect();
        let headers = vec![false; lines.len()];
        let mut dialog = crate::help::viewer(
            "Statement",
            crate::help::Lines {
                lines,
                headers,
                sections: Vec::new(),
            },
        );
        dialog.fit_text(self.screen);
        self.dialogs.push(dialog);
    }

    fn open_profiler_filter(&mut self, tab: u64) {
        let Some(p) = self.profiler_of(tab) else {
            return;
        };
        let f = p.filter.clone();
        let lw = FILTER_LABEL_WIDTH;
        self.dialogs.push(
            Dialog::new(
                DialogKind::ProfilerFilter,
                "Filter",
                56,
                vec![
                    Widget::field("user", "&User:", &f.user, lw),
                    Widget::field("database", "&Database:", &f.database, lw),
                    Widget::field("pid", "&PID:", &f.pid, lw),
                    Widget::field("application", "&Application:", &f.application, lw),
                    Widget::field("text", "&Text contains:", &f.text, lw),
                    Widget::label("Empty fields match everything."),
                    Widget::button("ok", "&OK", true),
                    Widget::button("cancel", "&Cancel", false),
                ],
            )
            .with_tag(tab),
        );
    }

    pub(super) fn apply_profiler_filter(&mut self, dialog: &Dialog) {
        let text = |id: &str| dialog.text(id).unwrap_or("").trim().to_owned();
        let filter = crate::profiler::Filter {
            user: text("user"),
            database: text("database"),
            pid: text("pid"),
            application: text("application"),
            text: text("text"),
        };
        if let Some(p) = self.profiler_of(dialog.tag) {
            p.filter = filter;
            p.rebuild();
        }
    }

    pub(super) fn open_profiler_menu(&mut self, anchor: (u16, u16)) {
        let entries = vec![
            menu::item("&Start / stop", Command::ProfilerStartStop),
            menu::item("&Pause / resume", Command::ProfilerPause),
            MenuEntry::Separator,
            menu::item("&Filter…", Command::ProfilerFilter),
            menu::item("&Clear", Command::ProfilerClear),
            menu::item("Show &entry…", Command::ShowRow),
        ];
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }

    pub(super) fn profiler_enabled(&self, cmd: Command) -> bool {
        let Some(p) = self.profiler() else {
            return false;
        };
        match cmd {
            Command::ProfilerPause => p.phase == Phase::Running,
            Command::ShowRow => !p.grid.rows.is_empty(),
            _ => true,
        }
    }

    pub(super) fn profiler_mouse(
        &mut self,
        kind: crossterm::event::MouseEventKind,
        p: Position,
        hits: &Hits,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        if self.profiler().is_none() {
            return false;
        }
        let ph = &hits.profiler;
        match kind {
            MouseEventKind::Down(b @ (MouseButton::Left | MouseButton::Right))
                if ph.grid.contains(p) =>
            {
                self.focus = Focus::Results;
                let cell = ph
                    .cells
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(rc, _)| *rc);
                if let Some(prof) = self.profiler_mut()
                    && let Some((row, col)) = cell
                {
                    prof.grid.row = row;
                    prof.grid.col = col;
                    prof.grid.all_selected = false;
                }
                if b == MouseButton::Right {
                    self.open_profiler_menu((p.x, p.y + 1));
                }
                true
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if ph.grid.contains(p) => {
                let delta = if kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if let Some(prof) = self.profiler_mut() {
                    prof.grid.move_cursor(delta, 0, 0);
                }
                self.profiler_scroll();
                true
            }
            _ => false,
        }
    }

    /// `server · user@database` and the profiler state (D-043, D-058).
    pub(crate) fn profiler_status(&self, tab: &Tab) -> Option<(String, String)> {
        let p = tab.profiler()?;
        let place = self.place_label(p.uid, Some(&p.conn_database));
        let left = match &p.conn {
            Some(c) if c.connected => place,
            Some(_) => format!("{place} – connecting…"),
            None => format!("{place} – not connected"),
        };
        Some((left, p.state_text()))
    }

    /// Closing a profiler tab: changed settings are put back on its connection, which
    /// then closes (the answers are not waited for).
    pub(super) fn close_profiler_conn(&mut self, p: Profiler) {
        let Some(conn) = p.conn else {
            return;
        };
        self.connections.remove(&conn.id);
        if conn.connected && !p.applied.is_empty() {
            for c in &p.applied {
                conn.handle.lookup(0, c.restore_sql());
            }
            if let Some(Ok(sql)) = p.version.map(|v| self.render_profiler_sql(v, "reload.sql")) {
                conn.handle.lookup(0, sql);
            }
            conn.handle.close();
        } else {
            conn.handle.abort();
        }
    }

    /// Quitting: profilers with changed settings put them back first. True if quitting
    /// has to wait for them (D-058).
    pub(super) fn restore_before_quit(&mut self) -> bool {
        if self.quit_after_restore {
            // F10 again while waiting: quit at once
            return false;
        }
        let tabs: Vec<u64> = self
            .tabs
            .iter()
            .filter(|t| {
                t.profiler()
                    .is_some_and(|p| p.needs_restore() && p.connected())
            })
            .map(|t| t.id)
            .collect();
        if tabs.is_empty() {
            return false;
        }
        for tab in tabs {
            self.profiler_stop(tab, AfterRestore::Quit);
        }
        self.quit_after_restore = true;
        self.status = "Restoring logging settings… (F10 again quits at once)".to_owned();
        true
    }

    fn maybe_quit_after_restore(&mut self) {
        if !self.quit_after_restore {
            return;
        }
        let waiting = self
            .tabs
            .iter()
            .any(|t| t.profiler().is_some_and(|p| p.phase == Phase::Restoring));
        if !waiting {
            self.quit = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::from_hex;

    #[test]
    fn hex_decoding() {
        assert_eq!(from_hex("41620a"), b"Ab\n");
        assert_eq!(from_hex(""), b"");
        assert_eq!(from_hex("41zz42"), b"A");
    }
}
