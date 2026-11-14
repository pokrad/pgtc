//! SQL files (D-041): the open / save dialog, loading a file into a new query tab,
//! saving a query tab.

use std::path::{Path, PathBuf};

use super::closing::AfterSave;
use super::result_export::export_options;
use super::{App, TextView};
use crate::dialog::{Dialog, DialogKind, Payload, Widget};

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
}

impl App {
    /// The mc-style file dialog for the current folder (D-041).
    pub(super) fn file_dialog(&self, f: FileDialog<'_>) -> Dialog {
        let dir = &self.file_dir;
        let entries = list_dir(dir, f.ext);
        let labels = entries.iter().map(|e| e.0.clone()).collect();
        let marks = entries.iter().map(|e| e.2).collect();
        let mut widgets = f.above;
        widgets.push(Widget::label(&format!("Folder: {}", dir.display())));
        widgets.push(Widget::field("path", "&File name:", f.name, f.label_width));
        widgets.push(Widget::list("files", "", labels, 0, f.list_rows).highlighted(marks));
        widgets.extend(f.below);
        widgets.push(Widget::button("ok", "&OK", true));
        widgets.push(Widget::button("cancel", "&Cancel", false));
        let mut dialog = Dialog::new(f.kind, f.title, 70, widgets);
        if f.pick {
            dialog.focus_on("files");
        }
        dialog
    }

    /// The choice made in a file dialog: the selected entry of the listing when it has
    /// the focus, else the typed name. A folder becomes the dialogs' folder.
    pub(super) fn picked(&mut self, dialog: &Dialog) -> Picked {
        let dir = self.file_dir.clone();
        let chosen = if dialog.focused_id() == Some("files") {
            let entries = list_dir(&dir, "");
            dialog
                .selected("files")
                .and_then(|i| entries.get(i))
                .map(|e| e.0.trim_end_matches('/').to_owned())
                .unwrap_or_default()
        } else {
            dialog.text("path").unwrap_or("").trim().to_owned()
        };
        if chosen.is_empty() {
            return Picked::Nothing;
        }
        let path = if chosen == ".." {
            dir.parent().map(Path::to_path_buf).unwrap_or(dir)
        } else {
            dir.join(&chosen)
        };
        if path.is_dir() {
            self.file_dir = path;
            Picked::Folder
        } else {
            Picked::File(path)
        }
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
            (DialogKind::SaveFile, "Save SQL file")
        } else {
            (DialogKind::OpenFile, "Open SQL file")
        };
        let dialog = self.file_dialog(FileDialog {
            kind,
            title,
            ext: "sql",
            name: &name,
            label_width: 12,
            list_rows: 14,
            above: Vec::new(),
            below: Vec::new(),
            pick: !save,
        });
        self.dialogs.push(dialog);
    }

    pub(super) fn file_dialog_done(&mut self, dialog: Dialog) {
        let save = dialog.kind == DialogKind::SaveFile;
        let export = dialog.kind == DialogKind::ExportFile;
        let path = match self.picked(&dialog) {
            Picked::Nothing => {
                self.dialogs.push(dialog);
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
                self.dialogs.push(
                    Dialog::confirm(
                        DialogKind::ConfirmExportOverwrite,
                        "Export result",
                        &overwrite,
                    )
                    .with_payload(Payload::Export(path, options)),
                );
            } else {
                self.start_export(path, options);
            }
        } else if save {
            if path.exists() {
                self.dialogs.push(
                    Dialog::confirm(DialogKind::ConfirmOverwrite, "Save", &overwrite)
                        .with_payload(Payload::Path(path)),
                );
            } else {
                self.save_to(path);
            }
        } else {
            self.open_path(path);
        }
    }

    pub(super) fn open_path(&mut self, path: std::path::PathBuf) {
        let text = match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(_) => {
                    self.error(&format!("{} is not a UTF-8 text file.", path.display()));
                    return;
                }
            },
            Err(e) => {
                self.error(&format!("Cannot open {}:\n{e}", path.display()));
                return;
            }
        };
        let mut view = TextView::new(false, self.tab_width());
        view.load(text);
        let id = self.add_query_tab(view);
        if let Some(q) = self.tabs.last_mut().and_then(|t| t.query_mut()) {
            q.file = Some(path.clone());
        }
        self.bind_query_tab(id);
        self.status = format!("Opened {}", path.display());
    }

    pub(super) fn save_to(&mut self, path: std::path::PathBuf) {
        let text = self.tabs[self.active_tab].view.editor.text();
        match std::fs::write(&path, text) {
            Ok(()) => {
                self.tabs[self.active_tab].view.editor.set_modified(false);
                if let Some(q) = self.query_mut() {
                    q.file = Some(path.clone());
                }
                if let Some(dir) = path.parent() {
                    self.file_dir = dir.to_path_buf();
                }
                self.status = format!("Saved {}", path.display());
                let id = self.tabs[self.active_tab].id;
                match self.after_save.take() {
                    Some(AfterSave::Close(tab)) if tab == id => self.request_close_tab(id, false),
                    Some(AfterSave::Bulk) => {
                        if let Some((b, _)) = self.bulk {
                            self.request_bulk(b);
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => self.error(&format!("Cannot save {}:\n{e}", path.display())),
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
                dirs.push(format!("{name}/"));
            } else {
                files.push(name);
            }
        }
    }
    dirs.sort_by_key(|d| d.to_lowercase());
    files.sort_by_key(|f| f.to_lowercase());
    let mut out = Vec::new();
    if dir.parent().is_some() {
        out.push(("../".to_owned(), true, false));
    }
    out.extend(dirs.into_iter().map(|d| (d, true, false)));
    out.extend(files.into_iter().map(|f| {
        let marked = f.to_lowercase().ends_with(&format!(".{ext}"));
        (f, false, marked)
    }));
    out
}
