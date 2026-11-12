//! Server and database connections (D-011, D-031, D-035): connecting, the events of
//! the tree's connections, the connection and login dialogs, deleting a connection.

use std::time::Duration;

use pgc_core::paths;
use pgc_core::settings::{SavedConnection, SslMode};
use pgc_db::conninfo::{PasswordPrompt, Resolved};
use pgc_db::pgpass::{self, PgPassKey};
use pgc_db::{ConnectParams, DbError, DbEvent};

use super::{App, Focus, Target};
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::objects::ObjType;
use crate::tree::{ConnState, DbConn};

const CONN_LABEL_WIDTH: u16 = 19;

impl App {
    /// D-031: reuse an identical saved connection or save a new one, then connect.
    pub(super) fn startup_connection(&mut self, resolved: Resolved) {
        let existing = self
            .tree
            .servers
            .iter()
            .find(|s| s.conn.same_target(&resolved.connection))
            .map(|s| s.uid);
        let uid = match existing {
            Some(uid) => uid,
            None => {
                let uid = self.tree.add(resolved.connection);
                self.persist_servers();
                uid
            }
        };
        let prompt = resolved.password_prompt;
        if let Some(node) = self.tree.by_uid(uid) {
            if resolved.password.is_some() {
                node.password = resolved.password;
            }
            node.never_prompt = prompt == PasswordPrompt::Never;
        }
        self.tree.select_server(uid);
        if prompt == PasswordPrompt::Always {
            self.open_login(uid, None);
        } else {
            self.connect_server(uid);
        }
    }

    pub(super) fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(
            self.settings.connection.connect_timeout_secs.max(1),
        ))
    }

    pub(super) fn pgpass_password(conn: &SavedConnection, database: &str) -> Option<String> {
        let path = paths::pgpass_file()?;
        pgpass::read_password(
            &path,
            PgPassKey {
                host: &conn.host,
                port: conn.port,
                database,
                username: &conn.username,
            },
        )
    }

    pub(super) fn connect_server(&mut self, uid: u64) {
        let timeout = self.timeout();
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        if node.state != ConnState::Disconnected {
            return;
        }
        let password = node
            .password
            .clone()
            .or_else(|| Self::pgpass_password(&node.conn, &node.conn.database));
        // Kept for the session: per-database connections reuse it (D-035).
        node.password = password.clone();
        let params = ConnectParams::new(&node.conn, &node.conn.database, password, timeout);
        self.next_conn += 1;
        let id = self.next_conn;
        node.state = ConnState::Connecting;
        node.handle = Some(pgc_db::spawn(id, params, self.sink.clone()));
        self.status = format!("Connecting to {}…", node.conn.name);
        self.connections.insert(id, Target::Server(uid));
    }

    pub(super) fn connect_database(&mut self, nid: u64) {
        let timeout = self.timeout();
        let Some(si) = self.tree.server_of(nid) else {
            return;
        };
        let s = &self.tree.servers[si];
        let (uid, conn, password) = (s.uid, s.conn.clone(), s.password.clone());
        let Some(node) = self.tree.node_mut(nid) else {
            return;
        };
        let name = node.name.clone();
        let password = password.or_else(|| Self::pgpass_password(&conn, &name));
        let params = ConnectParams::new(&conn, &name, password, timeout);
        self.next_conn += 1;
        let id = self.next_conn;
        node.conn = Some(DbConn {
            state: ConnState::Connecting,
            handle: Some(pgc_db::spawn(id, params, self.sink.clone())),
        });
        self.connections.insert(id, Target::Database(uid, nid));
    }

    pub(super) fn disconnect_server(&mut self, uid: u64) {
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        for handle in node.reset() {
            handle.close();
            self.connections.remove(&handle.id);
        }
    }

    pub(super) fn on_db(&mut self, event: DbEvent) {
        let id = match &event {
            DbEvent::Connected { id, .. }
            | DbEvent::ConnectFailed { id, .. }
            | DbEvent::Nodes { id, .. }
            | DbEvent::Definition { id, .. }
            | DbEvent::Query { id, .. }
            | DbEvent::Notice { id, .. }
            | DbEvent::Lookup { id, .. }
            | DbEvent::Model { id, .. }
            | DbEvent::Closed { id, .. } => *id,
        };
        // Events of connections closed on request are no longer interesting.
        let Some(target) = self.connections.get(&id).cloned() else {
            return;
        };
        match (target, event) {
            (Target::Query(tab), event) => self.on_query_event(id, tab, event),
            (Target::Structure, event) => self.on_job_event(id, event),
            // Find object searches run on tree connections (D-055)
            (_, DbEvent::Lookup { key, result, .. }) => self.find_loaded(key, result),
            (_, DbEvent::Query { .. } | DbEvent::Notice { .. } | DbEvent::Model { .. }) => {}
            (_, DbEvent::Nodes { key, result, .. }) => {
                let Some(nid) = self.pending.remove(&key) else {
                    return;
                };
                match result {
                    Ok(rows) => self.tree.set_children(nid, rows),
                    Err(e) => {
                        let label = self.tree.node(nid).map(|n| n.label.clone());
                        if let Some(n) = self.tree.node_mut(nid) {
                            n.loading = false;
                            n.expanded = false;
                        }
                        if let Some(label) = label {
                            self.error(&format!("Cannot load {label}:\n{e}"));
                        }
                    }
                }
            }
            (_, DbEvent::Definition { key, result, .. }) => self.definition_loaded(key, result),
            (Target::Server(uid), DbEvent::Connected { version, .. }) => {
                self.server_connected(uid, version);
            }
            (Target::Server(uid), DbEvent::ConnectFailed { error, .. }) => {
                self.connections.remove(&id);
                self.server_failed(uid, error);
            }
            (Target::Server(uid), DbEvent::Closed { error, .. }) => {
                self.connections.remove(&id);
                if let Some(node) = self.tree.by_uid(uid) {
                    let name = node.conn.name.clone();
                    for handle in node.reset() {
                        handle.close();
                        self.connections.remove(&handle.id);
                    }
                    self.status = match error {
                        Some(e) => format!("Connection to {name} lost: {e}"),
                        None => format!("Disconnected from {name}"),
                    };
                }
            }
            (Target::Database(_, nid), event) => {
                let Some(name) = self.tree.node(nid).map(|n| n.name.clone()) else {
                    // The database disappeared from the tree (refresh); its connection
                    // was dropped with the node.
                    self.connections.remove(&id);
                    return;
                };
                match event {
                    DbEvent::Connected { .. } => {
                        let groups = self.tree.groups(&ObjType::Database.children(false));
                        if let Some(n) = self.tree.node_mut(nid) {
                            if let Some(c) = &mut n.conn {
                                c.state = ConnState::Connected;
                            }
                            n.children = Some(groups);
                        }
                    }
                    DbEvent::ConnectFailed { error, .. } => {
                        self.connections.remove(&id);
                        self.database_reset(nid);
                        // opened by Find object: listed there, no dialog (D-055)
                        if !self.find_connect_failed(nid, &error) {
                            self.error(&format!("Cannot connect to database {name}:\n{error}"));
                        }
                    }
                    DbEvent::Closed { error, .. } => {
                        self.connections.remove(&id);
                        self.database_reset(nid);
                        if let Some(e) = error {
                            self.status = format!("Connection to database {name} lost: {e}");
                        }
                    }
                    DbEvent::Nodes { .. }
                    | DbEvent::Definition { .. }
                    | DbEvent::Query { .. }
                    | DbEvent::Notice { .. }
                    | DbEvent::Lookup { .. }
                    | DbEvent::Model { .. } => {}
                }
            }
        }
    }

    pub(super) fn database_reset(&mut self, nid: u64) {
        if let Some(n) = self.tree.node_mut(nid) {
            n.conn = Some(DbConn {
                state: ConnState::Disconnected,
                handle: None,
            });
            n.expanded = false;
            n.children = None;
        }
    }

    pub(super) fn server_connected(&mut self, uid: u64, version: pgc_core::ServerVersion) {
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        node.state = ConnState::Connected;
        node.version = Some(version);
        let name = node.conn.name.clone();
        let mut status = format!("Connected to {name} (PostgreSQL {version})");
        if version.is_unverified() {
            status = format!(
                "Connected to {name} — PostgreSQL {} – not yet verified with pgtc",
                version.major()
            );
        }
        // D-035: write .pgpass only after a successful connect.
        if node.save_password
            && let Some(password) = node.password.clone()
        {
            node.save_password = false;
            let conn = node.conn.clone();
            if let Err(e) = Self::save_pgpass(&conn, &password) {
                tracing::error!("saving password: {e}");
                status = format!("{status}; password not saved: {e}");
            } else {
                status = format!("{status}; password saved");
            }
        }
        self.status = status;
        // Groups appear; Databases is opened at once.
        self.tree.server_connected(uid);
        if let Some(databases) = self
            .tree
            .by_uid(uid)
            .and_then(|s| s.children.first())
            .map(|n| n.nid)
        {
            self.expand_node(databases);
        }
    }

    fn save_pgpass(conn: &SavedConnection, password: &str) -> Result<(), DbError> {
        let path = paths::pgpass_file()
            .ok_or_else(|| DbError::Other("cannot locate the .pgpass file".to_owned()))?;
        pgpass::save_password(
            &path,
            PgPassKey {
                host: &conn.host,
                port: conn.port,
                database: &conn.database,
                username: &conn.username,
            },
            password,
        )
    }

    fn server_failed(&mut self, uid: u64, error: DbError) {
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        node.handle = None;
        node.state = ConnState::Disconnected;
        let name = node.conn.name.clone();
        match error {
            DbError::PasswordRequired if node.never_prompt => {
                self.status = format!("Cannot connect to {name}");
                self.error(&format!(
                    "Cannot connect to {name}:\nthe server requires a password and -w was given"
                ));
            }
            DbError::PasswordRequired => self.open_login(uid, None),
            DbError::AuthFailed(msg) => {
                node.password = None;
                self.open_login(uid, Some(msg));
            }
            DbError::Other(msg) => {
                self.status = format!("Cannot connect to {name}");
                self.error(&format!("Cannot connect to {name}:\n{msg}"));
            }
        }
    }

    pub(super) fn persist_servers(&mut self) {
        self.settings.servers = self.tree.saved();
        self.save_settings();
    }

    /// New (`uid` = None) or Edit connection dialog (D-035).
    pub(super) fn open_connection_dialog(&mut self, uid: Option<u64>) {
        let conn = uid
            .and_then(|u| self.tree.by_uid(u).map(|n| n.conn.clone()))
            .unwrap_or_default();
        let name = if uid.is_some() {
            conn.name.as_str()
        } else {
            ""
        };
        let modes: Vec<String> = SslMode::ALL.iter().map(|m| m.name().to_owned()).collect();
        let mode = SslMode::ALL
            .iter()
            .position(|m| *m == conn.ssl_mode)
            .unwrap_or(1);
        let lw = CONN_LABEL_WIDTH;
        let title = if uid.is_some() {
            "Edit connection"
        } else {
            "New connection"
        };
        let dialog = Dialog::new(
            DialogKind::Connection,
            title,
            64,
            vec![
                Widget::field("name", "&Name:", name, lw),
                Widget::label(""),
                Widget::field("host", "&Host:", &conn.host, lw),
                Widget::field("port", "&Port:", &conn.port.to_string(), lw),
                Widget::field("database", "Maintenance &DB:", &conn.database, lw),
                Widget::field("user", "&User name:", &conn.username, lw),
                Widget::field("password", "Pass&word:", "", lw).masked(),
                Widget::checkbox("save", "&Save password", false),
                Widget::label(""),
                Widget::combo("ssl", "SS&L mode:", modes, mode, lw),
                Widget::field(
                    "rootcert",
                    "Root cer&t. file:",
                    conn.ssl_root_cert.as_deref().unwrap_or(""),
                    lw,
                ),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        )
        .with_tag(uid.unwrap_or(0));
        self.dialogs.push(dialog);
    }

    pub(super) fn apply_connection(&mut self, dialog: Dialog) {
        let text = |id: &str| dialog.text(id).unwrap_or("").trim().to_owned();
        let port = match text("port").parse::<u16>() {
            Ok(p) if p > 0 => p,
            _ => {
                let mut dialog = dialog;
                dialog.focus_on("port");
                self.dialogs.push(dialog);
                self.error("Port must be a number from 1 to 65535.");
                return;
            }
        };
        let host = text("host");
        let username = text("user");
        if host.is_empty() || username.is_empty() {
            let mut dialog = dialog;
            dialog.focus_on(if host.is_empty() { "host" } else { "user" });
            self.dialogs.push(dialog);
            self.error("Host and user name are required.");
            return;
        }
        let mut database = text("database");
        if database.is_empty() {
            database = "postgres".to_owned();
        }
        let mut name = text("name");
        if name.is_empty() {
            name = format!("{username}@{host}:{port}");
        }
        let rootcert = text("rootcert");
        let conn = SavedConnection {
            name,
            host,
            port,
            database,
            username,
            ssl_mode: dialog
                .selected("ssl")
                .map_or(SslMode::Prefer, |i| SslMode::ALL[i]),
            ssl_root_cert: (!rootcert.is_empty()).then_some(rootcert),
        };
        // The password is not trimmed: spaces may be part of it.
        let password = dialog
            .text("password")
            .filter(|p| !p.is_empty())
            .map(str::to_owned);
        let save = dialog.checked("save");

        if dialog.tag == 0 {
            let uid = self.tree.add(conn);
            if let Some(node) = self.tree.by_uid(uid) {
                node.password = password;
                node.save_password = save;
            }
            self.persist_servers();
            self.tree.select_server(uid);
            self.focus = Focus::Tree;
            self.connect_server(uid);
        } else if let Some(node) = self.tree.by_uid(dialog.tag) {
            let connected = node.state != ConnState::Disconnected;
            node.conn = conn;
            if password.is_some() {
                node.password = password;
            }
            node.save_password = save;
            self.persist_servers();
            self.status = if connected {
                "Connection saved; changes apply on the next connect".to_owned()
            } else {
                "Connection saved".to_owned()
            };
        }
    }

    fn open_login(&mut self, uid: u64, error: Option<String>) {
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        let mut text = format!(
            "Connection: {}\nServer:     {}:{}\nDatabase:   {}\nUser name:  {}",
            node.conn.name, node.conn.host, node.conn.port, node.conn.database, node.conn.username
        );
        if let Some(e) = &error {
            text = format!("{e}\n\n{text}");
        }
        self.dialogs.push(
            Dialog::new(
                DialogKind::Login,
                "Login",
                60,
                vec![
                    Widget::label(&text),
                    Widget::label(""),
                    Widget::field("password", "&Password:", "", 11).masked(),
                    Widget::checkbox("save", "&Save password", false),
                    Widget::button("ok", "&OK", true),
                    Widget::button("cancel", "&Cancel", false),
                ],
            )
            .with_tag(uid),
        );
        self.status = "Password required".to_owned();
    }

    pub(super) fn apply_login(&mut self, dialog: &Dialog) {
        let uid = dialog.tag;
        let password = dialog.text("password").unwrap_or("").to_owned();
        let save = dialog.checked("save");
        if let Some(node) = self.tree.by_uid(uid) {
            node.password = Some(password);
            node.save_password |= save;
        }
        self.connect_server(uid);
    }

    pub(super) fn confirm_delete(&mut self) {
        let Some(node) = self.tree.selected_server() else {
            return;
        };
        let text = format!("Delete connection \"{}\"?", node.conn.name);
        let uid = node.uid;
        self.dialogs.push(
            Dialog::confirm(DialogKind::ConfirmDelete, "Delete connection", &text).with_tag(uid),
        );
    }

    /// Removes a server from the tree and the settings; its tabs and connections are
    /// closed before ([`Self::finish_bulk`]).
    pub(super) fn delete_server(&mut self, uid: u64) {
        if let Some(node) = self.tree.remove(uid) {
            self.status = format!("Connection \"{}\" deleted", node.conn.name);
        }
        self.persist_servers();
    }
}
