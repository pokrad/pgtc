//! The object tree pane (D-036): expanding, loading and refreshing groups, quick
//! search, the context menu.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgc_sql::RenderContext;

use super::App;
use crate::keymap::{Command, Context};
use crate::keys::KeyCombo;
use crate::menu::{self, MenuEntry, Popup};
use crate::tree::{ConnState, RowKind};

impl App {
    /// Quick search keys (D-036). Returns true if the key was used by the search.
    pub(super) fn search_key(
        &mut self,
        ev: &KeyEvent,
        combo: KeyCombo,
        cmd: Option<Command>,
    ) -> bool {
        if let KeyCode::Char(c) = ev.code
            && !ev
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            if let Some(text) = &mut self.search {
                text.push(c);
            }
            self.search_from(self.tree.cursor);
            return true;
        }
        if cmd == Some(Command::QuickSearch) {
            self.search_from(self.tree.cursor + 1);
            return true;
        }
        match self.keymap.lookup_only(Context::Dialog, combo) {
            Some(Command::Backspace) => {
                if let Some(text) = &mut self.search {
                    text.pop();
                }
                self.search_from(self.tree.cursor);
                true
            }
            Some(Command::Cancel | Command::Accept) => {
                self.end_search();
                true
            }
            _ => {
                self.end_search();
                false
            }
        }
    }

    /// Moves the cursor to the first row from `start` (wrapping) whose name starts with
    /// the search text.
    fn search_from(&mut self, start: usize) {
        let Some(text) = self.search.clone() else {
            return;
        };
        let needle = text.to_lowercase();
        let rows = self.tree.rows();
        let n = rows.len();
        let found = (0..n).map(|k| (start + k) % n).find(|&i| {
            self.tree
                .row_name(rows[i])
                .to_lowercase()
                .starts_with(&needle)
        });
        if let Some(i) = found {
            self.tree.cursor = i;
        }
        self.status = if found.is_some() || text.is_empty() {
            format!("Quick search: {text}")
        } else {
            format!("Quick search: {text}  (not found)")
        };
    }

    fn end_search(&mut self) {
        self.search = None;
        self.status.clear();
    }

    pub(super) fn expand(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        match row.kind {
            RowKind::Server => {
                let s = &mut self.tree.servers[row.server];
                match s.state {
                    ConnState::Disconnected => {
                        let uid = s.uid;
                        self.connect_server(uid);
                    }
                    ConnState::Connected => s.expanded = true,
                    ConnState::Connecting => {}
                }
            }
            RowKind::Node(nid) => self.expand_node(nid),
            RowKind::Loading(_) => {}
        }
    }

    pub(super) fn expand_node(&mut self, nid: u64) {
        let Some(node) = self.tree.node_mut(nid) else {
            return;
        };
        if !node.expandable() {
            return;
        }
        node.expanded = true;
        if node.is_database() {
            if node
                .conn
                .as_ref()
                .is_none_or(|c| c.state == ConnState::Disconnected)
            {
                self.connect_database(nid);
            }
        } else if node.is_group() && node.children.is_none() && !node.loading {
            self.load_group(nid);
        }
    }

    pub(super) fn collapse(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        let was_expanded = match row.kind {
            RowKind::Server => {
                let s = &mut self.tree.servers[row.server];
                std::mem::replace(&mut s.expanded, false) && s.state == ConnState::Connected
            }
            RowKind::Node(nid) => self
                .tree
                .node_mut(nid)
                .is_some_and(|n| std::mem::replace(&mut n.expanded, false)),
            RowKind::Loading(_) => false,
        };
        if !was_expanded {
            self.tree.select_parent();
        }
    }

    pub(super) fn toggle(&mut self) {
        let expanded = match self.tree.selected().map(|r| r.kind) {
            Some(RowKind::Server) => self
                .tree
                .selected_server()
                .is_some_and(|s| s.expanded && s.state == ConnState::Connected),
            Some(RowKind::Node(nid)) => self.tree.node(nid).is_some_and(|n| n.expanded),
            _ => return,
        };
        if expanded {
            self.collapse();
        } else {
            self.expand();
        }
    }

    /// Starts loading the children of group `nid` (D-036).
    pub(super) fn load_group(&mut self, nid: u64) {
        let Some(ctx) = self.tree.load_context(nid) else {
            return;
        };
        let server = &self.tree.servers[ctx.server];
        let Some(version) = server.version else {
            return;
        };
        let handle = match ctx.database {
            Some(db) => self
                .tree
                .node(db)
                .and_then(|n| n.conn.as_ref())
                .filter(|c| c.state == ConnState::Connected)
                .and_then(|c| c.handle.clone()),
            None => server.handle.clone(),
        };
        let Some(handle) = handle else {
            return;
        };
        let render = RenderContext {
            version,
            show_system: self.settings.ui.show_system_objects,
        };
        let sql = match self.sql.render(ctx.obj.folder(), "nodes.sql", render) {
            Ok(sql) => sql,
            Err(e) => {
                tracing::error!("{e}");
                self.error(&e.to_string());
                return;
            }
        };
        let parent = if ctx.obj.takes_parent() {
            ctx.parent_oid
        } else {
            None
        };
        self.next_key += 1;
        self.pending.insert(self.next_key, nid);
        if let Some(n) = self.tree.node_mut(nid) {
            n.loading = true;
        }
        handle.load_nodes(self.next_key, sql, parent);
    }

    /// `Ctrl+R`: reloads the loaded groups below the selected row (D-036).
    pub(super) fn refresh(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        if self.tree.servers[row.server].state != ConnState::Connected {
            return;
        }
        let groups = self.tree.loaded_groups(row);
        for g in groups {
            self.load_group(g);
        }
        // autocomplete reads the names again (D-052)
        for q in self.tabs.iter_mut().filter_map(|t| t.query.as_mut()) {
            q.names.clear();
        }
        self.status = "Refreshed".to_owned();
        self.followed = None;
    }

    /// `Alt+.`: shows / hides system objects everywhere and saves the setting (D-036).
    pub(super) fn toggle_system_objects(&mut self) {
        let show = !self.settings.ui.show_system_objects;
        self.settings.ui.show_system_objects = show;
        self.save_settings();
        for g in self.tree.all_loaded_groups() {
            self.load_group(g);
        }
        self.status = if show {
            "System objects shown"
        } else {
            "System objects hidden"
        }
        .to_owned();
    }

    pub(super) fn open_context_menu(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        // items that do not apply to the row kind are left out (D-049)
        let in_database = match row.kind {
            RowKind::Node(nid) | RowKind::Loading(nid) => self.tree.database_name_of(nid).is_some(),
            RowKind::Server => false,
        };
        let entries = match row.kind {
            RowKind::Server => vec![
                menu::item("&Connect", Command::Connect),
                menu::item("&Disconnect", Command::Disconnect),
                MenuEntry::Separator,
                menu::item("&Refresh", Command::RefreshTree),
                MenuEntry::Separator,
                menu::item("&Edit connection…", Command::EditConnection),
                menu::item("De&lete connection", Command::DeleteConnection),
            ],
            _ if in_database => {
                // "Show in new tab" is F3 only, not in context menus (D-049)
                let mut entries = vec![
                    menu::item("New &query", Command::NewQuery),
                    menu::item("Ed&it in new query tab", Command::EditInQueryTab),
                    menu::item("&Open SQL file…", Command::OpenFile),
                ];
                // tables, views and the rows below them (D-056)
                if self.top_rows_source().is_ok() {
                    entries.insert(
                        2,
                        menu::item("Select &top 100 rows", Command::SelectTopRows),
                    );
                }
                entries.extend([
                    MenuEntry::Separator,
                    menu::item("&Refresh", Command::RefreshTree),
                ]);
                // database definition export / load (D-048)
                if self.selected_database().is_some() {
                    entries.extend([
                        MenuEntry::Separator,
                        menu::item("&Export database definition…", Command::ExportDefinition),
                        menu::item("&Load database definition…", Command::LoadDefinition),
                    ]);
                }
                entries
            }
            // server-level groups and objects (Databases, roles, tablespaces, …)
            _ => vec![menu::item("&Refresh", Command::RefreshTree)],
        };
        let anchor = (
            2,
            (self.tree.cursor.saturating_sub(self.tree.offset) + 3) as u16,
        );
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }
}
