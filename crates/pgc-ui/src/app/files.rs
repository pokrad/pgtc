//! SQL files (D-041): the open / save dialog, loading a file into a new query tab,
//! saving a query tab.

use std::path::Path;

use super::closing::AfterSave;
use super::result_export::export_options;
use super::{App, TabKind, TextView};
use crate::dialog::{Dialog, DialogKind, Widget};

impl App {
    pub(super) fn open_file_dialog(&mut self, save: bool) {
        let dir = self.file_dir.clone();
        let initial = if save {
            self.query()
                .and_then(|q| q.file.as_ref())
                .and_then(|f| f.file_name())
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "query.sql".to_owned())
        } else {
            String::new()
        };
        let entries = list_dir(&dir, "sql");
        let labels: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let marks: Vec<bool> = entries.iter().map(|e| e.2).collect();
        let (kind, title) = if save {
            (DialogKind::SaveFile, "Save SQL file")
        } else {
            (DialogKind::OpenFile, "Open SQL file")
        };
        let mut dialog = Dialog::new(
            kind,
            title,
            70,
            vec![
                Widget::label(&format!("Folder: {}", dir.display())),
                Widget::field("path", "&File name:", &initial, 12),
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

    pub(super) fn file_dialog_done(&mut self, dialog: Dialog) {
        let save = dialog.kind == DialogKind::SaveFile;
        let export = dialog.kind == DialogKind::ExportFile;
        let dir = self.file_dir.clone();
        let chosen = if dialog.focused_id() == Some("files") {
            let entries = list_dir(&dir, "sql");
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
            if export {
                let name = dialog.text("path").unwrap_or("").to_owned();
                let options = export_options(&dialog);
                self.open_export_dialog(Some(name), Some(options));
            } else {
                self.open_file_dialog(save);
            }
            return;
        }
        if export {
            let options = export_options(&dialog);
            if path.exists() {
                self.pending_export = Some((path.clone(), options));
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmExportOverwrite,
                    "Export result",
                    &format!("{} exists. Overwrite it?", path.display()),
                ));
            } else {
                self.start_export(path, options);
            }
        } else if save {
            if path.exists() {
                self.pending_save = Some(path.clone());
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmOverwrite,
                    "Save",
                    &format!("{} exists. Overwrite it?", path.display()),
                ));
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
        self.next_query += 1;
        let id = self.add_tab(TabKind::Query(self.next_query), view);
        if let Some(q) = self.tabs.last_mut().and_then(|t| t.query.as_mut()) {
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
