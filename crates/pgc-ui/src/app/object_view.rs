//! The default tab and object tabs (D-009, D-037): info texts and object definitions.

use pgc_db::DbError;
use pgc_sql::RenderContext;

use super::{App, Effect, TabKind, TextView};
use crate::tree::{ConnState, RowKind};

impl App {
    /// Node id of the selected row if it is an object (not a group).
    fn selected_object(&self) -> Option<u64> {
        match self.tree.selected()?.kind {
            RowKind::Node(nid) if self.tree.node(nid).is_some_and(|n| !n.is_group()) => Some(nid),
            _ => None,
        }
    }

    /// Keeps the default tab on the selected tree row: info text at once, object
    /// definitions after the cursor has rested (200 ms).
    pub(super) fn follow_selection(&mut self) -> Vec<Effect> {
        let (server, database) = self.selected_place();
        self.tabs[0].server = server;
        self.tabs[0].database = database;
        let selected = self.tree.selected();
        let kind = selected.map(|r| r.kind);
        let changed = kind != self.followed;
        self.followed = kind;
        match (selected, self.selected_object()) {
            (_, Some(nid)) if changed => {
                let title = self
                    .tree
                    .node(nid)
                    .map(|n| n.label.clone())
                    .unwrap_or_default();
                let view = &mut self.tabs[0].view;
                if view.title != title {
                    view.set_text(title, String::new());
                }
                view.loading = true;
                self.def_seq += 1;
                vec![Effect::StartDefinitionTimer(self.def_seq)]
            }
            (_, Some(_)) => Vec::new(),
            (Some(row), None) => {
                // Info text is cheap: rebuild it on every update (counts, states).
                if changed {
                    self.def_seq += 1;
                }
                let (title, text) = self.info_text(row);
                self.tabs[0].view.set_text(title, text);
                Vec::new()
            }
            (None, None) if changed => {
                self.tabs[0].view.set_text(String::new(), String::new());
                Vec::new()
            }
            (None, None) => Vec::new(),
        }
    }

    /// Info text for rows that are not objects (D-037), as SQL comments.
    fn info_text(&self, row: crate::tree::Row) -> (String, String) {
        match row.kind {
            RowKind::Server => {
                let s = &self.tree.servers[row.server];
                let state = match s.state {
                    ConnState::Connected => "connected",
                    ConnState::Connecting => "connecting",
                    ConnState::Disconnected => "disconnected",
                };
                let mut text = format!(
                    "-- Server: {}\n--\n--   Host:            {}:{}\n--   Maintenance DB:  {}\n\
                     --   User name:       {}\n--   SSL mode:        {}\n--   State:           {state}\n",
                    s.conn.name,
                    s.conn.host,
                    s.conn.port,
                    s.conn.database,
                    s.conn.username,
                    s.conn.ssl_mode.name()
                );
                if let Some(v) = s.version {
                    text.push_str(&format!("--   Version:         PostgreSQL {v}\n"));
                }
                (s.conn.name.clone(), text)
            }
            RowKind::Node(nid) => {
                let Some(n) = self.tree.node(nid) else {
                    return (String::new(), String::new());
                };
                let text = match &n.children {
                    Some(children) => {
                        let mut t = format!("-- {} ({})\n", n.label, children.len());
                        if !children.is_empty() {
                            t.push_str("--\n");
                        }
                        for c in children {
                            t.push_str(&format!("--   {}\n", c.label));
                        }
                        t
                    }
                    None if n.loading => format!("-- {}: loading…\n", n.label),
                    None => format!("-- {}: not loaded yet (expand to load)\n", n.label),
                };
                (n.label.clone(), text)
            }
            RowKind::Loading(_) => ("".to_owned(), "-- loading…\n".to_owned()),
        }
    }

    /// Requests the definition of object `nid` for tab `tab`.
    pub(super) fn request_definition(&mut self, tab: u64, nid: u64) {
        let Some(ctx) = self.tree.definition_context(nid) else {
            return;
        };
        let server = &self.tree.servers[ctx.server];
        let handle = match ctx.database {
            Some(db) => self
                .tree
                .node(db)
                .and_then(|n| n.conn.as_ref())
                .filter(|c| c.state == ConnState::Connected)
                .and_then(|c| c.handle.clone()),
            None => server.handle.clone(),
        };
        let (Some(version), Some(handle)) = (server.version, handle) else {
            return;
        };
        let render = RenderContext {
            version,
            show_system: self.settings.ui.show_system_objects,
        };
        let sql = match self
            .sql
            .render(ctx.obj.definition_folder(), "definition.sql", render)
        {
            Ok(sql) => sql,
            Err(e) => {
                tracing::error!("{e}");
                if let Some(t) = self.tab_mut(tab) {
                    t.view.load(format!("-- {e}\n"));
                }
                return;
            }
        };
        let mut params = vec![ctx.oid];
        if ctx.obj.definition_needs_parent() {
            let Some(parent) = ctx.parent_oid else {
                return;
            };
            params.push(parent);
        }
        self.next_key += 1;
        let key = self.next_key;
        self.pending_defs.insert(key, tab);
        if tab == 0 {
            self.default_key = key;
        }
        if let Some(t) = self.tab_mut(tab) {
            t.view.loading = true;
        }
        handle.load_definition(key, sql, params);
    }

    pub(super) fn definition_loaded(&mut self, key: u64, result: Result<String, DbError>) {
        let Some(tab) = self.pending_defs.remove(&key) else {
            return;
        };
        if tab == 0 && key != self.default_key {
            return; // the cursor has moved on
        }
        let text = match result {
            Ok(text) => text,
            Err(e) => e
                .to_string()
                .lines()
                .map(|l| format!("-- {l}\n"))
                .collect::<String>(),
        };
        if let Some(t) = self.tab_mut(tab) {
            t.view.load(text);
        }
    }

    /// F3: the selected row in a new object tab (D-009).
    pub(super) fn show_in_new_tab(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        let mut view = TextView::new(true, self.tab_width());
        match self.selected_object() {
            Some(nid) => {
                view.title = self
                    .tree
                    .node(nid)
                    .map(|n| n.label.clone())
                    .unwrap_or_default();
                view.loading = true;
                let id = self.add_tab(TabKind::Object, view);
                self.request_definition(id, nid);
            }
            None => {
                let (title, text) = self.info_text(row);
                view.set_text(title, text);
                self.add_tab(TabKind::Object, view);
            }
        }
        let (server, database) = self.selected_place();
        if let Some(t) = self.tabs.last_mut() {
            t.server = server;
            t.database = database;
        }
    }

    /// Server uid and database of the selected tree row (for the tab status).
    fn selected_place(&self) -> (Option<u64>, Option<String>) {
        let Some(row) = self.tree.selected() else {
            return (None, None);
        };
        let db = match row.kind {
            RowKind::Node(nid) | RowKind::Loading(nid) => self.tree.database_name_of(nid),
            RowKind::Server => None,
        };
        (Some(self.tree.servers[row.server].uid), db)
    }
}
