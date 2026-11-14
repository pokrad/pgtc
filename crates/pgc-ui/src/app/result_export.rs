//! Result export (D-021, D-042): the export dialog, and feeding the writer thread
//! from the grid and from the paused stream.

use std::path::Path;

use pgc_core::settings::{CsvSeparator, ExportFormat};
use pgc_db::QueryEvent;

use super::App;
use super::files::list_dir;
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::export::{ExportEvent, ExportOptions, ExportSink, ToWriter};
use crate::query_tab::{ExportRun, QueryState};

impl App {
    /// Where the writer threads report to (set by the event loop).
    pub fn set_export_sink(&mut self, sink: ExportSink) {
        self.export_sink = Some(sink);
    }

    /// The export dialog; `name` / `options` keep the user's input when the dialog is
    /// reopened for another folder.
    pub(super) fn open_export_dialog(
        &mut self,
        name: Option<String>,
        options: Option<ExportOptions>,
    ) {
        let Some(q) = self.query() else {
            return;
        };
        let set = q.active_result + 1;
        let o = options.unwrap_or(ExportOptions {
            format: self.settings.export.format,
            separator: self.settings.export.csv_separator,
            bom: self.settings.export.csv_bom,
        });
        let name = name.unwrap_or_else(|| format!("result_{set}.{}", o.format.extension()));
        let dir = self.file_dir.clone();
        let entries = list_dir(&dir, o.format.extension());
        let labels: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let marks: Vec<bool> = entries.iter().map(|e| e.2).collect();
        let formats = ExportFormat::ALL
            .iter()
            .map(|f| f.label().to_owned())
            .collect();
        let format_index = ExportFormat::ALL
            .iter()
            .position(|f| *f == o.format)
            .unwrap_or(0);
        let separators = vec![
            ", (comma)".to_owned(),
            "; (semicolon)".to_owned(),
            "Tab".to_owned(),
        ];
        let separator_index = match o.separator {
            CsvSeparator::Comma => 0,
            CsvSeparator::Semicolon => 1,
            CsvSeparator::Tab => 2,
        };
        self.dialogs.push(Dialog::new(
            DialogKind::ExportFile,
            "Export result",
            70,
            vec![
                Widget::label(&format!("Folder: {}", dir.display())),
                Widget::field("path", "&File name:", &name, 16),
                Widget::list("files", "", labels, 0, 12).highlighted(marks),
                Widget::combo("format", "For&mat:", formats, format_index, 16),
                Widget::combo(
                    "separator",
                    "CSV &separator:",
                    separators,
                    separator_index,
                    16,
                ),
                Widget::checkbox("bom", "CSV with UTF-8 &BOM", o.bom),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        ));
    }

    /// The file name's extension follows the chosen format (D-042).
    pub(super) fn sync_export_dialog(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if d.kind != DialogKind::ExportFile {
            return;
        }
        let format = export_options(d).format;
        let name = d.text("path").unwrap_or("").to_owned();
        let path = Path::new(&name);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let known = ExportFormat::ALL.iter().any(|f| f.extension() == ext);
        if known && ext != format.extension() {
            let renamed = path.with_extension(format.extension());
            d.set_text("path", &renamed.to_string_lossy());
        }
    }

    pub(super) fn start_export(&mut self, path: std::path::PathBuf, options: ExportOptions) {
        self.settings.export.format = options.format;
        self.settings.export.csv_separator = options.separator;
        self.settings.export.csv_bom = options.bom;
        self.save_settings();
        if let Some(dir) = path.parent() {
            self.file_dir = dir.to_path_buf();
        }
        let Some(sink) = self.export_sink.clone() else {
            self.error("Export is not available.");
            return;
        };
        self.next_export += 1;
        let id = self.next_export;
        let tab = self.tabs[self.active_tab].id;
        let Some(q) = self.query_mut() else {
            return;
        };
        let set = q.active_result;
        let Some(rs) = q.results.get(set) else {
            return;
        };
        let writer = crate::export::spawn(id, path.clone(), options, rs.columns.clone(), sink);
        q.export = Some(ExportRun {
            id,
            set,
            path: path.clone(),
            writer,
            next_loaded: 0,
            writing: false,
            awaiting_rows: false,
            from_stream: false,
            finishing: false,
            written: 0,
            abort_reason: None,
        });
        self.status = format!("Exporting to {}…", path.display());
        self.pump_export(tab);
    }

    /// Hands the next batch to the writer: first the rows in the grid, then rows taken
    /// from the paused stream, then `Finish` (D-021). One batch at a time.
    pub(super) fn pump_export(&mut self, tab: u64) {
        let batch = self.settings.query.batch_rows.max(1) as usize;
        let Some(q) = self
            .tab_index(tab)
            .and_then(|i| self.tabs[i].query.as_mut())
        else {
            return;
        };
        let last = q.results.len().saturating_sub(1);
        let run = q.last_run;
        let Some(e) = q.export.as_mut() else {
            return;
        };
        if e.writing || e.awaiting_rows || e.finishing || e.abort_reason.is_some() {
            return;
        }
        let Some(rs) = q.results.get_mut(e.set) else {
            return;
        };
        if e.next_loaded < rs.rows.len() {
            let end = (e.next_loaded + batch).min(rs.rows.len());
            let rows = rs.rows[e.next_loaded..end].to_vec();
            e.next_loaded = end;
            e.writing = true;
            let _ = e.writer.send(ToWriter::Rows(rows));
        } else if e.set == last && rs.more {
            if rs.fetching {
                // a grid fetch is on its way: its rows land in the grid first
                return;
            }
            if let Some(conn) = q.conn.as_ref() {
                rs.fetching = true;
                e.awaiting_rows = true;
                e.from_stream = true;
                conn.handle.fetch_more(run);
            }
        } else {
            e.finishing = true;
            let _ = e.writer.send(ToWriter::Finish);
        }
    }

    pub(super) fn on_export_event(&mut self, event: ExportEvent) {
        let id = match &event {
            ExportEvent::Written { id, .. }
            | ExportEvent::Done { id, .. }
            | ExportEvent::Failed { id, .. }
            | ExportEvent::Aborted { id } => *id,
        };
        let Some(i) = self.tabs.iter().position(|t| {
            t.query
                .as_ref()
                .and_then(|q| q.export.as_ref())
                .is_some_and(|e| e.id == id)
        }) else {
            return;
        };
        let tab = self.tabs[i].id;
        let active = i == self.active_tab;
        let q = self.tabs[i].query.as_mut().expect("query tab");
        if let ExportEvent::Written { rows, .. } = event {
            if let Some(e) = q.export.as_mut() {
                e.writing = false;
                e.written = rows;
            }
            self.pump_export(tab);
            return;
        }
        let e = q.export.take().expect("export");
        // its batch may still be on the way: not for the grid
        q.drop_rows = e.awaiting_rows;
        // rows were taken from a still paused result: end it (D-042)
        let last = q.results.len().saturating_sub(1);
        let paused = q.results.get(e.set).is_some_and(|rs| rs.more) && e.set == last;
        match event {
            ExportEvent::Done { rows, .. } => {
                if e.from_stream
                    && let Some(rs) = q.results.get_mut(e.set)
                {
                    rs.exported_total = Some(rows);
                }
                let text = format!(
                    "Exported {} rows to {}",
                    crate::grid::thousands(rows as usize),
                    e.path.display()
                );
                q.info(text.clone());
                if active {
                    self.status = text;
                }
            }
            ExportEvent::Failed { error, .. } => {
                if e.from_stream && paused {
                    q.close_paused();
                }
                q.info(format!("Export failed; {} was deleted.", e.path.display()));
                if active {
                    self.status = "Export failed".to_owned();
                }
                self.error(&format!(
                    "{error}

The incomplete file was deleted."
                ));
            }
            ExportEvent::Aborted { .. } => {
                if e.from_stream && paused {
                    q.close_paused();
                }
                let reason = e
                    .abort_reason
                    .unwrap_or_else(|| "Export cancelled".to_owned());
                let text = format!("{reason}; {} was deleted.", e.path.display());
                q.info(text.clone());
                if active {
                    self.status = text;
                }
            }
            ExportEvent::Written { .. } => {}
        }
    }
}

/// Format, separator and BOM chosen in the export dialog.
pub(super) fn export_options(d: &Dialog) -> ExportOptions {
    ExportOptions {
        format: ExportFormat::ALL[d.selected("format").unwrap_or(0).min(2)],
        separator: match d.selected("separator") {
            Some(1) => CsvSeparator::Semicolon,
            Some(2) => CsvSeparator::Tab,
            _ => CsvSeparator::Comma,
        },
        bom: d.checked("bom"),
    }
}

/// Rows the export asked for go to its writer, not to the grid (D-042). Returns the
/// event when it is for the grid / messages.
pub(super) fn take_export_rows(q: &mut QueryState, event: QueryEvent) -> Option<QueryEvent> {
    let last = q.results.len().saturating_sub(1);
    let ended = q.export.is_none() && q.drop_rows;
    let export = q
        .export
        .as_mut()
        .filter(|e| e.awaiting_rows && e.set == last);
    if export.is_none() && !ended {
        return Some(event);
    }
    match event {
        QueryEvent::Rows { rows, more } => {
            match export {
                Some(e) => {
                    e.awaiting_rows = false;
                    if e.abort_reason.is_none() {
                        e.writing = true;
                        let _ = e.writer.send(ToWriter::Rows(rows));
                    }
                }
                None => q.drop_rows = false,
            }
            // a result that is being closed does not pause again (D-019)
            let more = more && !q.running.is_some_and(|r| r.closing);
            if let Some(rs) = q.results.last_mut() {
                rs.more = more;
                rs.fetching = false;
            }
            if let Some(r) = q.running.as_mut() {
                r.paused = more;
            }
            None
        }
        QueryEvent::Error(_) | QueryEvent::Finished { .. } => {
            // the rows will not come: the file is incomplete
            q.drop_rows = false;
            if let Some(e) = export {
                e.awaiting_rows = false;
                e.abort_reason
                    .get_or_insert_with(|| "The query failed during the export".to_owned());
                let _ = e.writer.send(ToWriter::Abort);
            }
            Some(event)
        }
        other => Some(other),
    }
}
