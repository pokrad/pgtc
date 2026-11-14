//! Find object (D-055): `Ctrl+F` in the tree searches the objects below the selected
//! row through tree connections — on a server row every database of the server — and
//! opens the tree down to the chosen object, loading every level on the way.

use std::collections::{HashMap, HashSet};

use pgc_db::DbError;
use pgc_sql::RenderContext;

use super::{App, Effect, Focus};
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::objects::ObjType;
use crate::tree::{ConnState, NodeKind, RowKind};

/// Most rows shown; each query reads one more to know there are more.
const MAX_HITS: usize = 200;
/// Database connections opened at the same time for a search of all databases.
const MAX_CONNECTING: usize = 8;

/// Where a search looks: server, one database (nid, oid) or all databases of the
/// server, and the path prefix of the selected row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Scope {
    pub uid: u64,
    pub database: Option<(u64, u32)>,
    /// Server row or Databases group: the server's objects and every database.
    pub all_databases: bool,
    pub prefix: String,
    /// The selected row, for the dialog.
    pub label: String,
}

/// What answers a search: the server connection or a database's tree connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Source {
    Server,
    Database(u64),
}

/// One match: shown columns, the tree path from the server down, the type order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Hit {
    pub kind: String,
    pub database: String,
    pub schema: String,
    pub name: String,
    pub steps: Vec<(ObjType, u32)>,
    pub sort: i32,
    /// The database node the object is in.
    pub db: Option<u64>,
}

/// The open Find object dialog.
#[derive(Debug)]
pub(super) struct FindState {
    pub scope: Scope,
    /// Text, match mode and "Show columns" of the current search.
    pub seen: (String, Match, bool),
    /// Lookup key → source, for the current search (older answers are dropped).
    pub pending: HashMap<u64, Source>,
    /// Sources asked for the current search.
    pub searched: HashSet<Source>,
    /// Answers of the current search: matches and "more than the limit".
    pub results: HashMap<Source, (Vec<Hit>, bool)>,
    /// The merged list shown.
    pub hits: Vec<Hit>,
    /// Databases connected only for the search (closed again with the dialog).
    pub opened: HashSet<u64>,
    /// Databases that could not be opened: (nid, short reason).
    pub failed: Vec<(u64, String)>,
    /// A search error (server or the one database).
    pub error: Option<String>,
}

/// Opening the tree down to an object, level by level.
#[derive(Debug, Clone)]
pub(super) struct Reveal {
    pub uid: u64,
    pub steps: Vec<(ObjType, u32)>,
    /// Groups loaded and databases connected for it (a second failure ends it).
    pub tried: HashSet<u64>,
}

/// How the name is compared with the typed text (D-055).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Match {
    Contains,
    StartsWith,
    EndsWith,
}

impl Match {
    /// Combo entries, in this order; the first is the default.
    pub const ALL: [Match; 3] = [Match::Contains, Match::StartsWith, Match::EndsWith];

    pub fn label(self) -> &'static str {
        match self {
            Match::Contains => "Contains",
            Match::StartsWith => "Starts with",
            Match::EndsWith => "Ends with",
        }
    }
}

/// `text` as a case-insensitive LIKE pattern for `mode` (backslash escapes).
pub(super) fn like_pattern(text: &str, mode: Match) -> String {
    let mut p = String::new();
    if mode != Match::StartsWith {
        p.push('%');
    }
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            p.push('\\');
        }
        p.push(c);
    }
    if mode != Match::EndsWith {
        p.push('%');
    }
    p
}

/// `schemas:2200/tables:16384` → steps; `None` for an unknown folder or a bad oid.
pub(super) fn parse_path(path: &str) -> Option<Vec<(ObjType, u32)>> {
    path.split('/')
        .map(|step| {
            let (folder, oid) = step.split_once(':')?;
            Some((ObjType::from_folder(folder)?, oid.parse().ok()?))
        })
        .collect()
}

/// `text` cut to `width` characters (`…` when cut) and padded.
fn cell(text: &str, width: usize) -> String {
    let mut s: String = text.chars().take(width).collect();
    if text.chars().count() > width {
        s.pop();
        s.push('…');
    }
    format!("{s:<width$}")
}

/// A list row (and the header): Database, Schema, Name, Type.
pub(super) fn row_text(database: &str, schema: &str, name: &str, kind: &str) -> String {
    format!(
        "{} {} {} {kind}",
        cell(database, 14),
        cell(schema, 14),
        cell(name, 30)
    )
}

/// Matches before `b` in the merged list: type order, then database, schema, name.
fn order(a: &Hit, b: &Hit) -> std::cmp::Ordering {
    (a.sort, &a.database, &a.schema, &a.name).cmp(&(b.sort, &b.database, &b.schema, &b.name))
}

impl App {
    /// The selected row as a search scope (D-055); `Err` says why not.
    pub(super) fn find_scope(&self) -> Result<Scope, &'static str> {
        let row = self
            .tree
            .selected()
            .ok_or("Select a server or an object first")?;
        let server = &self.tree.servers[row.server];
        if server.state != ConnState::Connected {
            return Err("Connect the server first");
        }
        let uid = server.uid;
        let nid = match row.kind {
            RowKind::Server => {
                return Ok(Scope {
                    uid,
                    database: None,
                    all_databases: true,
                    prefix: String::new(),
                    label: format!("{} (all databases)", server.conn.name),
                });
            }
            RowKind::Node(nid) | RowKind::Loading(nid) => nid,
        };
        let (_, chain) = self.tree.chain(nid).ok_or("")?;
        let label = self
            .tree
            .node(nid)
            .map(|n| n.label.clone())
            .unwrap_or_default();
        let nothing = "There is nothing below this object to find";
        let Some(db) = chain
            .iter()
            .position(|(k, ..)| *k == NodeKind::Object(ObjType::Database))
        else {
            // server level: a group (Databases, roles, tablespaces), or one of them
            return match chain.last() {
                Some((NodeKind::Group(t), ..)) => Ok(Scope {
                    uid,
                    database: None,
                    all_databases: *t == ObjType::Database,
                    prefix: format!("{}:", t.folder()),
                    label,
                }),
                _ => Err(nothing),
            };
        };
        let (_, db_oid, db_nid) = chain[db];
        let below = &chain[db + 1..];
        let objects: Vec<String> = below
            .iter()
            .filter_map(|(k, oid, _)| match k {
                NodeKind::Object(t) => Some(format!("{}:{oid}", t.folder())),
                NodeKind::Group(_) => None,
            })
            .collect();
        let mut prefix = objects.join("/");
        match below.last() {
            None => {}
            Some((NodeKind::Group(t), ..)) => {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(&format!("{}:", t.folder()));
            }
            Some((NodeKind::Object(_), _, n)) => {
                let has_groups = self
                    .tree
                    .node(*n)
                    .and_then(|n| n.children.as_ref())
                    .is_some_and(|c| !c.is_empty());
                if !has_groups {
                    return Err(nothing);
                }
                prefix.push('/');
            }
        }
        Ok(Scope {
            uid,
            database: Some((db_nid, db_oid)),
            all_databases: false,
            prefix,
            label,
        })
    }

    /// `Ctrl+F` in the tree: the Find object dialog (D-055).
    pub(super) fn open_find_object(&mut self) {
        let scope = match self.find_scope() {
            Ok(s) => s,
            Err(why) => {
                if !why.is_empty() {
                    self.status = why.to_owned();
                }
                return;
            }
        };
        // a database row that is not connected yet: connect (the search waits)
        if let Some((nid, _)) = scope.database
            && self
                .tree
                .node(nid)
                .and_then(|n| n.conn.as_ref())
                .is_none_or(|c| c.state == ConnState::Disconnected)
        {
            self.expand_node(nid);
        }
        let rows = self.screen.height.saturating_sub(15).clamp(5, 20);
        self.dialogs.push(Dialog::new(
            DialogKind::FindObject,
            "Find object",
            80,
            vec![
                Widget::field("text", "N&ame:", "", 16),
                Widget::combo(
                    "match",
                    "&Match:",
                    Match::ALL.iter().map(|m| m.label().to_owned()).collect(),
                    0,
                    16,
                ),
                Widget::checkbox("columns", "Show &columns", false),
                Widget::label_id("scope", &format!("Below: {}", scope.label)),
                Widget::label_id("info", "Type a part of the name"),
                Widget::label(&format!(
                    " {}",
                    row_text("Database", "Schema", "Name", "Type")
                )),
                Widget::list("list", "", Vec::new(), 0, rows),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "Ca&ncel", false),
            ],
        ));
        self.find = Some(FindState {
            scope,
            seen: (String::new(), Match::Contains, false),
            pending: HashMap::new(),
            searched: HashSet::new(),
            results: HashMap::new(),
            hits: Vec::new(),
            opened: HashSet::new(),
            failed: Vec::new(),
            error: None,
        });
        // all databases: start opening them while the user types
        self.find_prepare();
    }

    /// After every update: a closed dialog ends the search; a changed text, match mode
    /// or "Show columns" starts the pause before the next search.
    pub(super) fn sync_find_dialog(&mut self) {
        let Some(f) = self.find.as_mut() else {
            return;
        };
        let Some(d) = self
            .dialogs
            .iter()
            .rev()
            .find(|d| d.kind == DialogKind::FindObject)
        else {
            self.end_find(None);
            return;
        };
        let mode = Match::ALL[d.selected("match").unwrap_or(0).min(Match::ALL.len() - 1)];
        let now = (
            d.text("text").unwrap_or("").to_owned(),
            mode,
            d.checked("columns"),
        );
        if now != f.seen {
            f.seen = now;
            self.find_seq += 1;
            self.effects
                .push(Effect::Timer(super::Timer::Find(self.find_seq)));
        }
    }

    /// Ends the search: databases opened only for it are closed, except `keep` (the
    /// one the tree opens into).
    fn end_find(&mut self, keep: Option<u64>) {
        let Some(f) = self.find.take() else {
            return;
        };
        for nid in f.opened {
            if Some(nid) == keep {
                continue;
            }
            let handle = self
                .tree
                .node_mut(nid)
                .and_then(|n| n.conn.as_mut())
                .and_then(|c| c.handle.take());
            if let Some(h) = handle {
                self.connections.remove(&h.id);
                h.close();
            }
            self.database_reset(nid);
        }
    }

    fn find_dialog(&mut self) -> Option<&mut Dialog> {
        self.dialogs
            .iter_mut()
            .rev()
            .find(|d| d.kind == DialogKind::FindObject)
    }

    /// The Databases group of the search's server.
    fn find_databases_group(&self) -> Option<u64> {
        let f = self.find.as_ref()?;
        let si = self.tree.index_of(f.scope.uid)?;
        self.tree.servers[si]
            .children
            .iter()
            .find(|n| n.kind == NodeKind::Group(ObjType::Database))
            .map(|n| n.nid)
    }

    /// Database nodes of the search's server (when the Databases group is loaded).
    fn find_database_nodes(&self) -> Vec<u64> {
        self.find_databases_group()
            .and_then(|g| self.tree.node(g))
            .and_then(|g| g.children.as_ref())
            .map(|c| {
                c.iter()
                    .filter(|n| n.is_database())
                    .map(|n| n.nid)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Who answers the search.
    fn find_sources(&self) -> Vec<Source> {
        let Some(f) = self.find.as_ref() else {
            return Vec::new();
        };
        if let Some((nid, _)) = f.scope.database {
            return vec![Source::Database(nid)];
        }
        let mut out = vec![Source::Server];
        if f.scope.all_databases {
            out.extend(self.find_database_nodes().into_iter().map(Source::Database));
        }
        out
    }

    /// All databases: the Databases group is loaded, then the databases that are not
    /// connected are opened, at most [`MAX_CONNECTING`] at a time.
    fn find_prepare(&mut self) {
        if !self.find.as_ref().is_some_and(|f| f.scope.all_databases) {
            return;
        }
        let Some(group) = self.find_databases_group() else {
            return;
        };
        let (loaded, loading) = self
            .tree
            .node(group)
            .map_or((false, false), |g| (g.children.is_some(), g.loading));
        if !loaded {
            if !loading {
                self.load_group(group);
            }
            return;
        }
        let state = |app: &App, nid: u64| {
            app.tree
                .node(nid)
                .and_then(|n| n.conn.as_ref())
                .map_or(ConnState::Disconnected, |c| c.state)
        };
        let Some(f) = self.find.as_ref() else {
            return;
        };
        let mut connecting = f
            .opened
            .iter()
            .filter(|n| state(self, **n) == ConnState::Connecting)
            .count();
        let mut open = Vec::new();
        for nid in self.find_database_nodes() {
            if connecting >= MAX_CONNECTING {
                break;
            }
            let failed = f.failed.iter().any(|(n, _)| *n == nid);
            if !failed && !f.opened.contains(&nid) && state(self, nid) == ConnState::Disconnected {
                open.push(nid);
                connecting += 1;
            }
        }
        for nid in open {
            self.connect_database(nid);
            if let Some(f) = self.find.as_mut() {
                f.opened.insert(nid);
            }
        }
    }

    /// A database opened for the search could not be connected: listed as not
    /// searched, no error dialog. `false` when the search did not open it.
    pub(super) fn find_connect_failed(&mut self, nid: u64, error: &DbError) -> bool {
        let Some(f) = self.find.as_mut() else {
            return false;
        };
        if !f.opened.remove(&nid) {
            return false;
        }
        let reason = match error {
            DbError::PasswordRequired => "password needed".to_owned(),
            DbError::AuthFailed(_) => "login failed".to_owned(),
            DbError::Other(e) => e.lines().next().unwrap_or("").chars().take(40).collect(),
        };
        f.failed.push((nid, reason));
        self.find_show();
        true
    }

    /// Runs the search for the dialog's text (after the pause, D-055).
    pub(super) fn find_search(&mut self) {
        let Some(f) = self.find.as_mut() else {
            return;
        };
        f.pending.clear();
        f.searched.clear();
        f.results.clear();
        f.error = None;
        self.find_send();
        self.find_show();
    }

    /// Sends the current search to every source that is ready and not asked yet.
    fn find_send(&mut self) {
        self.find_prepare();
        let Some(f) = self.find.as_ref() else {
            return;
        };
        let (text, mode, columns) = f.seen.clone();
        if text.trim().is_empty() {
            return;
        }
        let scope = f.scope.clone();
        let Some(si) = self.tree.index_of(scope.uid) else {
            return;
        };
        let Some(version) = self.tree.servers[si].version else {
            return;
        };
        let render = RenderContext {
            version,
            show_system: self.settings.ui.show_system_objects,
        };
        let pattern = like_pattern(text.trim(), mode);
        for source in self.find_sources() {
            if self
                .find
                .as_ref()
                .is_some_and(|f| f.searched.contains(&source))
            {
                continue;
            }
            let handle = match source {
                Source::Server => self.tree.servers[si].handle.clone(),
                Source::Database(nid) => self
                    .tree
                    .node(nid)
                    .and_then(|n| n.conn.as_ref())
                    .filter(|c| c.state == ConnState::Connected)
                    .and_then(|c| c.handle.clone()),
            };
            let Some(handle) = handle else {
                continue;
            };
            let (file, prefix) = match source {
                Source::Server => ("server.sql", scope.prefix.clone()),
                // a database of an all-databases search: everything in it
                Source::Database(_) if scope.all_databases => ("database.sql", String::new()),
                Source::Database(_) => ("database.sql", scope.prefix.clone()),
            };
            let sql = match self.sql.render("find", file, render) {
                Ok(sql) => sql,
                Err(e) => {
                    if let Some(f) = self.find.as_mut() {
                        f.error = Some(e.to_string());
                    }
                    return;
                }
            };
            self.next_key += 1;
            let key = self.next_key;
            if let Some(f) = self.find.as_mut() {
                f.pending.insert(key, source);
                f.searched.insert(source);
            }
            handle.lookup_with(
                key,
                sql,
                vec![
                    Some(pattern.clone()),
                    Some(prefix),
                    Some(columns.to_string()),
                ],
            );
        }
    }

    /// Answer of one source to [`Self::find_search`] (stale answers are dropped).
    pub(super) fn find_loaded(&mut self, key: u64, result: Result<Vec<Vec<String>>, DbError>) {
        let Some(f) = self.find.as_ref() else {
            return;
        };
        let Some(&source) = f.pending.get(&key) else {
            return;
        };
        let (database, db_oid, db) = match source {
            Source::Server => (String::new(), None, None),
            Source::Database(nid) => {
                let n = self.tree.node(nid);
                (
                    n.map(|n| n.name.clone()).unwrap_or_default(),
                    n.map(|n| n.oid),
                    Some(nid),
                )
            }
        };
        let Some(f) = self.find.as_mut() else {
            return;
        };
        f.pending.remove(&key);
        let rows = match result {
            Ok(rows) => rows,
            Err(e) => {
                match source {
                    Source::Database(nid) if f.scope.all_databases => {
                        f.failed
                            .push((nid, e.to_string().chars().take(40).collect()));
                    }
                    _ => f.error = Some(format!("Search failed: {e}")),
                }
                f.results.insert(source, (Vec::new(), false));
                self.find_show();
                return;
            }
        };
        let more = rows.len() > MAX_HITS;
        let first = db_oid.map(|oid| (ObjType::Database, oid));
        let hits = rows
            .into_iter()
            .take(MAX_HITS)
            .filter_map(|r| {
                let [kind, schema, name, path, sort]: [String; 5] = r.try_into().ok()?;
                let mut steps: Vec<_> = first.into_iter().collect();
                steps.extend(parse_path(&path)?);
                Some(Hit {
                    kind,
                    database: database.clone(),
                    schema,
                    name,
                    steps,
                    sort: sort.parse().unwrap_or(i32::MAX),
                    db,
                })
            })
            .collect();
        f.results.insert(source, (hits, more));
        self.find_show();
    }

    /// The merged list and the info line.
    fn find_show(&mut self) {
        let Some(f) = self.find.as_mut() else {
            return;
        };
        let mut all: Vec<Hit> = f.results.values().flat_map(|(h, _)| h.clone()).collect();
        all.sort_by(order);
        let more = all.len() > MAX_HITS || f.results.values().any(|(_, m)| *m);
        all.truncate(MAX_HITS);
        f.hits = all;
        let items: Vec<String> = f
            .hits
            .iter()
            .map(|h| row_text(&h.database, &h.schema, &h.name, &h.kind))
            .collect();
        let searching = !f.seen.0.trim().is_empty();
        let mut info = if !searching {
            "Type a part of the name".to_owned()
        } else if let Some(e) = &f.error {
            e.clone()
        } else {
            match items.len() {
                0 if f.results.is_empty() => "Searching…".to_owned(),
                0 => "No objects found".to_owned(),
                1 => "1 object".to_owned(),
                n if more => format!("{n}+ objects — type more to narrow the search"),
                n => format!("{n} objects"),
            }
        };
        if f.scope.all_databases {
            let dbs: Vec<u64> = f
                .results
                .keys()
                .chain(f.pending.values())
                .filter_map(|s| match s {
                    Source::Database(n) => Some(*n),
                    Source::Server => None,
                })
                .collect();
            let done = f
                .results
                .keys()
                .filter(|s| matches!(s, Source::Database(_)))
                .count();
            if searching && done < dbs.len() {
                info.push_str(&format!(" — {done} of {} databases", dbs.len()));
            }
        }
        if !f.failed.is_empty() {
            let names: Vec<String> = f
                .failed
                .iter()
                .map(|(nid, why)| {
                    let name = self
                        .tree
                        .node(*nid)
                        .map(|n| n.name.clone())
                        .unwrap_or_default();
                    format!("{name} ({why})")
                })
                .collect();
            info.push_str(&format!(" — not searched: {}", names.join(", ")));
        }
        let info: String = info.chars().take(76).collect();
        let selected = self
            .find_dialog()
            .and_then(|d| d.selected("list"))
            .unwrap_or(0);
        if let Some(d) = self.find_dialog() {
            d.set_list("list", items, Vec::new(), selected);
            d.set_label("info", &info);
        }
    }

    /// OK: open the tree down to the selected match.
    pub(super) fn find_chosen(&mut self, dialog: Dialog) {
        let index = dialog.selected("list").unwrap_or(0);
        let Some(hit) = self.find.as_ref().and_then(|f| f.hits.get(index)).cloned() else {
            // nothing to choose: the dialog stays
            self.dialogs.push(dialog);
            return;
        };
        let uid = self.find.as_ref().map_or(0, |f| f.scope.uid);
        // the database the tree opens into stays connected
        self.end_find(hit.db);
        self.reveal = Some(Reveal {
            uid,
            steps: hit.steps,
            tried: HashSet::new(),
        });
        self.continue_reveal();
    }

    /// After a tree connection event: databases that became ready are searched (and
    /// more are opened), an opening tree goes on.
    pub(super) fn after_tree_event(&mut self) {
        if self.find.is_some() {
            self.find_send();
            self.find_show();
        }
        if self.reveal.is_some() {
            self.continue_reveal();
        }
    }

    /// Opens the tree down to the object of [`Reveal`] as far as it is loaded; asks
    /// for the next level and waits, or ends on the object (cursor and focus on it).
    pub(super) fn continue_reveal(&mut self) {
        let Some(mut r) = self.reveal.take() else {
            return;
        };
        let Some(si) = self.tree.index_of(r.uid) else {
            return;
        };
        if self.tree.servers[si].state != ConnState::Connected {
            self.status = "Find object: the server is not connected".to_owned();
            return;
        }
        self.tree.servers[si].expanded = true;
        let mut parent: Option<u64> = None;
        let mut reached: Option<u64> = None;
        let last = r.steps.len().saturating_sub(1);
        let fail = |app: &mut App, reached: Option<u64>, why: String| {
            if let Some(n) = reached {
                app.tree.select_kind(RowKind::Node(n));
            }
            app.focus = Focus::Tree;
            app.status = why;
        };
        for (k, &(t, oid)) in r.steps.clone().iter().enumerate() {
            let kids: Vec<(NodeKind, u32, u64)> = match parent {
                None => self.tree.servers[si]
                    .children
                    .iter()
                    .map(|n| (n.kind, n.oid, n.nid))
                    .collect(),
                Some(p) => self
                    .tree
                    .node(p)
                    .and_then(|n| n.children.as_ref())
                    .map(|c| c.iter().map(|n| (n.kind, n.oid, n.nid)).collect())
                    .unwrap_or_default(),
            };
            let Some(group) = kids.iter().find(|c| c.0 == NodeKind::Group(t)).map(|c| c.2) else {
                fail(
                    self,
                    reached,
                    format!("Find object: no {} here", t.group_label()),
                );
                return;
            };
            let Some(g) = self.tree.node_mut(group) else {
                return;
            };
            g.expanded = true;
            if g.children.is_none() {
                let loading = g.loading;
                if loading || r.tried.insert(group) {
                    if !loading {
                        self.load_group(group);
                    }
                    if let Some(n) = reached {
                        self.tree.select_kind(RowKind::Node(n));
                    }
                    self.reveal = Some(r);
                } else {
                    fail(
                        self,
                        reached,
                        format!("Find object: cannot load {}", t.group_label()),
                    );
                }
                return;
            }
            let Some(obj) = self
                .tree
                .node(group)
                .and_then(|g| g.children.as_ref())
                .and_then(|c| {
                    c.iter()
                        .find(|n| n.kind == NodeKind::Object(t) && n.oid == oid)
                })
                .map(|n| n.nid)
            else {
                fail(
                    self,
                    Some(group),
                    "Find object: the object is not there any more (refresh the tree)".to_owned(),
                );
                return;
            };
            reached = Some(obj);
            if k == last {
                self.tree.select_kind(RowKind::Node(obj));
                self.focus = Focus::Tree;
                return;
            }
            let is_db = self.tree.node(obj).is_some_and(|n| n.is_database());
            if is_db {
                let state = self
                    .tree
                    .node(obj)
                    .and_then(|n| n.conn.as_ref())
                    .map_or(ConnState::Disconnected, |c| c.state);
                let ready = state == ConnState::Connected
                    && self.tree.node(obj).is_some_and(|n| n.children.is_some());
                if !ready {
                    if state == ConnState::Connecting || r.tried.insert(obj) {
                        if state == ConnState::Disconnected {
                            self.expand_node(obj);
                        }
                        self.tree.select_kind(RowKind::Node(obj));
                        self.reveal = Some(r);
                    } else {
                        fail(
                            self,
                            reached,
                            "Find object: cannot connect to the database".to_owned(),
                        );
                    }
                    return;
                }
            }
            if let Some(n) = self.tree.node_mut(obj) {
                n.expanded = true;
            }
            parent = Some(obj);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_escape_like_characters() {
        assert_eq!(like_pattern("ord", Match::Contains), "%ord%");
        assert_eq!(like_pattern("ord", Match::StartsWith), "ord%");
        assert_eq!(like_pattern("ord", Match::EndsWith), "%ord");
        assert_eq!(like_pattern("a_b%c\\", Match::Contains), "%a\\_b\\%c\\\\%");
    }

    #[test]
    fn paths_become_steps() {
        assert_eq!(
            parse_path("schemas:2200/tables:16384/columns:3"),
            Some(vec![
                (ObjType::Schema, 2200),
                (ObjType::Table, 16384),
                (ObjType::Column, 3)
            ])
        );
        assert_eq!(
            parse_path("catalogs:11/views:12000"),
            Some(vec![(ObjType::Catalog, 11), (ObjType::View, 12000)])
        );
        assert_eq!(parse_path("nope:1"), None);
        assert_eq!(parse_path("tables:x"), None);
    }
}
