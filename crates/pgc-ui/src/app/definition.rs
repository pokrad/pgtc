//! Database definition export and load (D-012, D-048): the file dialog and the job
//! that reads the structure on its own connection.

use pgc_db::{ConnId, DbEvent};

use super::files::{FileDialog, Picked};
use super::{App, Target, TextView};
use crate::dialog::{Dialog, DialogKind, Payload, Widget};
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

    /// The file dialog of a definition export (`save`) or load for `target`: server
    /// uid and database (D-048).
    pub(super) fn open_definition_dialog(&mut self, save: bool, name: &str, target: (u64, String)) {
        let (kind, title) = if save {
            (DialogKind::SaveDefinition, "Export database definition")
        } else {
            (DialogKind::OpenDefinition, "Load database definition")
        };
        let (uid, database) = target;
        let dialog = self
            .file_dialog(FileDialog {
                kind,
                title,
                ext: "pgdef",
                name,
                label_width: 12,
                list_rows: 14,
                above: vec![Widget::label(&format!("Database: {database}"))],
                below: Vec::new(),
                pick: !save,
            })
            .with_payload(Payload::Database {
                uid,
                database,
                path: None,
            });
        self.dialogs.push(dialog);
    }

    pub(super) fn definition_dialog_done(&mut self, dialog: Dialog) {
        let save = dialog.kind == DialogKind::SaveDefinition;
        let Payload::Database { uid, database, .. } = dialog.payload.clone() else {
            return;
        };
        let path = match self.picked(&dialog) {
            Picked::Nothing => {
                self.dialogs.push(dialog);
                return;
            }
            Picked::Folder => {
                let name = dialog.text("path").unwrap_or("");
                self.open_definition_dialog(save, if save { name } else { "" }, (uid, database));
                return;
            }
            Picked::File(path) => path,
        };
        if save {
            if path.exists() {
                let text = format!("{} exists. Overwrite it?", path.display());
                self.dialogs.push(
                    Dialog::confirm(
                        DialogKind::ConfirmDefinitionOverwrite,
                        "Export database definition",
                        &text,
                    )
                    .with_payload(Payload::Database {
                        uid,
                        database,
                        path: Some(path),
                    }),
                );
            } else {
                self.start_job(JobKind::Export(path), (uid, database));
            }
        } else {
            match pgc_model::file::load(&path) {
                Ok(model) => self.start_job(JobKind::Load(Box::new(model)), (uid, database)),
                Err(e) => self.error(&format!("Cannot load {}:\n{e}", path.display())),
            }
        }
    }

    /// Opens a connection to the database for the job; the structure is read once it
    /// is connected.
    pub(super) fn start_job(&mut self, kind: JobKind, (uid, database): (u64, String)) {
        let Some(si) = self.tree.index_of(uid) else {
            return;
        };
        let handle = self.open_connection(si, &database, Target::Structure);
        let id = handle.id;
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
                    let id = self.add_query_tab(view);
                    if let Some(q) = self.tab_index(id).and_then(|i| self.tabs[i].query_mut()) {
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
