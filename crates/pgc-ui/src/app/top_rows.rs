//! Select top 100 rows (`Ctrl+B` in the tree, D-056).

use pgc_core::quote::quote_qualified;

use super::App;
use crate::tree::{ConnState, RowKind};

/// Rows the generated query asks for.
const TOP_ROWS: usize = 100;

/// The query of Select top rows for `schema.name`.
pub(super) fn top_rows_sql(schema: &str, name: &str) -> String {
    format!(
        "SELECT * FROM {} LIMIT {TOP_ROWS};\n",
        quote_qualified(&[schema, name])
    )
}

impl App {
    /// Server uid, database and query for the relation of the selected tree row: a
    /// table, view, materialized view, foreign table or partition, or any row below
    /// it. `Err` says why not.
    pub(super) fn top_rows_source(&self) -> Result<(u64, String, String), &'static str> {
        const NO: &str = "Select a table or view (or an object of it)";
        let Some(RowKind::Node(nid)) = self.tree.selected().map(|r| r.kind) else {
            return Err(NO);
        };
        let (server, database, schema, name) = self.tree.relation_of(nid).ok_or(NO)?;
        let server = &self.tree.servers[server];
        if server.state != ConnState::Connected {
            return Err("Connect the server first");
        }
        Ok((server.uid, database, top_rows_sql(&schema, &name)))
    }

    /// Ctrl+B: the first rows of the relation in a new query tab on the same server and
    /// database, executed as soon as the tab's connection is open (D-056).
    pub(super) fn select_top_rows(&mut self) {
        let (uid, database, sql) = match self.top_rows_source() {
            Ok(s) => s,
            Err(why) => {
                self.status = why.to_owned();
                return;
            }
        };
        self.new_query_tab(Some((uid, database)));
        self.focus = super::Focus::Tabs;
        let tab = &mut self.tabs[self.active_tab];
        tab.view.editor.set_text(&sql);
        tab.view.editor.set_modified(true);
        let id = tab.id;
        let Some(q) = tab.query_mut() else {
            return;
        };
        if q.conn.is_none() {
            return; // the tab says why it is not connected
        }
        if q.connected() {
            self.execute_tab(id);
        } else {
            q.run_on_connect = true;
        }
    }
}
