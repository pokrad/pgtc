//! File work in the background (D-072): opening and saving SQL files, reading and
//! writing `.pgdef` files and building the sync script run in a blocking task; the
//! result comes back as a [`FileEvent`] message, like database events.

use std::path::PathBuf;
use std::sync::Arc;

use pgc_core::places::Place;
use pgc_model::Database;

use super::closing::AfterSave;
use super::definition::JobKind;
use super::files::list_dir;
use super::{App, TextView};
use crate::dialog::Dialog;

/// Work for the background.
pub(super) enum FileJob {
    /// Read an SQL file (UTF-8, without its byte order mark).
    Open { path: PathBuf },
    /// Write `text`, the text of tab `tab` at editor revision `revision`.
    Save {
        tab: u64,
        path: PathBuf,
        text: String,
        revision: u64,
    },
    /// Read a `.pgdef` file to compare it with `database` on server `uid`.
    LoadDefinition {
        path: PathBuf,
        uid: u64,
        database: String,
    },
    /// Write the structure of `database` as a `.pgdef` file.
    SaveDefinition {
        path: PathBuf,
        database: String,
        model: Box<Database>,
    },
    /// The script that brings `live` (`database` on server `uid`) to `want`.
    SyncScript {
        uid: u64,
        database: String,
        want: Box<Database>,
        live: Box<Database>,
    },
    /// File dialogs (D-070): the listing of `dir`; files with the extension `ext` are
    /// highlighted.
    ListDir { seq: u64, dir: PathBuf, ext: String },
    /// File dialogs: the places; `current` is the folder of the tab's file.
    Places { seq: u64, current: Option<PathBuf> },
    /// File dialogs: what the chosen `path` is.
    Probe { seq: u64, path: PathBuf },
    /// Write the template of tab `tab` (D-074); `marks`: the state of the tab that
    /// `text` was made from. Without `overwrite` an existing file is left alone.
    SaveTemplate {
        tab: u64,
        path: PathBuf,
        text: String,
        marks: crate::template_tab::SaveMarks,
        overwrite: bool,
    },
    /// A change of the template manager in the templates folder `dir` (D-074).
    TemplateOp {
        dir: PathBuf,
        op: pgc_sql::devtemplate::LibraryOp,
    },
}

/// A listing for a file dialog.
pub enum Listing {
    /// (name, is directory, highlighted), as [`list_dir`] gives it.
    Folder(Vec<(String, bool, bool)>),
    Places(Vec<Place>),
}

/// What a path chosen in a file dialog is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathState {
    Folder,
    /// Not there, and its folder (given) is not there either.
    NoFolder(PathBuf),
    /// A file, or a name for a new one.
    File {
        exists: bool,
    },
}

/// The outcome of a [`FileJob`]; errors are texts for the user.
pub enum FileEvent {
    Opened {
        path: PathBuf,
        result: Result<String, String>,
    },
    Saved {
        tab: u64,
        path: PathBuf,
        revision: u64,
        result: Result<(), String>,
    },
    DefinitionLoaded {
        uid: u64,
        database: String,
        result: Result<Box<Database>, String>,
    },
    DefinitionSaved {
        path: PathBuf,
        database: String,
        result: Result<(), String>,
    },
    SyncScript {
        uid: u64,
        database: String,
        /// `None`: the structures already match.
        script: Option<String>,
    },
    /// The listing request `seq` of a file dialog asked for.
    Listed { seq: u64, listing: Listing },
    /// What the path chosen in a file dialog is.
    Probed {
        seq: u64,
        path: PathBuf,
        state: PathState,
    },
    /// The template of tab `tab` was written to `path`, or could not be; `exists`:
    /// nothing was written because the file is there already.
    TemplateSaved {
        tab: u64,
        path: PathBuf,
        marks: crate::template_tab::SaveMarks,
        exists: bool,
        result: Result<(), String>,
    },
    /// A change of the template manager was made, or could not be.
    TemplateOp {
        op: pgc_sql::devtemplate::LibraryOp,
        result: Result<(), String>,
    },
}

impl std::fmt::Debug for FileEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // the texts can be very long: only what happened
        f.write_str(match self {
            FileEvent::Opened { .. } => "Opened",
            FileEvent::Saved { .. } => "Saved",
            FileEvent::DefinitionLoaded { .. } => "DefinitionLoaded",
            FileEvent::DefinitionSaved { .. } => "DefinitionSaved",
            FileEvent::SyncScript { .. } => "SyncScript",
            FileEvent::Listed { .. } => "Listed",
            FileEvent::Probed { .. } => "Probed",
            FileEvent::TemplateSaved { .. } => "TemplateSaved",
            FileEvent::TemplateOp { .. } => "TemplateOp",
        })
    }
}

/// Where the background tasks report to (set by the event loop).
pub type FileSink = Arc<dyn Fn(FileEvent) + Send + Sync>;

impl FileJob {
    /// Does the work; runs outside the UI loop.
    fn run(self) -> FileEvent {
        match self {
            FileJob::Open { path } => {
                let result = match std::fs::read(&path) {
                    Ok(bytes) => String::from_utf8(bytes)
                        .map_err(|_| format!("{} is not a UTF-8 text file.", path.display()))
                        // a byte order mark is not part of the SQL (the server would
                        // reject it)
                        .map(|text| match text.strip_prefix('\u{feff}') {
                            Some(rest) => rest.to_owned(),
                            None => text,
                        }),
                    Err(e) => Err(format!("Cannot open {}:\n{e}", path.display())),
                };
                FileEvent::Opened { path, result }
            }
            FileJob::Save {
                tab,
                path,
                text,
                revision,
            } => {
                // a temporary file first: a crash or a full disk keeps the old file
                let result = pgc_core::fsutil::atomic_write(
                    &path,
                    text.as_bytes(),
                    pgc_core::fsutil::Access::Default,
                )
                .map_err(|e| format!("Cannot save {}:\n{e}", path.display()));
                FileEvent::Saved {
                    tab,
                    path,
                    revision,
                    result,
                }
            }
            FileJob::LoadDefinition {
                path,
                uid,
                database,
            } => {
                let result = pgc_model::file::load(&path)
                    .map(Box::new)
                    .map_err(|e| format!("Cannot load {}:\n{e}", path.display()));
                FileEvent::DefinitionLoaded {
                    uid,
                    database,
                    result,
                }
            }
            FileJob::SaveDefinition {
                path,
                database,
                model,
            } => {
                let result = pgc_model::file::save(&model, &path)
                    .map_err(|e| format!("Cannot write {}:\n{e}", path.display()));
                FileEvent::DefinitionSaved {
                    path,
                    database,
                    result,
                }
            }
            FileJob::SyncScript {
                uid,
                database,
                want,
                live,
            } => FileEvent::SyncScript {
                uid,
                database,
                script: pgc_model::sync::sync_script(&want, &live),
            },
            FileJob::ListDir { seq, dir, ext } => FileEvent::Listed {
                seq,
                listing: Listing::Folder(list_dir(&dir, &ext)),
            },
            FileJob::Places { seq, current } => {
                let mut places = pgc_core::places::places();
                if let Some(dir) = current.filter(|d| d.is_dir()) {
                    // after the home folder
                    let at = usize::from(places.first().is_some_and(|p| p.name == "~"));
                    places.insert(
                        at,
                        Place {
                            name: dir.display().to_string(),
                            path: dir,
                            note: "Current file".to_owned(),
                        },
                    );
                }
                FileEvent::Listed {
                    seq,
                    listing: Listing::Places(places),
                }
            }
            FileJob::Probe { seq, path } => {
                let state = if path.is_dir() {
                    PathState::Folder
                } else {
                    match path
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty() && !p.is_dir())
                    {
                        Some(parent) => PathState::NoFolder(parent.to_path_buf()),
                        None => PathState::File {
                            exists: path.exists(),
                        },
                    }
                };
                FileEvent::Probed { seq, path, state }
            }
            FileJob::SaveTemplate {
                tab,
                path,
                text,
                marks,
                overwrite,
            } => {
                let exists = !overwrite && path.exists();
                let result = if exists {
                    Ok(())
                } else {
                    path.parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| {
                            pgc_core::fsutil::atomic_write(
                                &path,
                                text.as_bytes(),
                                pgc_core::fsutil::Access::Default,
                            )
                        })
                        .map_err(|e| {
                            format!(
                                "Cannot save {}:
{e}",
                                path.display()
                            )
                        })
                };
                FileEvent::TemplateSaved {
                    tab,
                    path,
                    marks,
                    exists,
                    result,
                }
            }
            FileJob::TemplateOp { dir, op } => {
                let result = op.run(&dir);
                FileEvent::TemplateOp { op, result }
            }
        }
    }
}

impl App {
    /// Where the background file tasks report to (set by the event loop). Without it
    /// (tests) the work is done at once, in place.
    pub fn set_file_sink(&mut self, sink: FileSink) {
        self.file_sink = Some(sink);
    }

    /// Starts `job` in the background; its [`FileEvent`] follows.
    pub(super) fn start_file_job(&mut self, job: FileJob) {
        match self.file_sink.clone() {
            Some(sink) => {
                tokio::task::spawn_blocking(move || sink(job.run()));
            }
            None => {
                let event = job.run();
                self.on_file_event(event);
            }
        }
    }

    /// Open SQL file (D-041, D-072): the tab appears when the file is read.
    pub(super) fn open_path(&mut self, path: PathBuf) {
        self.status = format!("Opening {}…", path.display());
        self.start_file_job(FileJob::Open { path });
    }

    /// Saves the active tab to `path` (D-064, D-072).
    pub(super) fn save_to(&mut self, path: PathBuf) {
        let tab = self.tabs[self.active_tab].id;
        self.save_tab(tab, path);
    }

    /// Writes the text tab `tab` has now to `path`. While an earlier save of the tab
    /// is still being written, this one waits for it.
    fn save_tab(&mut self, tab: u64, path: PathBuf) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let t = &mut self.tabs[i];
        let Some(q) = t.query_mut() else {
            return;
        };
        if q.saving {
            q.save_next = Some(path);
            return;
        }
        q.saving = true;
        let editor = &t.view.editor;
        let (text, revision) = (editor.text(), editor.revision());
        self.status = format!("Saving {}…", path.display());
        self.start_file_job(FileJob::Save {
            tab,
            path,
            text,
            revision,
        });
    }

    /// A save is on its way: closing / quitting that waits for it goes on then.
    pub(super) fn saving(&self) -> bool {
        self.tabs
            .iter()
            .any(|t| t.query().is_some_and(|q| q.saving) || t.template().is_some_and(|t| t.saving))
    }

    pub(super) fn on_file_event(&mut self, event: FileEvent) {
        match event {
            FileEvent::Opened { path, result } => match result {
                Ok(text) => {
                    let mut view = TextView::new(false, self.tab_width());
                    view.load(text);
                    let id = self.add_query_tab(view);
                    if let Some(q) = self.tabs.last_mut().and_then(|t| t.query_mut()) {
                        q.file = Some(path.clone());
                    }
                    self.bind_query_tab(id);
                    self.status = format!("Opened {}", path.display());
                }
                Err(e) => {
                    self.status.clear();
                    self.error(&e);
                }
            },
            FileEvent::Saved {
                tab,
                path,
                revision,
                result,
            } => self.saved(tab, path, revision, result),
            FileEvent::DefinitionLoaded {
                uid,
                database,
                result,
            } => match result {
                Ok(model) => self.start_job(JobKind::Load(model), (uid, database)),
                Err(e) => {
                    self.status.clear();
                    self.error(&e);
                }
            },
            FileEvent::DefinitionSaved {
                path,
                database,
                result,
            } => match result {
                Ok(()) => {
                    self.status =
                        format!("Definition of {database} exported to {}", path.display());
                }
                Err(e) => {
                    self.status.clear();
                    self.error(&e);
                }
            },
            FileEvent::Listed { seq, listing } => self.file_listing_loaded(seq, listing),
            FileEvent::Probed { seq, path, state } => self.path_probed(seq, path, state),
            FileEvent::TemplateSaved {
                tab,
                path,
                marks,
                exists,
                result,
            } => self.template_saved(tab, path, marks, exists, result),
            FileEvent::TemplateOp { op, result } => self.library_op_done(&op, result),
            FileEvent::SyncScript {
                uid,
                database,
                script,
            } => match script {
                None => {
                    self.status.clear();
                    self.dialogs.push(Dialog::message(
                        "Load database definition",
                        &format!(
                            "The structure of {database} already matches the definition file."
                        ),
                        false,
                    ));
                }
                Some(script) => {
                    // review in a query tab of that database; never run automatically
                    let mut view = TextView::new(false, self.tab_width());
                    view.load(script);
                    let id = self.add_query_tab(view);
                    if let Some(q) = self.tab_index(id).and_then(|i| self.tabs[i].query_mut()) {
                        q.binding = Some((uid, database.clone()));
                    }
                    self.bind_query_tab(id);
                    self.status =
                        format!("Sync script for {database}: review it, then run it with F5");
                }
            },
        }
    }

    /// The text of tab `tab` at `revision` was written to `path`, or could not be.
    fn saved(&mut self, tab: u64, path: PathBuf, revision: u64, result: Result<(), String>) {
        let Some(i) = self.tab_index(tab) else {
            // the tab is gone (closed without waiting): nothing to mark
            return;
        };
        let next = {
            let t = &mut self.tabs[i];
            let unchanged = t.view.editor.revision() == revision;
            let Some(q) = t.query_mut() else {
                return;
            };
            q.saving = false;
            let next = q.save_next.take();
            if result.is_ok() {
                q.file = Some(path.clone());
                // edited while it was written: still modified (D-072)
                if unchanged {
                    t.view.editor.set_modified(false);
                }
            }
            next
        };
        if let Err(e) = result {
            // closing / quitting that waited for this save stops
            self.closing.after_save = None;
            self.closing.bulk = None;
            self.status.clear();
            self.error(&e);
            return;
        }
        if let Some(dir) = path.parent() {
            self.files.dir = dir.to_path_buf();
        }
        self.status = format!("Saved {}", path.display());
        // a save asked for while this one was written: the newer text
        if let Some(path) = next {
            self.save_tab(tab, path);
            return;
        }
        self.after_save(tab);
    }

    /// Tab `tab` was saved: closing it, or quitting, that waited for the save goes on.
    pub(super) fn after_save(&mut self, tab: u64) {
        match self.closing.after_save.take() {
            Some(AfterSave::Close(closing)) if closing == tab => {
                self.request_close_tab(tab, false);
            }
            Some(AfterSave::Bulk) => {
                if let Some((b, _)) = self.closing.bulk {
                    self.request_bulk(b);
                }
            }
            _ => {}
        }
    }
}
