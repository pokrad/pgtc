//! SQL files (D-041): the open / save dialog, loading a file into a new query tab,
//! saving a query tab.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

use pgc_core::places::Place;

use super::App;
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

/// What was chosen in a file dialog.
pub(super) enum Picked {
    Nothing,
    /// A folder: it is the dialogs' folder now.
    Folder,
    File(PathBuf),
    /// A path in a folder that does not exist: the message to show (D-070).
    Invalid(String),
}

impl App {
    /// The mc-style file dialog for the current folder, or for the places (D-041,
    /// D-070).
    pub(super) fn file_dialog(&self, f: FileDialog<'_>) -> Dialog {
        let (folder, labels, marks) = match &self.files.places {
            Some(places) => {
                let mut labels = vec![format!("..{MAIN_SEPARATOR}")];
                labels.extend(place_labels(places));
                let marks = vec![false; labels.len()];
                ("Places".to_owned(), labels, marks)
            }
            None => {
                let entries = list_dir(&self.files.dir, f.ext);
                let labels = entries.iter().map(|e| e.0.clone()).collect();
                let marks = entries.iter().map(|e| e.2).collect();
                (self.files.dir.display().to_string(), labels, marks)
            }
        };
        let mut widgets = f.above;
        widgets.push(Widget::label(&format!("Folder: {folder}")));
        widgets.push(Widget::field(
            "path",
            "&Name or path:",
            f.name,
            f.label_width,
        ));
        widgets.push(Widget::list("files", "", labels, 0, f.list_rows).highlighted(marks));
        widgets.extend(f.below);
        widgets.push(Widget::button(Button::Ok, "&OK", true));
        widgets.push(Widget::button(Button::Places, "&Places", false));
        widgets.push(Widget::button(Button::Cancel, "&Cancel", false));
        let mut dialog = Dialog::new(f.kind, f.title, 70, widgets);
        if f.pick || self.files.places.is_some() {
            dialog.focus_on("files");
        }
        dialog
    }

    /// The listing shows the places (D-070): home, the folder of the tab's file, then
    /// the drives / volumes.
    fn show_places(&mut self) {
        let mut places = pgc_core::places::places();
        let file_dir = self
            .query()
            .and_then(|q| q.file.as_ref())
            .and_then(|f| f.parent())
            .filter(|d| d.is_dir())
            .map(Path::to_path_buf);
        if let Some(dir) = file_dir {
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
        self.files.places = Some(places);
    }

    /// The choice made in a file dialog with `button`: the selected entry of the
    /// listing when it has the focus, else the typed name or path. A folder becomes
    /// the dialogs' folder (D-070).
    pub(super) fn picked(&mut self, dialog: &Dialog, button: Button) -> Picked {
        if button == Button::Places {
            self.show_places();
            return Picked::Folder;
        }
        let dir = self.files.dir.clone();
        let path = if dialog.focused_id() == Some("files") {
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
            // the row as the user sees it: the folder may have changed since it was
            // listed
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
            dir.join(name)
        } else {
            let text = dialog.text("path").unwrap_or("").trim();
            if text.is_empty() {
                return Picked::Nothing;
            }
            resolve(&dir, text, pgc_core::paths::home_dir().as_deref())
        };
        if path.is_dir() {
            self.files.dir = path;
            self.files.places = None;
            return Picked::Folder;
        }
        if let Some(parent) = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty() && !p.is_dir())
        {
            return Picked::Invalid(format!("The folder {} does not exist.", parent.display()));
        }
        self.files.places = None;
        Picked::File(path)
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
            .filter(|d| d.is_dir())
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
        let dialog = self.file_dialog(FileDialog {
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
        self.dialogs.push(dialog);
    }

    pub(super) fn file_dialog_done(&mut self, dialog: Dialog, button: Button) {
        let save = dialog.kind == DialogKind::SaveFile;
        let export = dialog.kind == DialogKind::ExportFile;
        let path = match self.picked(&dialog, button) {
            Picked::Nothing => {
                self.dialogs.push(dialog);
                return;
            }
            Picked::Invalid(text) => {
                self.dialogs.push(dialog);
                self.error(&text);
                return;
            }
            Picked::Folder => {
                if export {
                    let name = dialog.text("path").unwrap_or("").to_owned();
                    let options = export_options(&dialog);
                    self.open_export_dialog(Some(name), Some(options));
                } else {
                    self.open_file_dialog(save);
                }
                return;
            }
            Picked::File(path) => path,
        };
        let overwrite = format!("{} exists. Overwrite it?", path.display());
        if export {
            let options = export_options(&dialog);
            if path.exists() {
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmExportOverwrite { path, options },
                    "Export result",
                    &overwrite,
                ));
            } else {
                self.start_export(path, options);
            }
        } else if save {
            if path.exists() {
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmOverwrite { path },
                    "Save",
                    &overwrite,
                ));
            } else {
                self.save_to(path);
            }
        } else {
            self.open_path(path);
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
