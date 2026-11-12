//! Object tree model (D-009, D-036): servers → groups → objects → groups → ...
//! Group nodes load their children lazily with a `nodes.sql` query.

use pgc_core::ServerVersion;
use pgc_core::settings::SavedConnection;
use pgc_db::{ConnectionHandle, NodeRow};

use crate::icons::Icon;
use crate::objects::{ObjType, SERVER_GROUPS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// "Tables", "Columns", ... — children are objects of this type.
    Group(ObjType),
    Object(ObjType),
}

/// Connection of a database node (D-011: one connection per expanded database).
#[derive(Debug)]
pub struct DbConn {
    pub state: ConnState,
    pub handle: Option<ConnectionHandle>,
}

/// Server index, database name (if any) and the objects from the top down (D-046).
/// (server, database, objects as (type, name, label)).
pub type ObjectPath = (usize, Option<String>, Vec<(ObjType, String, String)>);

/// A node on the way down from the server: (kind, oid, nid), see [`Tree::chain`].
pub type ChainNode = (NodeKind, u32, u64);

#[derive(Debug)]
pub struct Node {
    /// Unique id for the session.
    pub nid: u64,
    pub kind: NodeKind,
    pub oid: u32,
    pub name: String,
    pub label: String,
    pub partitioned: bool,
    pub expanded: bool,
    /// `None` = not loaded (groups) / not connected (databases).
    pub children: Option<Vec<Node>>,
    pub loading: bool,
    /// Database nodes only.
    pub conn: Option<DbConn>,
}

impl Node {
    pub fn is_group(&self) -> bool {
        matches!(self.kind, NodeKind::Group(_))
    }

    pub fn is_database(&self) -> bool {
        self.kind == NodeKind::Object(ObjType::Database)
    }

    /// Can be expanded (groups always; databases; objects with child groups).
    pub fn expandable(&self) -> bool {
        match self.kind {
            NodeKind::Group(_) => true,
            NodeKind::Object(ObjType::Database) => true,
            NodeKind::Object(_) => self.children.as_ref().is_some_and(|c| !c.is_empty()),
        }
    }

    fn walk<'a>(&'a self, out: &mut Vec<&'a Node>) {
        out.push(self);
        for c in self.children.iter().flatten() {
            c.walk(out);
        }
    }

    fn find_mut(&mut self, nid: u64) -> Option<&mut Node> {
        if self.nid == nid {
            return Some(self);
        }
        self.children
            .iter_mut()
            .flatten()
            .find_map(|c| c.find_mut(nid))
    }
}

#[derive(Debug)]
pub struct ServerNode {
    /// Runtime identity (indices change when servers are deleted).
    pub uid: u64,
    pub conn: SavedConnection,
    pub state: ConnState,
    pub expanded: bool,
    /// Groups (Databases, Roles, Tablespaces) while connected.
    pub children: Vec<Node>,
    pub version: Option<ServerVersion>,
    /// Password known for this session (dialog, login prompt, URI, PGPASSWORD).
    pub password: Option<String>,
    /// Write the password to `.pgpass` after the next successful connect (D-035).
    pub save_password: bool,
    /// `-w` on the command line: a missing password is an error, no prompt (D-031).
    pub never_prompt: bool,
    pub handle: Option<ConnectionHandle>,
}

impl ServerNode {
    fn new(uid: u64, conn: SavedConnection) -> Self {
        Self {
            uid,
            conn,
            state: ConnState::Disconnected,
            expanded: false,
            children: Vec::new(),
            version: None,
            password: None,
            save_password: false,
            never_prompt: false,
            handle: None,
        }
    }

    /// Back to the disconnected state; returns the handles that must be closed
    /// (server and all database connections).
    pub fn reset(&mut self) -> Vec<ConnectionHandle> {
        let mut handles: Vec<ConnectionHandle> = self.handle.take().into_iter().collect();
        fn collect(node: &mut Node, out: &mut Vec<ConnectionHandle>) {
            if let Some(conn) = &mut node.conn
                && let Some(h) = conn.handle.take()
            {
                out.push(h);
            }
            for c in node.children.iter_mut().flatten() {
                collect(c, out);
            }
        }
        for c in &mut self.children {
            collect(c, &mut handles);
        }
        self.children.clear();
        self.state = ConnState::Disconnected;
        self.expanded = false;
        self.version = None;
        handles
    }
}

/// What a visible row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Server,
    Node(u64),
    /// "loading…" placeholder under the group with this id.
    Loading(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub server: usize,
    pub kind: RowKind,
    pub depth: usize,
}

/// Text parts of a row.
pub struct RowText {
    pub depth: usize,
    /// `v` expanded, `>` expandable, ` ` leaf.
    pub marker: char,
    pub icon: Option<Icon>,
    pub label: String,
}

/// Where a group node sits: what is needed to load its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadContext {
    pub server: usize,
    pub obj: ObjType,
    /// Database node whose connection runs the query (None = server connection).
    pub database: Option<u64>,
    /// OID of the nearest object above the group (schema, table, ...).
    pub parent_oid: Option<u32>,
}

/// Where an object sits: what is needed to load its definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefContext {
    pub server: usize,
    pub obj: ObjType,
    pub oid: u32,
    /// Database node whose connection runs the query (None = server connection).
    pub database: Option<u64>,
    /// OID of the nearest object above (table for columns, ...).
    pub parent_oid: Option<u32>,
}

#[derive(Debug, Default)]
pub struct Tree {
    pub servers: Vec<ServerNode>,
    pub cursor: usize,
    pub offset: usize,
    next_uid: u64,
    next_nid: u64,
}

impl Tree {
    pub fn new(conns: &[SavedConnection]) -> Self {
        let mut tree = Self::default();
        for c in conns {
            tree.add(c.clone());
        }
        tree
    }

    pub fn add(&mut self, conn: SavedConnection) -> u64 {
        self.next_uid += 1;
        self.servers.push(ServerNode::new(self.next_uid, conn));
        self.next_uid
    }

    pub fn remove(&mut self, uid: u64) -> Option<ServerNode> {
        let i = self.index_of(uid)?;
        let node = self.servers.remove(i);
        self.clamp();
        Some(node)
    }

    pub fn saved(&self) -> Vec<SavedConnection> {
        self.servers.iter().map(|s| s.conn.clone()).collect()
    }

    pub fn index_of(&self, uid: u64) -> Option<usize> {
        self.servers.iter().position(|s| s.uid == uid)
    }

    pub fn by_uid(&mut self, uid: u64) -> Option<&mut ServerNode> {
        self.servers.iter_mut().find(|s| s.uid == uid)
    }

    fn nid(&mut self) -> u64 {
        self.next_nid += 1;
        self.next_nid
    }

    /// Group nodes for `types` (not loaded).
    pub fn groups(&mut self, types: &[ObjType]) -> Vec<Node> {
        types
            .iter()
            .map(|t| Node {
                nid: self.nid(),
                kind: NodeKind::Group(*t),
                oid: 0,
                name: t.group_label().to_owned(),
                label: t.group_label().to_owned(),
                partitioned: false,
                expanded: false,
                children: None,
                loading: false,
                conn: None,
            })
            .collect()
    }

    /// Server `uid` connected: create its groups, expand it.
    pub fn server_connected(&mut self, uid: u64) {
        let groups = self.groups(SERVER_GROUPS);
        if let Some(s) = self.by_uid(uid) {
            s.children = groups;
            s.expanded = true;
        }
    }

    pub fn node(&self, nid: u64) -> Option<&Node> {
        let mut all = Vec::new();
        for s in &self.servers {
            for c in &s.children {
                c.walk(&mut all);
            }
        }
        all.into_iter().find(|n| n.nid == nid)
    }

    pub fn node_mut(&mut self, nid: u64) -> Option<&mut Node> {
        self.servers
            .iter_mut()
            .flat_map(|s| s.children.iter_mut())
            .find_map(|c| c.find_mut(nid))
    }

    /// Server index and ancestor chain (outermost first, `nid` last) of a node.
    fn ancestry(&self, nid: u64) -> Option<(usize, Vec<&Node>)> {
        fn search<'a>(node: &'a Node, nid: u64, path: &mut Vec<&'a Node>) -> bool {
            path.push(node);
            if node.nid == nid {
                return true;
            }
            for c in node.children.iter().flatten() {
                if search(c, nid, path) {
                    return true;
                }
            }
            path.pop();
            false
        }
        for (i, s) in self.servers.iter().enumerate() {
            for c in &s.children {
                let mut path = Vec::new();
                if search(c, nid, &mut path) {
                    return Some((i, path));
                }
            }
        }
        None
    }

    /// Server index and the nodes from the top down to `nid` (itself last) as
    /// (kind, oid, nid) — for Find object (D-055).
    pub fn chain(&self, nid: u64) -> Option<(usize, Vec<ChainNode>)> {
        let (server, path) = self.ancestry(nid)?;
        Some((
            server,
            path.iter().map(|n| (n.kind, n.oid, n.nid)).collect(),
        ))
    }

    /// What is needed to load the children of group `nid`.
    pub fn load_context(&self, nid: u64) -> Option<LoadContext> {
        let (server, path) = self.ancestry(nid)?;
        let group = path.last()?;
        let NodeKind::Group(obj) = group.kind else {
            return None;
        };
        let database = path.iter().find(|n| n.is_database()).map(|n| n.nid);
        let parent_oid = path
            .iter()
            .rev()
            .skip(1)
            .find(|n| !n.is_group())
            .map(|n| n.oid);
        Some(LoadContext {
            server,
            obj,
            database,
            parent_oid,
        })
    }

    /// What is needed to load the definition of object `nid` (D-037): the database
    /// above it (not itself — a database's definition comes from the server
    /// connection) and the nearest object above it.
    pub fn definition_context(&self, nid: u64) -> Option<DefContext> {
        let (server, path) = self.ancestry(nid)?;
        let node = path.last()?;
        let NodeKind::Object(obj) = node.kind else {
            return None;
        };
        let above = &path[..path.len() - 1];
        Some(DefContext {
            server,
            obj,
            oid: node.oid,
            database: above.iter().find(|n| n.is_database()).map(|n| n.nid),
            parent_oid: above.iter().rev().find(|n| !n.is_group()).map(|n| n.oid),
        })
    }

    /// Objects on the path to `nid` (D-046 template defaults): server index, database
    /// name (if inside one), and every object node from the top down as
    /// (type, name). The node itself is last.
    pub fn object_path(&self, nid: u64) -> Option<ObjectPath> {
        let (server, path) = self.ancestry(nid)?;
        let database = path
            .iter()
            .find(|n| n.is_database())
            .map(|n| n.name.clone());
        let objects = path
            .iter()
            .filter_map(|n| match n.kind {
                NodeKind::Object(obj) => Some((obj, n.name.clone(), n.label.clone())),
                NodeKind::Group(_) => None,
            })
            .collect();
        Some((server, database, objects))
    }

    /// The table, view, materialized view, foreign table or partition that `nid` is or
    /// is below (D-056): server index, database, schema and name.
    pub fn relation_of(&self, nid: u64) -> Option<(usize, String, String, String)> {
        let (server, path) = self.ancestry(nid)?;
        let database = path.iter().find(|n| n.is_database())?.name.clone();
        let at = path.iter().rposition(|n| {
            matches!(
                n.kind,
                NodeKind::Object(
                    ObjType::Table
                        | ObjType::View
                        | ObjType::MatView
                        | ObjType::ForeignTable
                        | ObjType::Partition
                )
            )
        })?;
        let name = path[at].name.clone();
        // A partition in another schema than its parent table has the label
        // `schema.name`; otherwise it is in its parent's schema.
        let mut schema = None;
        for n in path[..=at].iter().rev() {
            match n.kind {
                NodeKind::Object(ObjType::Partition) => {
                    if let Some(s) = n
                        .label
                        .strip_suffix(n.name.as_str())
                        .and_then(|s| s.strip_suffix('.'))
                        .filter(|s| !s.is_empty())
                    {
                        schema = Some(s.to_owned());
                        break;
                    }
                }
                NodeKind::Object(ObjType::Schema | ObjType::Catalog) => {
                    schema = Some(n.name.clone());
                    break;
                }
                _ => {}
            }
        }
        Some((server, database, schema?, name))
    }

    /// Name of the database node `nid` is in (or is).
    pub fn database_name_of(&self, nid: u64) -> Option<String> {
        let (_, path) = self.ancestry(nid)?;
        path.iter()
            .find(|n| n.is_database())
            .map(|n| n.name.clone())
    }

    /// Server index of node `nid`.
    pub fn server_of(&self, nid: u64) -> Option<usize> {
        self.ancestry(nid).map(|(s, _)| s)
    }

    /// Replaces the children of group `nid` with `rows`, keeping existing nodes (and
    /// their expanded state and loaded children) whose OID is still there.
    pub fn set_children(&mut self, nid: u64, rows: Vec<NodeRow>) {
        let Some(NodeKind::Group(obj)) = self.node(nid).map(|n| n.kind) else {
            return;
        };
        let mut old: Vec<Node> = self
            .node_mut(nid)
            .and_then(|n| n.children.take())
            .unwrap_or_default();
        let mut children = Vec::with_capacity(rows.len());
        for row in rows {
            let label = row.label.clone().unwrap_or_else(|| row.name.clone());
            if let Some(i) = old.iter().position(|n| n.oid == row.oid) {
                let mut node = old.swap_remove(i);
                node.name = row.name;
                node.label = label;
                if node.partitioned != row.partitioned {
                    node.partitioned = row.partitioned;
                    let groups = self.groups(&obj.children(row.partitioned));
                    node.children = (!groups.is_empty()).then_some(groups);
                }
                children.push(node);
                continue;
            }
            let groups = self.groups(&obj.children(row.partitioned));
            children.push(Node {
                nid: self.nid(),
                kind: NodeKind::Object(obj),
                oid: row.oid,
                name: row.name,
                label,
                partitioned: row.partitioned,
                expanded: false,
                children: (obj != ObjType::Database && !groups.is_empty()).then_some(groups),
                loading: false,
                conn: (obj == ObjType::Database).then_some(DbConn {
                    state: ConnState::Disconnected,
                    handle: None,
                }),
            });
        }
        if let Some(n) = self.node_mut(nid) {
            n.children = Some(children);
            n.loading = false;
        }
    }

    /// Ids of all loaded groups in the subtree of `row` (for refresh, D-036).
    pub fn loaded_groups(&self, row: Row) -> Vec<u64> {
        let mut all = Vec::new();
        match row.kind {
            RowKind::Server => {
                for c in &self.servers[row.server].children {
                    c.walk(&mut all);
                }
            }
            RowKind::Node(nid) | RowKind::Loading(nid) => {
                if let Some(n) = self.node(nid) {
                    n.walk(&mut all);
                }
            }
        }
        all.into_iter()
            .filter(|n| n.is_group() && n.children.is_some())
            .map(|n| n.nid)
            .collect()
    }

    /// All loaded groups of all servers.
    pub fn all_loaded_groups(&self) -> Vec<u64> {
        (0..self.servers.len())
            .flat_map(|i| {
                self.loaded_groups(Row {
                    server: i,
                    kind: RowKind::Server,
                    depth: 0,
                })
            })
            .collect()
    }

    pub fn rows(&self) -> Vec<Row> {
        fn push(node: &Node, server: usize, depth: usize, rows: &mut Vec<Row>) {
            rows.push(Row {
                server,
                kind: RowKind::Node(node.nid),
                depth,
            });
            if !node.expanded {
                return;
            }
            match &node.children {
                Some(children) => {
                    for c in children {
                        push(c, server, depth + 1, rows);
                    }
                }
                None if node.loading || node.is_database() => rows.push(Row {
                    server,
                    kind: RowKind::Loading(node.nid),
                    depth: depth + 1,
                }),
                None => {}
            }
        }
        let mut rows = Vec::new();
        for (i, s) in self.servers.iter().enumerate() {
            rows.push(Row {
                server: i,
                kind: RowKind::Server,
                depth: 0,
            });
            if s.expanded && s.state == ConnState::Connected {
                for c in &s.children {
                    push(c, i, 1, &mut rows);
                }
            }
        }
        rows
    }

    pub fn selected(&self) -> Option<Row> {
        self.rows().get(self.cursor).copied()
    }

    /// Server of the selected row.
    pub fn selected_server(&self) -> Option<&ServerNode> {
        self.selected().map(|r| &self.servers[r.server])
    }

    pub fn select(&mut self, row: Row) {
        if let Some(i) = self.rows().iter().position(|r| *r == row) {
            self.cursor = i;
        }
    }

    pub fn select_kind(&mut self, kind: RowKind) {
        if let Some(i) = self.rows().iter().position(|r| r.kind == kind) {
            self.cursor = i;
        }
    }

    pub fn select_server(&mut self, uid: u64) {
        if let Some(i) = self.index_of(uid) {
            self.select(Row {
                server: i,
                kind: RowKind::Server,
                depth: 0,
            });
        }
    }

    /// Moves the cursor to the parent of the selected row.
    pub fn select_parent(&mut self) {
        let rows = self.rows();
        let Some(row) = rows.get(self.cursor) else {
            return;
        };
        if let Some(i) = rows[..self.cursor]
            .iter()
            .rposition(|r| r.depth < row.depth)
        {
            self.cursor = i;
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let n = self.rows().len();
        if n == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as isize + delta).clamp(0, n as isize - 1) as usize;
    }

    pub fn clamp(&mut self) {
        let n = self.rows().len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
    }

    /// Scrolls so that the cursor is inside a window of `height` rows.
    pub fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.cursor < self.offset {
            self.offset = self.cursor;
        } else if self.cursor >= self.offset + height {
            self.offset = self.cursor + 1 - height;
        }
        let n = self.rows().len();
        self.offset = self.offset.min(n.saturating_sub(height));
    }

    /// Name used by quick search (D-036).
    pub fn row_name(&self, row: Row) -> String {
        match row.kind {
            RowKind::Server => self.servers[row.server].conn.name.clone(),
            RowKind::Node(nid) => self.node(nid).map(|n| n.name.clone()).unwrap_or_default(),
            RowKind::Loading(_) => String::new(),
        }
    }

    pub fn row_text(&self, row: Row) -> RowText {
        let expand = |e: bool| if e { '▼' } else { '►' };
        let state_suffix = |state: ConnState| match state {
            ConnState::Disconnected => "",
            ConnState::Connecting => " (connecting…)",
            ConnState::Connected => " (connected)",
        };
        match row.kind {
            RowKind::Server => {
                let s = &self.servers[row.server];
                RowText {
                    depth: 0,
                    marker: expand(s.expanded && s.state == ConnState::Connected),
                    icon: Some(Icon::Server),
                    label: format!("{}{}", s.conn.name, state_suffix(s.state)),
                }
            }
            RowKind::Node(nid) => {
                let Some(n) = self.node(nid) else {
                    return RowText {
                        depth: row.depth,
                        marker: ' ',
                        icon: None,
                        label: String::new(),
                    };
                };
                let (icon, label) = match n.kind {
                    NodeKind::Group(_) => {
                        let label = match &n.children {
                            Some(c) => format!("{} ({})", n.label, c.len()),
                            None => n.label.clone(),
                        };
                        (Icon::Folder, label)
                    }
                    NodeKind::Object(t) => {
                        let suffix = n.conn.as_ref().map_or("", |c| state_suffix(c.state));
                        (t.icon(), format!("{}{suffix}", n.label))
                    }
                };
                RowText {
                    depth: row.depth,
                    marker: if n.expandable() {
                        expand(n.expanded)
                    } else {
                        ' '
                    },
                    icon: Some(icon),
                    label,
                }
            }
            RowKind::Loading(_) => RowText {
                depth: row.depth,
                marker: ' ',
                icon: None,
                label: "loading…".to_owned(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(name: &str) -> SavedConnection {
        SavedConnection {
            name: name.into(),
            ..SavedConnection::default()
        }
    }

    fn row(oid: u32, name: &str) -> NodeRow {
        NodeRow {
            oid,
            name: name.into(),
            label: None,
            partitioned: false,
        }
    }

    fn group(t: &Tree, server: usize, obj: ObjType) -> u64 {
        t.servers[server]
            .children
            .iter()
            .find(|n| n.kind == NodeKind::Group(obj))
            .unwrap()
            .nid
    }

    /// Connected server `a` with database `app` (connected) → Schemas → `public`.
    fn sample() -> (Tree, u64, u64, u64) {
        let mut t = Tree::new(&[conn("a"), conn("b")]);
        let uid = t.servers[0].uid;
        t.servers[0].state = ConnState::Connected;
        t.server_connected(uid);
        let dbs = group(&t, 0, ObjType::Database);
        t.set_children(dbs, vec![row(5, "app")]);
        let db = t.node(dbs).unwrap().children.as_ref().unwrap()[0].nid;
        let groups = t.groups(&ObjType::Database.children(false));
        let node = t.node_mut(db).unwrap();
        node.children = Some(groups);
        node.conn.as_mut().unwrap().state = ConnState::Connected;
        let schemas = t
            .node(db)
            .unwrap()
            .children
            .as_ref()
            .unwrap()
            .iter()
            .find(|n| n.kind == NodeKind::Group(ObjType::Schema))
            .unwrap()
            .nid;
        t.set_children(schemas, vec![row(2200, "public")]);
        (t, dbs, db, schemas)
    }

    #[test]
    fn rows_follow_expansion() {
        let (mut t, dbs, db, schemas) = sample();
        // server expanded: Databases, Roles, Tablespaces
        assert_eq!(t.rows().len(), 1 + 3 + 1);
        t.node_mut(dbs).unwrap().expanded = true;
        assert_eq!(t.row_text(t.rows()[1]).label, "Databases (1)");
        t.node_mut(db).unwrap().expanded = true;
        t.node_mut(schemas).unwrap().expanded = true;
        let labels: Vec<String> = t.rows().iter().map(|r| t.row_text(*r).label).collect();
        assert!(labels.contains(&"app (connected)".to_owned()), "{labels:?}");
        assert!(labels.contains(&"Schemas (1)".to_owned()), "{labels:?}");
        assert!(labels.contains(&"public".to_owned()), "{labels:?}");
    }

    #[test]
    fn load_context_finds_connection_and_parent() {
        let (mut t, dbs, db, schemas) = sample();
        let public = t.node(schemas).unwrap().children.as_ref().unwrap()[0].nid;
        let tables = t.node(public).unwrap().children.as_ref().unwrap()[0].nid;
        assert_eq!(
            t.load_context(tables),
            Some(LoadContext {
                server: 0,
                obj: ObjType::Table,
                database: Some(db),
                parent_oid: Some(2200),
            })
        );
        assert_eq!(
            t.load_context(dbs),
            Some(LoadContext {
                server: 0,
                obj: ObjType::Database,
                database: None,
                parent_oid: None,
            })
        );
        // Columns of a table: parent is the table.
        t.set_children(tables, vec![row(16400, "t")]);
        let table = t.node(tables).unwrap().children.as_ref().unwrap()[0].nid;
        let columns = t.node(table).unwrap().children.as_ref().unwrap()[0].nid;
        assert_eq!(t.load_context(columns).unwrap().parent_oid, Some(16400));
        assert!(t.node_mut(db).is_some());
    }

    #[test]
    fn reload_keeps_existing_nodes() {
        let (mut t, _dbs, _db, schemas) = sample();
        let public = t.node(schemas).unwrap().children.as_ref().unwrap()[0].nid;
        t.node_mut(public).unwrap().expanded = true;
        t.set_children(schemas, vec![row(2200, "public"), row(3000, "app")]);
        let kids = t.node(schemas).unwrap().children.as_ref().unwrap();
        assert_eq!(kids.len(), 2);
        let kept = kids.iter().find(|n| n.oid == 2200).unwrap();
        assert_eq!(kept.nid, public);
        assert!(kept.expanded);
        t.set_children(schemas, vec![row(3000, "app")]);
        assert!(t.node(public).is_none());
    }

    #[test]
    fn loaded_groups_and_reset() {
        let (mut t, dbs, _db, schemas) = sample();
        let all = t.all_loaded_groups();
        assert!(all.contains(&dbs) && all.contains(&schemas));
        assert_eq!(all.len(), 2);
        let handles = t.servers[0].reset();
        assert!(handles.is_empty());
        assert_eq!(t.rows().len(), 2);
    }

    #[test]
    fn cursor_scrolling_and_parent() {
        let names: Vec<_> = (0..10).map(|i| conn(&i.to_string())).collect();
        let mut t = Tree::new(&names);
        t.move_cursor(7);
        t.ensure_visible(4);
        assert_eq!((t.cursor, t.offset), (7, 4));
        t.move_cursor(-100);
        t.ensure_visible(4);
        assert_eq!((t.cursor, t.offset), (0, 0));
        t.move_cursor(100);
        assert_eq!(t.cursor, 9);
        let uid = t.servers[9].uid;
        t.remove(uid);
        assert_eq!(t.cursor, 8);

        let (mut t, dbs, _, _) = sample();
        t.select_kind(RowKind::Node(dbs));
        t.select_parent();
        assert_eq!(t.selected().unwrap().kind, RowKind::Server);
    }
}
