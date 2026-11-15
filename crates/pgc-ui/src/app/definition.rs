//! Database definition export and load (D-012, D-048): the file dialog and the job
//! that reads the structure on its own connection.

use pgc_db::{ConnId, DbEvent};

use super::file_jobs::FileJob;
use super::files::FileDialog;
use super::{App, Target};
use crate::dialog::{DialogKind, Widget};
use crate::tree::RowKind;

/// A definition export or load running on its own connection (D-048).
pub(super) struct StructureJob {
    /// Server uid and database name.
    uid: u64,
    database: String,
    kind: JobKind,
    pub(super) handle: pgc_db::ConnectionHandle,
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
        let (server, database) = target;
        let label = Widget::label(&format!("Database: {database}"));
        let (kind, title) = if save {
            (
                DialogKind::SaveDefinition { server, database },
                "Export database definition",
            )
        } else {
            (
                DialogKind::OpenDefinition { server, database },
                "Load database definition",
            )
        };
        self.open_listing_dialog(FileDialog {
            kind,
            title,
            ext: "pgdef",
            name,
            label_width: 15,
            list_rows: 14,
            above: vec![label],
            below: Vec::new(),
            pick: !save,
        });
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

    /// The live structure is read: it is written to the file, or compared with the
    /// definition — both in the background (D-072).
    fn job_done(&mut self, job: StructureJob, live: pgc_model::Database) {
        match job.kind {
            JobKind::Export(path) => {
                self.status = format!("Writing {}…", path.display());
                self.start_file_job(FileJob::SaveDefinition {
                    path,
                    database: job.database,
                    model: Box::new(live),
                });
            }
            JobKind::Load(want) => {
                self.status = format!("Comparing the definition with {}…", job.database);
                self.start_file_job(FileJob::SyncScript {
                    uid: job.uid,
                    database: job.database,
                    want,
                    live: Box::new(live),
                });
            }
        }
    }
}
