//! The file dialogs (D-041, D-070): the folder's listing with a name input, for SQL
//! files, result exports and database definitions. Nothing here touches the file
//! system: the listing, the places and the check of a chosen path are read in the
//! background (D-072).

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

use pgc_core::places::Place;

use super::App;
use super::definition::JobKind;
use super::file_jobs::{FileJob, Listing, PathState};
use super::result_export::export_options;
use crate::dialog::{Button, Dialog, DialogKind, Widget};

/// Layout of a file dialog (D-041): the folder's listing with a name input.
pub(super) struct FileDialog<'a> {
    pub kind: DialogKind,
    pub title: &'a str,
    /// Files with this extension are highlighted.
    pub ext: &'a str,
    /// Text of the name input.
    pub name: &'a str,
    pub label_width: u16,
    pub list_rows: u16,
    /// Widgets above the folder line and below the listing.
    pub above: Vec<Widget>,
    pub below: Vec<Widget>,
    /// The listing has the focus (dialogs that open a file).
    pub pick: bool,
}

/// What was chosen in a file dialog (nothing is looked up yet).
enum Picked {
    Nothing,
    /// The dialogs' folder (or the places) changed: the dialog is shown again.
    Folder,
    /// A path: a folder to go to or a file — the background finds out.
    Path(PathBuf),
}

/// The only row of a listing that is still being read.
const READING: &str = "Reading…";

/// The dialog is one of the file dialogs.
pub(super) fn is_file_dialog(kind: &DialogKind) -> bool {
    matches!(
        kind,
        DialogKind::OpenFile
            | DialogKind::SaveFile
            | DialogKind::ExportFile
            | DialogKind::SaveDefinition { .. }
            | DialogKind::OpenDefinition { .. }
    )
}

impl App {
    /// Opens the mc-style file dialog for the current folder, or for the places
    /// (D-041, D-070). It shows at once; its listing follows from the background
    /// (D-072).
    pub(super) fn open_listing_dialog(&mut self, f: FileDialog<'_>) {
        let places = self.files.places.is_some();
        let folder = if places {
            "Places".to_owned()
        } else {
            self.files.dir.display().to_string()
        };
        let mut widgets = f.above;
        widgets.push(Widget::label(&format!("Folder: {folder}")));
        widgets.push(Widget::field(
            "path",
            "&Name or path:",
            f.name,
            f.label_width,
        ));
        widgets.push(Widget::list(
            "files",
            "",
            vec![READING.to_owned()],
            0,
            f.list_rows,
        ));
        widgets.extend(f.below);
        widgets.push(Widget::button(Button::Ok, "&OK", true));
        widgets.push(Widget::button(Button::Places, "&Places", false));
        widgets.push(Widget::button(Button::Cancel, "&Cancel", false));
        let mut dialog = Dialog::new(f.kind, f.title, 70, widgets);
        if f.pick || places {
            dialog.focus_on("files");
        }
        self.dialogs.push(dialog);

        self.files.seq += 1;
        let seq = self.files.seq;
        self.files.listing = Some(seq);
        self.files.probing = None;
        self.files.list_rows = f.list_rows;
        let job = if places {
            FileJob::Places {
                seq,
                current: self
                    .query()
                    .and_then(|q| q.file.as_ref())
                    .and_then(|f| f.parent())
                    .map(Path::to_path_buf),
            }
        } else {
            FileJob::ListDir {
                seq,
                dir: self.files.dir.clone(),
                ext: f.ext.to_owned(),
            }
        };
        self.start_file_job(job);
    }

    /// The listing request `seq` asked for is read: it fills the open file dialog.
    pub(super) fn file_listing_loaded(&mut self, seq: u64, listing: Listing) {
        if self.files.listing != Some(seq) {
            return;
        }
        self.files.listing = None;
        let (labels, marks) = match listing {
            Listing::Places(places) => {
                let mut labels = vec![format!("..{MAIN_SEPARATOR}")];
                labels.extend(place_labels(&places));
                let marks = vec![false; labels.len()];
                self.files.places = Some(places);
                (labels, marks)
            }
            Listing::Folder(entries) => (
                entries.iter().map(|e| e.0.clone()).collect(),
                entries.iter().map(|e| e.2).collect(),
            ),
        };
        let rows = self.files.list_rows;
        if let Some(d) = self
            .dialogs
            .iter_mut()
            .rev()
            .find(|d| is_file_dialog(&d.kind))
        {
            // the height the dialog was made with: a resize while it showed the one
            // row *Reading…* has fitted it to that row
            d.set_list_rows("files", rows);
            d.set_list("files", labels, marks, 0);
        }
    }

    /// A file dialog closed without a choice: the next one shows the folder again
    /// (D-070), and what was being read for this one is not waited for.
    pub(super) fn file_dialog_closed(&mut self) {
        self.files.places = None;
        self.files.listing = None;
        self.files.probing = None;
    }

    /// The listing shows the places (D-070): home, the folder of the tab's file, then
    /// the drives / volumes. They are read when the dialog opens again.
    fn show_places(&mut self) {
        self.files.places = Some(Vec::new());
    }

    /// The choice made in a file dialog with `button`: the selected entry of the
    /// listing when it has the focus, else the typed name or path (D-070).
    fn picked(&mut self, dialog: &Dialog, button: Button) -> Picked {
        if button == Button::Places {
            self.show_places();
            return Picked::Folder;
        }
        let dir = self.files.dir.clone();
        // the list or the name field, whichever was used last (OK takes the focus
        // when it is clicked)
        if dialog.field_id() == Some("files") {
            // still being read: there is nothing to choose yet
            if self.files.listing.is_some() {
                return Picked::Nothing;
            }
            let Some(i) = dialog.selected("files") else {
                return Picked::Nothing;
            };
            if let Some(places) = self.files.places.take() {
                // `..` goes back to the folder
                if let Some(place) = i.checked_sub(1).and_then(|i| places.get(i)) {
                    self.files.dir = place.path.clone();
                }
                return Picked::Folder;
            }
            let Some(name) = dialog
                .selected_item("files")
                .map(|n| n.trim_end_matches(MAIN_SEPARATOR))
            else {
                return Picked::Nothing;
            };
            if name == ".." {
                match dir.parent() {
                    Some(up) => self.files.dir = up.to_path_buf(),
                    // the top of a drive: the places
                    None => self.show_places(),
                }
                return Picked::Folder;
            }
            Picked::Path(dir.join(name))
        } else {
            let text = dialog.text("path").unwrap_or("").trim();
            if text.is_empty() {
                return Picked::Nothing;
            }
            Picked::Path(resolve(&dir, text, pgc_core::paths::home_dir().as_deref()))
        }
    }

    /// Save (D-064): to the tab's file without a dialog, else Save as.
    pub(super) fn save_file(&mut self) {
        match self.query().and_then(|q| q.file.clone()) {
            Some(path) => self.save_to(path),
            None => self.save_file_as(),
        }
    }

    /// Save as (D-064): always the dialog, in the folder of the tab's file.
    pub(super) fn save_file_as(&mut self) {
        if let Some(dir) = self
            .query()
            .and_then(|q| q.file.as_ref())
            .and_then(|f| f.parent())
        {
            self.files.dir = dir.to_path_buf();
        }
        self.open_file_dialog(true);
    }

    pub(super) fn open_file_dialog(&mut self, save: bool) {
        let name = if save {
            self.query()
                .and_then(|q| q.file.as_ref())
                .and_then(|f| f.file_name())
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "query.sql".to_owned())
        } else {
            String::new()
        };
        let (kind, title) = if save {
            (DialogKind::SaveFile, "Save SQL file as")
        } else {
            (DialogKind::OpenFile, "Open SQL file")
        };
        self.open_listing_dialog(FileDialog {
            kind,
            title,
            ext: "sql",
            name: &name,
            label_width: 15,
            list_rows: 14,
            above: Vec::new(),
            below: Vec::new(),
            pick: !save,
        });
    }

    /// OK or Places in a file dialog. A chosen path is checked in the background
    /// (D-072): the dialog stays until [`Self::path_probed`] has the answer.
    pub(super) fn file_dialog_done(&mut self, dialog: Dialog, button: Button) {
        match self.picked(&dialog, button) {
            Picked::Nothing => self.dialogs.push(dialog),
            Picked::Folder => self.reopen_file_dialog(&dialog),
            Picked::Path(path) => {
                self.dialogs.push(dialog);
                self.files.seq += 1;
                let seq = self.files.seq;
                self.files.probing = Some(seq);
                self.start_file_job(FileJob::Probe { seq, path });
            }
        }
    }

    /// What the path chosen in the open file dialog is: a folder becomes the dialogs'
    /// folder, a path in a folder that does not exist is an error (D-070), anything
    /// else is the file.
    pub(super) fn path_probed(&mut self, seq: u64, path: PathBuf, state: PathState) {
        if self.files.probing != Some(seq) {
            return;
        }
        self.files.probing = None;
        // the dialog it was chosen in must still be the one in front
        if !self.dialogs.last().is_some_and(|d| is_file_dialog(&d.kind)) {
            return;
        }
        let Some(dialog) = self.dialogs.pop() else {
            return;
        };
        match state {
            PathState::Folder => {
                self.files.dir = path;
                self.files.places = None;
                self.reopen_file_dialog(&dialog);
            }
            PathState::NoFolder(parent) => {
                self.dialogs.push(dialog);
                self.error(&format!("The folder {} does not exist.", parent.display()));
            }
            PathState::File { exists } => {
                self.files.places = None;
                self.files.listing = None;
                self.file_chosen(dialog, path, exists);
            }
        }
    }

    /// The same dialog again for the folder (or the places) the dialogs show now,
    /// keeping what the user had entered.
    fn reopen_file_dialog(&mut self, dialog: &Dialog) {
        let name = dialog.text("path").unwrap_or("").to_owned();
        match &dialog.kind {
            DialogKind::ExportFile => {
                let options = export_options(dialog);
                self.open_export_dialog(Some(name), Some(options));
            }
            DialogKind::SaveFile => self.open_file_dialog(true),
            DialogKind::OpenFile => self.open_file_dialog(false),
            DialogKind::SaveDefinition { server, database } => {
                self.open_definition_dialog(true, &name, (*server, database.clone()));
            }
            DialogKind::OpenDefinition { server, database } => {
                self.open_definition_dialog(false, "", (*server, database.clone()));
            }
            _ => {}
        }
    }

    /// A file is chosen in `dialog`; `exists`: it is there already (writing asks
    /// before it overwrites).
    fn file_chosen(&mut self, dialog: Dialog, path: PathBuf, exists: bool) {
        let overwrite = format!("{} exists. Overwrite it?", path.display());
        match dialog.kind.clone() {
            DialogKind::ExportFile => {
                let options = export_options(&dialog);
                if exists {
                    self.dialogs.push(Dialog::confirm(
                        DialogKind::ConfirmExportOverwrite { path, options },
                        "Export result",
                        &overwrite,
                    ));
                } else {
                    self.start_export(path, options);
                }
            }
            DialogKind::SaveFile if exists => {
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmOverwrite { path },
                    "Save",
                    &overwrite,
                ));
            }
            DialogKind::SaveFile => self.save_to(path),
            DialogKind::OpenFile => self.open_path(path),
            DialogKind::SaveDefinition { server, database } if exists => {
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmDefinitionOverwrite {
                        server,
                        database,
                        path,
                    },
                    "Export database definition",
                    &overwrite,
                ));
            }
            DialogKind::SaveDefinition { server, database } => {
                self.start_job(JobKind::Export(path), (server, database));
            }
            DialogKind::OpenDefinition { server, database } => {
                self.status = format!("Reading {}…", path.display());
                self.start_file_job(FileJob::LoadDefinition {
                    path,
                    uid: server,
                    database,
                });
            }
            _ => {}
        }
    }
}

/// Directory listing for the file dialogs: (name, is directory, highlighted); files
/// with the extension `ext` are highlighted.
pub(super) fn list_dir(dir: &Path, ext: &str) -> Vec<(String, bool, bool)> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                dirs.push(format!("{name}{MAIN_SEPARATOR}"));
            } else {
                files.push(name);
            }
        }
    }
    dirs.sort_by_key(|d| d.to_lowercase());
    files.sort_by_key(|f| f.to_lowercase());
    let mut out = Vec::new();
    // on Windows also at the top of a drive: `..` leads to the places (D-070)
    if dir.parent().is_some() || cfg!(windows) {
        out.push((format!("..{MAIN_SEPARATOR}"), true, false));
    }
    out.extend(dirs.into_iter().map(|d| (d, true, false)));
    out.extend(files.into_iter().map(|f| {
        let marked = f.to_lowercase().ends_with(&format!(".{ext}"));
        (f, false, marked)
    }));
    out
}

/// Listing rows for the places: the name, then its note in a column (D-070). Long
/// names (the folder of the tab's file) do not widen the column.
fn place_labels(places: &[Place]) -> Vec<String> {
    const MAX_COLUMN: usize = 24;
    // the listing of the 70-column file dialog
    const PLACE_ROW: usize = 60;
    let width = places
        .iter()
        .map(|p| p.name.chars().count())
        .filter(|&w| w <= MAX_COLUMN)
        .max()
        .unwrap_or(0);
    places
        .iter()
        .map(|p| {
            if p.note.is_empty() {
                p.name.clone()
            } else {
                // a long name is shortened from the left so the note stays visible
                let room = PLACE_ROW.saturating_sub(p.note.chars().count() + 2);
                let name = crate::view::shorten_left(&p.name, room);
                format!("{name:<width$}  {}", p.note)
            }
        })
        .collect()
}

/// The path typed in a file dialog, seen from `dir` (D-070): absolute or relative,
/// `~` / `~/…` for `home`, a drive letter alone (`D:`) for the root of the drive on
/// Windows; `.` and `..` resolved.
pub(super) fn resolve(dir: &Path, input: &str, home: Option<&Path>) -> PathBuf {
    let rest = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
        .or((input == "~").then_some(""));
    let path = match (rest, home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => {
            let b = input.as_bytes();
            if cfg!(windows) && b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                PathBuf::from(format!("{input}\\"))
            } else {
                dir.join(input)
            }
        }
    };
    normalize(&path)
}

/// `.` and `..` removed by their meaning (no file system access); `..` stops at the
/// root.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}
