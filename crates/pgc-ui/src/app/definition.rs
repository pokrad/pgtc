//! Database definition export and load (D-012, D-048): the file dialog and the job
//! that reads the structure on its own connection.

use std::path::Path;

use pgc_db::{ConnId, ConnectParams, DbEvent};

use super::files::list_dir;
use super::{App, TabKind, Target, TextView};
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::tree::RowKind;

/// A definition export or load running on its own connection (D-048).
pub(super) struct StructureJob {
    /// Server uid and database name.
    uid: u64,
    database: String,
    kind: JobKind,
    handle: pgc_db::ConnectionHandle,
}

pub(super) enum JobKind {
    Export(std::path::PathBuf),
    /// The definition read from the file, compared with the live database.
    Load(Box<pgc_model::Database>),
}

impl App {
    /// The database selected in the tree: (server uid, database name).
    pub(super) fn selected_database(&self) -> Option<(u64, String)> {
        let row = self.tree.selected()?;
        let RowKind::Node(nid) = row.kind else {
            return None;
        };
        let node = self.tree.node(nid).filter(|n| n.is_database())?;
        Some((self.tree.servers[row.server].uid, node.name.clone()))
    }

    pub(super) fn open_definition_dialog(&mut self, save: bool, initial: &str) {
        let dir = self.file_dir.clone();
        let entries = list_dir(&dir, "pgdef");
        let labels: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let marks: Vec<bool> = entries.iter().map(|e| e.2).collect();
        let database = self
            .definition_target
            .as_ref()
            .map(|t| t.1.clone())
            .unwrap_or_default();
        let (kind, title) = if save {
            (DialogKind::SaveDefinition, "Export database definition")
        } else {
            (DialogKind::OpenDefinition, "Load database definition")
        };
        let mut dialog = Dialog::new(
            kind,
            title,
            70,
            vec![
                Widget::label(&format!("Database: {database}")),
                Widget::label(&format!("Folder: {}", dir.display())),
                Widget::field("path", "&File name:", initial, 12),
                Widget::list("files", "", labels, 0, 14).highlighted(marks),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        );
        if !save {
            dialog.focus_on("files");
        }
        self.dialogs.push(dialog);
    }

    pub(super) fn definition_dialog_done(&mut self, dialog: Dialog) {
        let save = dialog.kind == DialogKind::SaveDefinition;
        let dir = self.file_dir.clone();
        let chosen = if dialog.focused_id() == Some("files") {
            let entries = list_dir(&dir, "pgdef");
            dialog
                .selected("files")
                .and_then(|i| entries.get(i))
                .map(|e| e.0.trim_end_matches('/').to_owned())
                .unwrap_or_default()
        } else {
            dialog.text("path").unwrap_or("").trim().to_owned()
        };
        if chosen.is_empty() {
            self.dialogs.push(dialog);
            return;
        }
        let path = if chosen == ".." {
            dir.parent().map(Path::to_path_buf).unwrap_or(dir)
        } else {
            dir.join(&chosen)
        };
        if path.is_dir() {
            self.file_dir = path;
            let initial = dialog.text("path").unwrap_or("").to_owned();
            self.open_definition_dialog(save, if save { &initial } else { "" });
            return;
        }
        if save {
            if path.exists() {
                self.pending_definition = Some(path.clone());
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmDefinitionOverwrite,
                    "Export database definition",
                    &format!("{} exists. Overwrite it?", path.display()),
                ));
            } else {
                self.start_job(JobKind::Export(path));
            }
        } else {
            match pgc_model::file::load(&path) {
                Ok(model) => self.start_job(JobKind::Load(Box::new(model))),
                Err(e) => self.error(&format!("Cannot load {}:\n{e}", path.display())),
            }
        }
    }

    /// Opens a connection to the target database for the job; the structure is read
    /// once it is connected.
    pub(super) fn start_job(&mut self, kind: JobKind) {
        let Some((uid, database)) = self.definition_target.take() else {
            return;
        };
        let Some(si) = self.tree.index_of(uid) else {
            return;
        };
        let timeout = self.timeout();
        let server = &self.tree.servers[si];
        let password = server
            .password
            .clone()
            .or_else(|| Self::pgpass_password(&server.conn, &database));
        let params = ConnectParams::new(&server.conn, &database, password, timeout);
        self.next_conn += 1;
        let id = self.next_conn;
        let handle = pgc_db::spawn(id, params, self.sink.clone());
        self.connections.insert(id, Target::Structure);
        self.status = format!("Reading the structure of {database}…");
        self.jobs.insert(
            id,
            StructureJob {
                uid,
                database,
                kind,
                handle,
            },
        );
    }

    fn end_job(&mut self, id: ConnId) -> Option<StructureJob> {
        self.connections.remove(&id);
        let job = self.jobs.remove(&id)?;
        job.handle.close();
        Some(job)
    }

    pub(super) fn on_job_event(&mut self, id: ConnId, event: DbEvent) {
        match event {
            DbEvent::Connected { version, .. } => {
                let Some(job) = self.jobs.get(&id) else {
                    return;
                };
                match pgc_db::model::ModelQueries::render(&self.sql, version) {
                    Ok(q) => job.handle.read_model(0, q),
                    Err(e) => {
                        self.end_job(id);
                        self.error(&format!("Cannot read the structure:\n{e}"));
                    }
                }
            }
            DbEvent::ConnectFailed { error, .. } => {
                if let Some(job) = self.end_job(id) {
                    self.status.clear();
                    self.error(&format!("Cannot connect to {}:\n{error}", job.database));
                }
            }
            DbEvent::Closed { error, .. } => {
                if let Some(job) = self.end_job(id) {
                    self.status.clear();
                    let why =
                        error.map_or("the connection was closed".to_owned(), |e| e.to_string());
                    self.error(&format!(
                        "Reading the structure of {} failed:\n{why}",
                        job.database
                    ));
                }
            }
            DbEvent::Model { result, .. } => {
                let Some(job) = self.end_job(id) else {
                    return;
                };
                self.status.clear();
                match result {
                    Ok(live) => self.job_done(job, *live),
                    Err(e) => self.error(&format!(
                        "Cannot read the structure of {}:\n{e}",
                        job.database
                    )),
                }
            }
            _ => {}
        }
    }

    fn job_done(&mut self, job: StructureJob, live: pgc_model::Database) {
        match job.kind {
            JobKind::Export(path) => match pgc_model::file::save(&live, &path) {
                Ok(()) => {
                    self.status = format!(
                        "Definition of {} exported to {}",
                        job.database,
                        path.display()
                    );
                }
                Err(e) => self.error(&format!("Cannot write {}:\n{e}", path.display())),
            },
            JobKind::Load(want) => match pgc_model::sync::sync_script(&want, &live) {
                None => self.dialogs.push(Dialog::message(
                    "Load database definition",
                    &format!(
                        "The structure of {} already matches the definition file.",
                        job.database
                    ),
                    false,
                )),
                Some(script) => {
                    // review in a query tab of that database; never run automatically
                    let mut view = TextView::new(false, self.tab_width());
                    view.load(script);
                    self.next_query += 1;
                    let id = self.add_tab(TabKind::Query(self.next_query), view);
                    if let Some(q) = self.tab_index(id).and_then(|i| self.tabs[i].query.as_mut()) {
                        q.binding = Some((job.uid, job.database.clone()));
                    }
                    self.bind_query_tab(id);
                    self.status = format!(
                        "Sync script for {}: review it, then run it with F5",
                        job.database
                    );
                }
            },
        }
    }
}
