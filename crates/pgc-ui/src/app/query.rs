//! Query tabs (D-011, D-019, D-020, D-041): their connections, running statements,
//! transactions, cancel, and the tab's connection label (D-043).

use pgc_db::{ConnId, ConnectParams, DbEvent, ExecuteRequest, TxStatus};

use super::result_export::take_export_rows;
use super::{App, Tab, TabKind, Target, TextView};
use crate::dialog::{Dialog, DialogKind};
use crate::export::ToWriter;
use crate::query_tab::{AfterRun, LineMap, QueryConn, QueryConnState, Sent};
use crate::tree::{ConnState, RowKind};

impl App {
    /// Server and database for a new query tab: the database of the selected tree row
    /// (the database itself or an object below it) on a connected server (D-049).
    pub(super) fn query_target(&self) -> Option<(usize, String)> {
        let row = self.tree.selected()?;
        if self.tree.servers[row.server].state != ConnState::Connected {
            return None;
        }
        let db = match row.kind {
            RowKind::Node(nid) | RowKind::Loading(nid) => self.tree.database_name_of(nid),
            RowKind::Server => None,
        }?;
        Some((row.server, db))
    }

    /// A new, empty query tab bound to `binding` (server uid, database), or to the
    /// tree selection's database.
    pub(super) fn new_query_tab(&mut self, binding: Option<(u64, String)>) {
        self.next_query += 1;
        let n = self.next_query;
        let view = TextView::new(false, self.tab_width());
        let id = self.add_tab(TabKind::Query(n), view);
        if let Some(q) = self.query_mut() {
            q.binding = binding;
        }
        self.bind_query_tab(id);
    }

    /// Opens the query tab's own connection (D-011, D-041).
    pub(super) fn bind_query_tab(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        // reconnect to the same database if the tab had one
        let previous = self.tabs[i]
            .query
            .as_ref()
            .and_then(|q| q.binding.clone())
            .and_then(|(uid, db)| self.tree.index_of(uid).map(|s| (s, db)))
            .filter(|(s, _)| self.tree.servers[*s].state == ConnState::Connected);
        let Some((si, database)) = previous.or_else(|| self.query_target()) else {
            if let Some(q) = self.tabs[i].query.as_mut() {
                q.info("Not connected — select a database in the tree, then press F5.");
            }
            return;
        };
        let timeout = self.timeout();
        let server = &self.tree.servers[si];
        let password = server
            .password
            .clone()
            .or_else(|| Self::pgpass_password(&server.conn, &database));
        let params = ConnectParams::new(&server.conn, &database, password, timeout);
        let (uid, ssl_mode, root) = (
            server.uid,
            server.conn.ssl_mode,
            server.conn.ssl_root_cert.clone(),
        );
        self.next_conn += 1;
        let id = self.next_conn;
        let handle = pgc_db::spawn(id, params, self.sink.clone());
        self.connections.insert(id, Target::Query(tab));
        if let Some(q) = self.tabs[i].query.as_mut() {
            q.binding = Some((uid, database.clone()));
            q.conn = Some(QueryConn {
                id,
                database: database.clone(),
                handle,
                cancel: None,
                state: QueryConnState::Connecting,
                ssl_mode,
                ssl_root_cert: root,
            });
            q.info(format!("Connecting to {database}…"));
        }
    }

    pub(super) fn on_query_event(&mut self, conn: ConnId, tab: u64, event: DbEvent) {
        let Some(i) = self.tab_index(tab) else {
            self.connections.remove(&conn);
            return;
        };
        let Tab { view, query, .. } = &mut self.tabs[i];
        let Some(q) = query.as_mut() else {
            return;
        };
        match event {
            DbEvent::Connected {
                cancel, version, ..
            } => {
                if let Some(c) = q.conn.as_mut() {
                    c.state = QueryConnState::Connected;
                    c.cancel = Some(cancel);
                    let db = c.database.clone();
                    q.info(format!("Connected to {db} (PostgreSQL {version})"));
                }
                self.run_queued(tab);
            }
            DbEvent::ConnectFailed { error, .. } => {
                self.connections.remove(&conn);
                q.conn = None;
                q.info(format!("Cannot connect: {error}"));
                q.show_messages = true;
            }
            DbEvent::Query { run, event, .. } => {
                if run != q.last_run {
                    return;
                }
                let Some(event) = take_export_rows(q, event) else {
                    return;
                };
                let editor = &view.editor;
                let line_of = |c: usize| editor.line_of_char(c);
                let line_start = |l: usize| editor.line_to_char(l.min(editor.len_lines() - 1));
                let lines = LineMap {
                    line_of: &line_of,
                    line_start: &line_start,
                };
                let finished = q.apply(event, &lines);
                if q.export.is_some() {
                    self.pump_export(tab);
                }
                if finished {
                    self.query_finished(tab);
                }
            }
            DbEvent::Notice { notice, .. } => q.notice(&notice),
            DbEvent::Closed { error, .. } => {
                self.connections.remove(&conn);
                q.conn = None;
                q.running = None;
                q.tx = TxStatus::Idle;
                q.info(match error {
                    Some(e) => format!("Connection lost: {e}"),
                    None => "Connection closed".to_owned(),
                });
                self.drop_tab_lookups(tab);
            }
            DbEvent::Lookup { key, result, .. } => self.lookup_loaded(key, result),
            DbEvent::Nodes { .. } | DbEvent::Definition { .. } | DbEvent::Model { .. } => {}
        }
    }

    fn query_finished(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(q) = self.tabs[i].query.as_mut() else {
            return;
        };
        let after = q.after_run.take();
        // the window status line replaces "Executing…" with how the run ended (D-043)
        if i == self.active_tab {
            self.status = match q.finished {
                Some(f) if f.failed => format!("{} — see Messages", f.text()),
                Some(f) => f.text(),
                None if q.show_messages => "Query failed — see Messages".to_owned(),
                None => String::new(),
            };
        }
        match after {
            Some(AfterRun::CloseTab) if q.tx == TxStatus::Idle => self.close_tab(tab),
            Some(AfterRun::Bulk) => self.maybe_finish_bulk(),
            _ => {}
        }
    }

    /// F5: the selection, or the whole text, as one request (D-019, D-020).
    pub(super) fn execute(&mut self) {
        let Some(q) = self.query() else {
            return;
        };
        if q.conn.is_none() {
            let id = self.tabs[self.active_tab].id;
            self.bind_query_tab(id);
            if self.query().is_some_and(|q| q.conn.is_none()) {
                self.status = "Select a database in the tree first".to_owned();
                return;
            }
        }
        let q = self.query().expect("query tab");
        if q.busy() {
            self.status = "A query is already running".to_owned();
            return;
        }
        if q.export.is_some() {
            self.status = "An export is running".to_owned();
            return;
        }
        if !q.connected() {
            self.status = "Still connecting…".to_owned();
            return;
        }
        let editor = &self.tabs[self.active_tab].view.editor;
        let (text, offset) = match editor.selection() {
            Some((a, _)) => (editor.selected_text().unwrap_or_default(), a),
            None => (editor.text(), 0),
        };
        if text.trim().is_empty() {
            return;
        }
        let (sql, plpgsql_lines) = if self.query().is_some_and(|q| q.plpgsql) {
            match crate::query_tab::wrap_plpgsql(&text) {
                Ok((sql, n)) => (sql, Some(n)),
                Err(e) => {
                    self.error(&e);
                    return;
                }
            }
        } else {
            (text, None)
        };
        self.send_query(
            sql,
            Sent {
                offset,
                plpgsql_lines,
            },
            true,
        );
    }

    /// The tab's connection is open: runs what waited for it (D-056).
    pub(super) fn run_queued(&mut self, tab: u64) {
        let queued = self
            .tab_index(tab)
            .and_then(|i| self.tabs[i].query.as_mut())
            .is_some_and(|q| std::mem::take(&mut q.run_on_connect));
        if queued {
            self.execute_tab(tab);
        }
    }

    /// F5 in tab `tab`, also when it is not the active one.
    pub(super) fn execute_tab(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let previous = self.active_tab;
        let status = self.status.clone();
        self.active_tab = i;
        self.execute();
        self.active_tab = previous.min(self.tabs.len() - 1);
        if previous != i {
            self.status = status;
        }
    }

    /// Sends `sql` for the active query tab.
    fn send_query(&mut self, sql: String, sent: Sent, use_modes: bool) {
        self.next_run += 1;
        let run = self.next_run;
        let batch_rows = self.settings.query.batch_rows.max(1) as usize;
        let Some(q) = self.query_mut() else {
            return;
        };
        let Some(conn) = q.conn.as_ref() else {
            return;
        };
        let request = ExecuteRequest {
            run,
            sql,
            batch_rows,
            auto_begin: use_modes && !q.auto_commit,
            auto_rollback: use_modes && q.auto_rollback,
        };
        conn.handle.execute(request);
        q.start_run(run, sent);
        self.status = "Executing…".to_owned();
    }

    /// COMMIT / ROLLBACK in the active query tab (D-019).
    pub(super) fn send_simple(&mut self, sql: &str, after: Option<AfterRun>) {
        let Some(q) = self.query() else {
            return;
        };
        if !q.connected() || q.busy() {
            return;
        }
        self.send_query(
            sql.to_owned(),
            Sent {
                offset: 0,
                plpgsql_lines: None,
            },
            false,
        );
        if let Some(q) = self.query_mut() {
            q.after_run = after;
        }
    }

    pub(super) fn finish_transaction(&mut self, tab: u64, after: AfterRun) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let previous = self.active_tab;
        self.active_tab = i;
        self.send_simple("COMMIT", Some(after));
        self.active_tab = previous.min(self.tabs.len() - 1);
    }

    /// Alt+Shift+C: PostgreSQL's cancel request (D-019).
    pub(super) fn cancel_query(&mut self) {
        let Some(q) = self.query_mut() else {
            return;
        };
        if let Some(e) = q.export.as_mut() {
            // D-042: stop the export; the writer deletes the file
            e.abort_reason
                .get_or_insert_with(|| "Export cancelled".to_owned());
            let _ = e.writer.send(ToWriter::Abort);
            self.status = "Cancelling export…".to_owned();
            return;
        }
        let (Some(running), Some(conn)) = (q.running.as_mut(), q.conn.as_ref()) else {
            return;
        };
        if running.paused {
            // a paused result: end it, the remaining rows are dropped (D-019)
            conn.handle.discard(q.last_run);
            self.status = "Result closed".to_owned();
            return;
        }
        if let Some(cancel) = &conn.cancel {
            pgc_db::cancel_query(cancel, conn.ssl_mode, conn.ssl_root_cert.clone());
            running
                .cancel_requested
                .get_or_insert_with(std::time::Instant::now);
            self.status = "Cancel requested…".to_owned();
        }
    }

    /// After `cancel_wait_secs` without reaction: offer to close the connection (D-041).
    pub(super) fn check_cancel_wait(&mut self) {
        let wait =
            std::time::Duration::from_secs(u64::from(self.settings.query.cancel_wait_secs.max(1)));
        let mut ask = Vec::new();
        for tab in &mut self.tabs {
            if let Some(r) = tab.query.as_mut().and_then(|q| q.running.as_mut())
                && !r.asked
                && r.cancel_requested.is_some_and(|t| t.elapsed() >= wait)
            {
                r.asked = true;
                ask.push(tab.id);
            }
        }
        for id in ask {
            self.dialogs.push(
                Dialog::confirm_buttons(
                    DialogKind::CancelWait,
                    "Cancel",
                    "The server did not react to the cancel request.\nClose this query tab's connection?",
                    &[("close", "&Close connection"), ("wait", "&Keep waiting")],
                )
                .with_tag(id),
            );
        }
    }

    pub(super) fn abort_query_conn(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        if let Some(q) = self.tabs[i].query.as_mut()
            && let Some(conn) = q.conn.take()
        {
            conn.handle.abort();
            self.connections.remove(&conn.id);
            q.running = None;
            q.tx = TxStatus::Idle;
            q.info("Connection closed (the server did not react to the cancel request)");
        }
        self.drop_tab_lookups(tab);
    }

    /// Where a query tab is connected: `server · user@database` or its state (D-043).
    pub fn tab_connection_label(&self, tab: &Tab) -> String {
        let Some(q) = &tab.query else {
            // object and default tabs: where the object comes from, then read only
            return match tab.server {
                Some(uid) => format!(
                    "{} · read only",
                    self.place_label(uid, tab.database.as_deref())
                ),
                None => "read only".to_owned(),
            };
        };
        let Some((uid, db)) = &q.binding else {
            return "No database – select one in the tree".to_owned();
        };
        let place = self.place_label(*uid, Some(db));
        match q.conn.as_ref().map(|c| c.state) {
            Some(QueryConnState::Connected) => place,
            Some(QueryConnState::Connecting) => format!("{place} – connecting…"),
            None => format!("{place} – not connected"),
        }
    }

    /// `server · user@database` (or `server · user` without a database).
    fn place_label(&self, uid: u64, db: Option<&str>) -> String {
        let (server, user) = self
            .tree
            .servers
            .iter()
            .find(|s| s.uid == uid)
            .map(|s| (s.conn.name.as_str(), s.conn.username.as_str()))
            .unwrap_or(("?", ""));
        match (user.is_empty(), db) {
            (true, Some(db)) => format!("{server} · {db}"),
            (false, Some(db)) => format!("{server} · {user}@{db}"),
            (true, None) => server.to_owned(),
            (false, None) => format!("{server} · {user}"),
        }
    }

    /// Server version of the active query tab's server.
    pub(super) fn tab_server_version(&self) -> Option<pgc_core::ServerVersion> {
        let (uid, _) = self.query()?.binding.as_ref()?;
        self.tree.servers.iter().find(|s| s.uid == *uid)?.version
    }
}
