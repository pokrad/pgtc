//! The developer template editor (D-074): the template manager (Tools → Developer
//! templates…), the template tabs with their parameter dialogs and helper menu,
//! saving, and testing a template in its real form.

use std::path::PathBuf;

use crossterm::event::{MouseButton, MouseEventKind};
use pgc_core::{Feature, ServerVersion};
use pgc_editor::{Action, Editor};
use pgc_sql::devtemplate::{self, LibraryOp, Param, ParamType, Source, TemplateLibrary};
use ratatui::layout::{Position, Rect};

use super::editing::editor_action;
use super::file_jobs::FileJob;
use super::{App, Focus, Tab, TabBody, TextView};
use crate::dialog::{Button, Dialog, DialogKind, TemplateNameFor, Widget};
use crate::keymap::Command;
use crate::menu::{self, MenuEntry, Popup};
use crate::template_dialog::TemplateForm;
use crate::template_tab::{self, FIXED_ROWS, Part, SaveMarks, TemplateTab};
use crate::tree::{ConnState, RowKind};
use crate::view::Hits;

/// Where the cursor goes in an inserted text, and where the selection is put.
const CURSOR: char = '\u{1}';
const SELECTION: char = '\u{2}';

/// Label width of the parameter dialog.
const LABEL_W: u16 = 14;
const NO_SECTION: &str = "(no section)";
const NEW_SECTION: &str = "New section…";

/// A row of the template manager.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ManagerRow {
    Section(String),
    /// Entry of the library and the version folder of the file.
    Template(usize, String),
}

/// The template manager (D-074): the library, name and description of every file,
/// the rows shown and the filter they were made for.
pub(in crate::app) struct Manager {
    lib: TemplateLibrary,
    /// Per entry: (version folder, name, description) of its files.
    names: Vec<Vec<(String, String, String)>>,
    rows: Vec<ManagerRow>,
    filter: Option<String>,
}

impl Manager {
    fn scan(dir: &std::path::Path) -> Self {
        let lib = TemplateLibrary::scan(dir);
        let names = lib
            .entries
            .iter()
            .map(|e| {
                e.variants()
                    .iter()
                    .map(|v| match devtemplate::parse(v.text) {
                        Ok(t) => (
                            v.folder.to_owned(),
                            t.name.unwrap_or_else(|| e.stem().to_owned()),
                            t.description,
                        ),
                        Err(err) => (v.folder.to_owned(), format!("{} (error)", e.stem()), err),
                    })
                    .collect()
            })
            .collect();
        Self {
            lib,
            names,
            rows: Vec::new(),
            filter: None,
        }
    }

    /// Rows, their labels and highlight marks: every section (also the empty ones)
    /// with its templates; with a filter only what matches.
    fn build(&self, filter: &str) -> (Vec<ManagerRow>, Vec<String>, Vec<bool>) {
        let mut rows = Vec::new();
        let mut labels = Vec::new();
        let mut marks = Vec::new();
        let mut sections: Vec<&str> = self.lib.sections.iter().map(String::as_str).collect();
        sections.sort_by_key(|s| {
            s.to_lowercase()
                .split('/')
                .map(str::to_owned)
                .collect::<Vec<_>>()
        });
        for section in std::iter::once("").chain(sections) {
            let depth = if section.is_empty() {
                0
            } else {
                section.matches('/').count() + 1
            };
            let mut found = Vec::new();
            for (i, e) in self.lib.entries.iter().enumerate() {
                if e.folders().join("/") != section {
                    continue;
                }
                for (folder, name, desc) in &self.names[i] {
                    if !filter.is_empty()
                        && !name.to_lowercase().contains(filter)
                        && !desc.to_lowercase().contains(filter)
                        && !e.rel.to_lowercase().contains(filter)
                    {
                        continue;
                    }
                    let version = if folder == "default" {
                        String::new()
                    } else {
                        format!("  [{folder}]")
                    };
                    found.push((
                        ManagerRow::Template(i, folder.clone()),
                        format!("{}  {name}{version}", "  ".repeat(depth)),
                    ));
                }
            }
            if found.is_empty() && !filter.is_empty() {
                continue;
            }
            if !section.is_empty() {
                let name = section.rsplit('/').next().unwrap_or(section);
                rows.push(ManagerRow::Section(section.to_owned()));
                labels.push(format!("{}▾ {name}", "  ".repeat(depth - 1)));
                marks.push(true);
            }
            for (row, label) in found {
                rows.push(row);
                labels.push(label);
                marks.push(false);
            }
        }
        (rows, labels, marks)
    }
}

/// A parameter (or a field of a list) as its dialog edits it.
pub(in crate::app) struct Draft {
    param: Param,
    /// Where the values of a choice come from: 0 options, 1 source, 2 query.
    values: usize,
    /// The type and the kind of values the dialog was built for.
    seen: (usize, usize),
}

impl Draft {
    fn new(param: Param) -> Self {
        let values = if param.source.is_some() {
            1
        } else if param.query.is_some() {
            2
        } else {
            0
        };
        Self {
            param,
            values,
            seen: (usize::MAX, usize::MAX),
        }
    }
}

/// The parameter being edited in the parameter dialog of template tab `tab`.
pub(in crate::app) struct ParamEdit {
    /// Its place in the list; `None`: a new one.
    index: Option<usize>,
    draft: Draft,
    /// The field of the list parameter that is being edited over it.
    field: Option<(Option<usize>, Draft)>,
    /// *Make parameter from selection*: the selection of the body it replaces.
    replace: Option<(usize, usize)>,
}

fn param_types(field: bool) -> Vec<ParamType> {
    ParamType::ALL
        .into_iter()
        .filter(|t| !field || *t != ParamType::List)
        .collect()
}

fn field_rows(p: &Param) -> Vec<String> {
    p.fields
        .iter()
        .map(|f| {
            let required = if f.required { " *" } else { "" };
            format!("{:<18} {}{required}", f.name, f.kind.name())
        })
        .collect()
}

/// The dialog of a parameter (`field`: of a field of a list). It shows the inputs of
/// the parameter's type only; changing the type builds it again.
fn param_dialog(tab: u64, field: bool, title: &str, d: &mut Draft) -> Dialog {
    let p = &d.param;
    let types = param_types(field);
    let type_at = types.iter().position(|t| *t == p.kind).unwrap_or(0);
    let mut w = vec![
        Widget::field("name", "&Name:", &p.name, LABEL_W),
        Widget::field(
            "label",
            "&Label:",
            p.label.as_deref().unwrap_or(""),
            LABEL_W,
        ),
        Widget::combo(
            "type",
            "&Type:",
            types.iter().map(|t| t.name().to_owned()).collect(),
            type_at,
            LABEL_W,
        ),
        Widget::checkbox("required", "&Required", p.required),
    ];
    match p.kind {
        ParamType::Bool => w.push(Widget::checkbox(
            "default_on",
            "Checked by &default",
            p.default_bool(),
        )),
        ParamType::List => {}
        _ => w.push(Widget::field(
            "default",
            "&Default:",
            &p.default_text(),
            LABEL_W,
        )),
    }
    w.push(Widget::field(
        "help",
        "&Help:",
        p.help.as_deref().unwrap_or(""),
        LABEL_W,
    ));
    match p.kind {
        ParamType::Choice => {
            w.push(Widget::combo(
                "values",
                "&Values from:",
                vec![
                    "Fixed options".to_owned(),
                    "Database objects".to_owned(),
                    "Query".to_owned(),
                ],
                d.values,
                LABEL_W,
            ));
            match d.values {
                0 => w.push(Widget::memo(
                    "options",
                    "O&ptions, one per line",
                    &p.options.join("\n"),
                    4,
                )),
                1 => {
                    let at = p
                        .source
                        .and_then(|s| Source::ALL.iter().position(|x| *x == s))
                        .unwrap_or(0);
                    w.push(Widget::combo(
                        "source",
                        "&Source:",
                        Source::ALL.iter().map(|s| s.name().to_owned()).collect(),
                        at,
                        LABEL_W,
                    ));
                }
                _ => w.push(Widget::memo(
                    "query",
                    "&Query (its first column gives the values)",
                    p.query.as_deref().unwrap_or(""),
                    4,
                )),
            }
        }
        ParamType::Sql if !field => w.push(Widget::checkbox(
            "fill",
            "Takes the &free rows of the form",
            p.fill,
        )),
        ParamType::List => {
            w.push(Widget::label("Fields of a row:"));
            w.push(Widget::list("fields", "", field_rows(p), 0, 5));
        }
        _ => {}
    }
    w.push(Widget::button(Button::Ok, "&OK", true));
    w.push(Widget::button(Button::Cancel, "&Cancel", false));
    if p.kind == ParamType::List {
        w.push(Widget::button(Button::New, "&Add", false));
        w.push(Widget::button(Button::Edit, "&Edit", false));
        w.push(Widget::button(Button::Delete, "&Delete", false));
        w.push(Widget::button(Button::Up, "&Up", false));
        w.push(Widget::button(Button::Down, "Do&wn", false));
    }
    d.seen = (type_at, d.values);
    Dialog::new(DialogKind::TemplateParam { tab, field }, title, 78, w)
}

/// Takes what the dialog shows into the draft.
fn read_param(dialog: &Dialog, d: &mut Draft, field: bool) {
    let has = |id: &str| dialog.widgets.iter().any(|w| w.id == id);
    let text = |id: &str| dialog.text(id).unwrap_or("").trim().to_owned();
    let p = &mut d.param;
    let types = param_types(field);
    let kind = dialog
        .selected("type")
        .and_then(|i| types.get(i).copied())
        .unwrap_or(p.kind);
    p.name = text("name");
    p.label = Some(text("label")).filter(|s| !s.is_empty());
    p.required = dialog.checked("required");
    let default = if has("default_on") {
        dialog.checked("default_on").then(|| "true".to_owned())
    } else if has("default") {
        Some(dialog.text("default").unwrap_or("").to_owned()).filter(|s| !s.trim().is_empty())
    } else {
        None
    };
    p.default = match (kind, default) {
        (ParamType::List, _) | (_, None) => None,
        (ParamType::Bool, Some(s)) => (s.trim() == "true").then_some(toml::Value::Boolean(true)),
        (ParamType::Number, Some(s)) => Some(if let Ok(i) = s.trim().parse::<i64>() {
            toml::Value::Integer(i)
        } else if let Ok(f) = s.trim().parse::<f64>() {
            toml::Value::Float(f)
        } else {
            toml::Value::String(s)
        }),
        (_, Some(s)) => Some(toml::Value::String(s)),
    };
    p.help = Some(text("help")).filter(|s| !s.is_empty());
    if let Some(v) = dialog.selected("values") {
        d.values = v;
    }
    if let Some(options) = dialog.memo_text("options") {
        p.options = options
            .lines()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect();
    }
    if let Some(i) = dialog.selected("source") {
        p.source = Source::ALL.get(i).copied();
    }
    if let Some(query) = dialog.memo_text("query") {
        p.query = Some(query.trim().to_owned()).filter(|q| !q.is_empty());
    }
    if has("fill") {
        p.fill = dialog.checked("fill");
    }
    p.kind = kind;
}

/// The parameter of a draft as it is declared: only the keys of its type.
fn finished(d: &Draft, field: bool) -> Param {
    let mut p = d.param.clone();
    if p.kind == ParamType::Choice {
        if d.values != 0 {
            p.options.clear();
        }
        if d.values != 1 {
            p.source = None;
        }
        if d.values != 2 {
            p.query = None;
        }
    } else {
        p.options.clear();
        p.source = None;
        p.query = None;
    }
    if p.kind != ParamType::List {
        p.fields.clear();
    }
    if p.kind != ParamType::Sql || field {
        p.fill = false;
    }
    p
}

/// A parameter's own problems, and a name another one (`others`) has already.
fn check_param(p: &Param, others: &[&Param], field: bool) -> Result<(), String> {
    if p.name.is_empty() {
        return Err("The parameter needs a name.".to_owned());
    }
    if others.iter().any(|o| o.name == p.name) {
        return Err(format!("The name '{}' is used already.", p.name));
    }
    devtemplate::check_declarations(std::slice::from_ref(p), field)
}

/// Puts `snippet` at the cursor of `editor`: [`SELECTION`] becomes the selected text
/// (which is replaced), the cursor goes to [`CURSOR`].
fn insert_snippet(editor: &mut Editor, snippet: &str) {
    let selected = editor.selected_text().unwrap_or_default();
    let start = editor.selection().map_or(editor.cursor(), |(a, _)| a);
    let text = snippet.replace(SELECTION, &selected);
    let cursor = text.chars().position(|c| c == CURSOR);
    let text: String = text.chars().filter(|c| *c != CURSOR).collect();
    editor.apply(Action::Insert(text));
    if let Some(at) = cursor {
        editor.set_cursor(start + at, false);
    }
}

/// Puts the filter (`|ident`) where it belongs: after the value of the `{{ … }}` the
/// cursor is in, around a selected name, or into a new `{{ … }}`.
fn insert_filter(editor: &mut Editor, filter: &str) {
    if let Some(selected) = editor.selected_text().filter(|s| !s.trim().is_empty()) {
        let text = format!("{{{{ {}{filter} }}}}", selected.trim());
        editor.apply(Action::Insert(text));
        return;
    }
    let cursor = editor.cursor();
    let line = editor.line_of_char(cursor);
    let chars: Vec<char> = editor.line(line).chars().collect();
    let at = (cursor - editor.line_to_char(line)).min(chars.len());
    let before: String = chars[..at].iter().collect();
    let after: String = chars[at..].iter().collect();
    let inside = match (before.rfind("{{"), before.rfind("}}")) {
        (Some(open), Some(close)) => open > close,
        (Some(_), None) => true,
        _ => false,
    };
    if let (true, Some(end)) = (inside, after.find("}}")) {
        // before the spaces in front of `}}`
        let value = after[..end].trim_end();
        let target = cursor + value.chars().count();
        editor.set_cursor(target, false);
        editor.apply(Action::Insert(filter.to_owned()));
    } else {
        insert_snippet(editor, &format!("{{{{ {CURSOR}{filter} }}}}"));
    }
}

/// A mouse position in a text drawn without line numbers.
fn click_text(editor: &mut Editor, area: Rect, p: Position, select: bool) {
    let line = editor.top + (p.y - area.y) as usize;
    let last = editor.len_lines() - 1;
    if line > last {
        let end = editor.line_to_char(last) + editor.line(last).chars().count();
        editor.set_cursor(end, select);
        return;
    }
    let col = editor.left + p.x.saturating_sub(area.x) as usize;
    let pos = editor.pos_at_col(line, col);
    editor.set_cursor(pos, select);
}

/// What an insert dialog offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Insert {
    Param,
    Construct,
    Filter,
}

/// The Jinja constructs of *Insert construct*: (label, text).
fn constructs() -> Vec<(String, String, String)> {
    let c = CURSOR;
    let s = SELECTION;
    let mut out: Vec<(String, String, String)> = [
        (
            "if … endif",
            format!("{{% if {c} %}}\n{s}\n{{% endif %}}\n"),
            "A part of the SQL only when a condition holds (a checkbox, a filled value)",
        ),
        (
            "if … else … endif",
            format!("{{% if {c} %}}\n{s}\n{{% else %}}\n\n{{% endif %}}\n"),
            "One part of the SQL or another",
        ),
        (
            "for … endfor",
            format!("{{% for row in {c} %}}\n{s}\n{{% endfor %}}\n"),
            "Once for every row of a list parameter; the fields are row.name",
        ),
        (
            "comma between rows",
            "{% if not loop.last %},{% endif %}".to_owned(),
            "Inside a loop: a comma after every row but the last",
        ),
        (
            "value {{ … }}",
            format!("{{{{ {c}{s} }}}}"),
            "The value of a parameter or an expression",
        ),
        (
            "comment {# … #}",
            format!("{{# {s}{c} #}}"),
            "A note for the author; it is not part of the SQL",
        ),
        (
            "server_version check",
            format!("{{% if server_version >= {c}170000 %}}\n{s}\n{{% endif %}}\n"),
            "A part only from a server version on (170000 = PostgreSQL 17.0)",
        ),
    ]
    .into_iter()
    .map(|(label, text, note)| (label.to_owned(), text, note.to_owned()))
    .collect();
    for f in Feature::ALL {
        out.push((
            format!("has(\"{}\")", f.name()),
            format!("{{% if has(\"{}\") %}}\n{s}{c}\n{{% endif %}}\n", f.name()),
            format!(
                "A part only on servers that have {} (PostgreSQL {} and newer)",
                f.name(),
                f.since() / 10000
            ),
        ));
    }
    out
}

impl App {
    // ---- the active template tab --------------------------------------------------

    /// The template of the active tab.
    pub(super) fn template(&self) -> Option<&TemplateTab> {
        self.tabs[self.active_tab].template()
    }

    pub(super) fn template_mut(&mut self) -> Option<&mut TemplateTab> {
        self.tabs[self.active_tab].template_mut()
    }

    /// The header list of the active template tab has the keys.
    pub(super) fn template_list_focused(&self) -> bool {
        self.focus == Focus::Tabs
            && self
                .template()
                .is_some_and(|t| !t.raw && t.part == Part::Header)
    }

    /// The server version a template's preview is made for, and whether it is only
    /// assumed (D-074): the connected server selected in the tree, else the first
    /// version of the template's version folder, else the newest one pgtc knows
    /// features of.
    fn template_version(&self, folder: &str) -> (ServerVersion, bool) {
        if let Some(v) = self
            .tree
            .selected_server()
            .filter(|s| s.state == ConnState::Connected)
            .and_then(|s| s.version)
        {
            return (v, false);
        }
        if let Some(major) = devtemplate::folder_major(folder) {
            return (ServerVersion(major * 10000), true);
        }
        let newest = Feature::ALL
            .iter()
            .map(|f| f.since())
            .max()
            .unwrap_or(pgc_core::version::MIN_SUPPORTED.0);
        (ServerVersion(newest), true)
    }

    /// The texts of the active template tab learn their sizes from the last frame.
    pub(super) fn set_template_views(&mut self, hits: &Hits) {
        let line_numbers = self.settings.editor.line_numbers;
        let tab = &mut self.tabs[self.active_tab];
        let Tab { view, body, .. } = tab;
        let TabBody::Template(t) = body else {
            return;
        };
        let h = &hits.template_tab;
        self.frame.template = h.clone();
        if h.body.width > 0 {
            let gutter = pgc_editor::gutter_width(&view.editor, line_numbers);
            view.editor.set_view(
                h.body.height as usize,
                h.body.width.saturating_sub(gutter) as usize,
            );
        }
        if h.header.width > 0 {
            t.header
                .set_view(h.header.height as usize, h.header.width as usize);
        }
        if h.preview.width > 0 {
            t.preview
                .set_view(h.preview.height as usize, h.preview.width as usize);
        }
    }

    /// Keeps the preview and the checks of the active template tab up to date.
    pub(super) fn sync_template_tab(&mut self) {
        let Some(folder) = self.template().map(|t| t.folder.clone()) else {
            return;
        };
        let (version, assumed) = self.template_version(&folder);
        let Tab { view, body, .. } = &mut self.tabs[self.active_tab];
        let TabBody::Template(t) = body else {
            return;
        };
        if t.part == Part::Preview && !t.preview_visible {
            t.part = Part::Body;
        }
        t.refresh(&view.editor, version, assumed);
    }

    /// Whether a command of the template tab can run now; `None`: not one of them.
    pub(super) fn template_enabled(&self, cmd: Command) -> Option<bool> {
        use Command as C;
        let tab = &self.tabs[self.active_tab];
        let t = tab.template()?;
        let list = !t.raw;
        let at = t.selected_param();
        Some(match cmd {
            C::TemplateTest
            | C::TemplateHelpers
            | C::TemplateRawHeader
            | C::TemplateInsertParam
            | C::TemplateInsertConstruct
            | C::TemplateInsertFilter
            | C::TemplateAddVariant
            | C::ToggleResults => true,
            C::ResultsSmaller | C::ResultsLarger => t.preview_visible,
            C::TemplateMakeParam => list && tab.view.editor.selection().is_some(),
            C::TemplateAddParam | C::TemplateEditParam => list,
            C::TemplateDeleteParam => list && at.is_some(),
            C::TemplateParamUp => list && at.is_some_and(|i| i > 0),
            C::TemplateParamDown => list && at.is_some_and(|i| i + 1 < t.params.len()),
            _ => return None,
        })
    }

    /// Runs a command in the active template tab; `false`: it is not one of the
    /// tab's. `in_view`: the tab has the focus (its keys for moving apply).
    pub(super) fn template_command(&mut self, cmd: Command, in_view: bool) -> bool {
        use Command as C;
        let id = self.tabs[self.active_tab].id;
        let list = in_view && self.template_list_focused();
        let Some(part) = self.template().map(|t| t.part) else {
            return false;
        };
        match cmd {
            C::SaveFile => self.save_template(id, false),
            C::SaveFileAs => self.save_template(id, true),
            C::TemplateTest => self.test_template(),
            C::TemplateHelpers => self.open_template_helpers(),
            C::TemplateRawHeader => self.toggle_raw_header(),
            C::TemplateInsertParam => self.open_insert_dialog(Insert::Param),
            C::TemplateInsertConstruct => self.open_insert_dialog(Insert::Construct),
            C::TemplateInsertFilter => self.open_insert_dialog(Insert::Filter),
            C::TemplateMakeParam => self.make_param(),
            C::TemplateAddVariant => self.open_variant_dialog(),
            C::TemplateAddParam => self.open_param_dialog(None, None),
            C::TemplateEditParam => self.edit_header_row(),
            C::TemplateDeleteParam => self.confirm_delete_param(),
            C::TemplateParamUp | C::TemplateParamDown => {
                let delta = if cmd == C::TemplateParamUp { -1 } else { 1 };
                if let Some(t) = self.template_mut().filter(|t| !t.raw) {
                    t.move_param(delta);
                }
            }
            C::ToggleResults => {
                if let Some(t) = self.template_mut() {
                    t.preview_visible = !t.preview_visible;
                }
            }
            C::ResultsSmaller | C::ResultsLarger => {
                let delta: i16 = if cmd == C::ResultsLarger { 5 } else { -5 };
                if let Some(t) = self.template_mut() {
                    t.preview_percent = (t.preview_percent as i16 + delta).clamp(15, 85) as u16;
                    t.preview_visible = true;
                }
            }
            // the parts of the tab, in a circle (D-074)
            C::FocusLower | C::NextField if in_view => {
                if let Some(t) = self.template_mut() {
                    t.cycle_part(true);
                }
            }
            C::FocusUpper | C::PrevField if in_view => {
                if let Some(t) = self.template_mut() {
                    t.cycle_part(false);
                }
            }
            C::ContextMenu if list => self.open_header_menu(),
            // Enter in the preview: to the line of the error it shows
            C::Newline if in_view && part == Part::Preview => self.jump_to_template_error(),
            C::Up | C::Down | C::PageUp | C::PageDown | C::Home | C::End if list => {
                let delta = match cmd {
                    C::Up => -1,
                    C::Down => 1,
                    C::PageUp => -10,
                    C::PageDown => 10,
                    C::Home => isize::MIN / 2,
                    _ => isize::MAX / 2,
                };
                if let Some(t) = self.template_mut() {
                    t.move_cursor(delta);
                }
            }
            // the list is not a text: the editing keys do nothing in it
            c if list
                && (editor_action(c).is_some()
                    || matches!(
                        c,
                        C::Copy
                            | C::Cut
                            | C::Paste
                            | C::Find
                            | C::FindNext
                            | C::FindPrev
                            | C::Complete
                    )) => {}
            _ => return false,
        }
        true
    }

    /// The cursor of the body goes to the line of its Jinja error.
    fn jump_to_template_error(&mut self) {
        let Some(line) = self.template().and_then(TemplateTab::error_line) else {
            return;
        };
        if let Some(t) = self.template_mut() {
            t.part = Part::Body;
        }
        let editor = &mut self.tabs[self.active_tab].view.editor;
        let line = line.min(editor.len_lines() - 1);
        editor.set_cursor(editor.line_to_char(line), false);
        editor.scroll_to_cursor();
    }

    /// `Ctrl+T`: the parameter list ↔ the header as TOML text. Back to the list the
    /// text must parse.
    fn toggle_raw_header(&mut self) {
        let Some(t) = self.template_mut() else {
            return;
        };
        let raw = !t.raw;
        match t.set_raw(raw) {
            Ok(()) => {
                t.part = Part::Header;
                self.status = if raw {
                    "The header as TOML text".to_owned()
                } else {
                    "The header as a list".to_owned()
                };
            }
            Err(e) => {
                self.status = format!(
                    "The header has an error — {}",
                    e.lines().next().unwrap_or("")
                );
            }
        }
    }

    /// Mouse in the active template tab; `true`: it took the event.
    pub(super) fn template_mouse(
        &mut self,
        kind: MouseEventKind,
        p: Position,
        hits: &Hits,
    ) -> bool {
        let Some(t) = self.template() else {
            return false;
        };
        let raw = t.raw;
        let part = t.part;
        let h = &hits.template_tab;
        let wheel = match kind {
            MouseEventKind::ScrollDown => 3,
            MouseEventKind::ScrollUp => -3,
            _ => 0,
        };
        if wheel != 0 {
            let tab = &mut self.tabs[self.active_tab];
            let Tab { view, body, .. } = tab;
            let TabBody::Template(t) = body else {
                return false;
            };
            if h.header.contains(p) && raw {
                t.header.scroll_lines(wheel);
            } else if h.header.contains(p) {
                t.move_cursor(wheel);
            } else if h.body.contains(p) {
                view.editor.scroll_lines(wheel);
            } else if h.preview.contains(p) {
                t.preview.scroll_lines(wheel);
            } else {
                return false;
            }
            return true;
        }
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(i, _)) = h.buttons.iter().find(|(_, r)| r.contains(p)) {
                    self.focus = Focus::Tabs;
                    if let Some(t) = self.template_mut() {
                        t.part = Part::Header;
                    }
                    let cmd = [
                        Command::TemplateAddParam,
                        Command::TemplateEditParam,
                        Command::TemplateDeleteParam,
                        Command::TemplateParamUp,
                        Command::TemplateParamDown,
                    ][i];
                    if self.is_enabled(cmd) {
                        self.run(cmd);
                    }
                    return true;
                }
                if let Some(&(row, _)) = h.rows.iter().find(|(_, r)| r.contains(p)) {
                    self.focus = Focus::Tabs;
                    let double = self.double_click(p);
                    if let Some(t) = self.template_mut() {
                        t.part = Part::Header;
                        t.cursor = row;
                    }
                    if double {
                        self.edit_header_row();
                    }
                    return true;
                }
                let to = if h.header.contains(p) {
                    Part::Header
                } else if h.body.contains(p) {
                    Part::Body
                } else if h.preview.contains(p) {
                    Part::Preview
                } else {
                    return false;
                };
                self.focus = Focus::Tabs;
                let Some(t) = self.template_mut() else {
                    return false;
                };
                t.part = to;
                match to {
                    Part::Header if raw => click_text(&mut t.header, h.header, p, false),
                    Part::Header => {}
                    Part::Preview => click_text(&mut t.preview, h.preview, p, false),
                    // the body is the tab's editor: the usual click places its cursor
                    Part::Body => {
                        self.frame.editor_area = h.body;
                        return false;
                    }
                }
                true
            }
            MouseEventKind::Drag(MouseButton::Left) if self.focus == Focus::Tabs => {
                let Some(t) = self.template_mut() else {
                    return false;
                };
                match part {
                    Part::Header if raw && h.header.contains(p) => {
                        click_text(&mut t.header, h.header, p, true);
                        true
                    }
                    Part::Preview if h.preview.contains(p) => {
                        click_text(&mut t.preview, h.preview, p, true);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    // ---- the template manager -----------------------------------------------------

    /// Tools → Developer templates… (D-074): the sections and templates of the
    /// templates folder. It needs no connection.
    pub(super) fn open_template_manager(&mut self) {
        if self
            .dialogs
            .iter()
            .any(|d| d.kind == DialogKind::TemplateManager)
        {
            return;
        }
        self.templates.manager = Some(Manager::scan(&self.paths.user_templates_dir()));
        self.dialogs.push(Dialog::new(
            DialogKind::TemplateManager,
            "Developer templates",
            78,
            vec![
                Widget::field("search", "&Search:", "", 9),
                Widget::list("items", "", Vec::new(), 0, 14),
                Widget::label_id("desc", " "),
                Widget::button(Button::Edit, "&Edit", true),
                Widget::button(Button::New, "&New", false),
                Widget::button(Button::NewSection, "Sec&tion", false),
                Widget::button(Button::Copy, "&Copy", false),
                Widget::button(Button::Rename, "&Rename", false),
                Widget::button(Button::Delete, "&Delete", false),
                Widget::button(Button::Cancel, "C&lose", false),
            ],
        ));
        if let Some(d) = self.dialogs.last_mut() {
            d.focus_on("items");
        }
        self.sync_template_manager();
    }

    /// Manager: builds the rows again when the search text (or the library) changed;
    /// shows what the selected row is.
    pub(super) fn sync_template_manager(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if d.kind != DialogKind::TemplateManager {
            return;
        }
        let Some(m) = self.templates.manager.as_mut() else {
            return;
        };
        let filter = d.text("search").unwrap_or("").trim().to_lowercase();
        if m.filter.as_deref() != Some(filter.as_str()) {
            // the library was read again: the selection stays where it was
            let keep = m.filter.is_none().then(|| d.selected("items")).flatten();
            let (rows, labels, marks) = m.build(&filter);
            m.rows = rows;
            m.filter = Some(filter);
            let first = m
                .rows
                .iter()
                .position(|r| matches!(r, ManagerRow::Template(..)))
                .unwrap_or(0);
            let at = keep.map_or(first, |i| i.min(m.rows.len().saturating_sub(1)));
            d.set_list("items", labels, marks, at);
        }
        let desc = match d.selected("items").and_then(|i| m.rows.get(i)) {
            Some(ManagerRow::Section(s)) => format!("Section {s}"),
            Some(ManagerRow::Template(e, folder)) => m.names[*e]
                .iter()
                .find(|n| n.0 == *folder)
                .map(|n| n.2.lines().next().unwrap_or("").to_owned())
                .unwrap_or_default(),
            None if m.rows.is_empty() && m.lib.entries.is_empty() => {
                "No templates yet — New makes one; Settings → Install templates brings pgtc's."
                    .to_owned()
            }
            None => String::new(),
        };
        d.set_label("desc", if desc.is_empty() { " " } else { &desc });
    }

    /// The row selected in the manager.
    fn manager_row(&self, dialog: &Dialog) -> Option<ManagerRow> {
        let m = self.templates.manager.as_ref()?;
        dialog
            .selected("items")
            .and_then(|i| m.rows.get(i).cloned())
    }

    /// A button of the manager. All but *Edit* / *New* (which open a tab) and *Close*
    /// keep it open.
    pub(super) fn manager_button(&mut self, dialog: Dialog, button: Button) {
        let row = self.manager_row(&dialog);
        // path below the version folders, file stem and version folder of a template
        let template = match &row {
            Some(ManagerRow::Template(e, folder)) => self.templates.manager.as_ref().map(|m| {
                let entry = &m.lib.entries[*e];
                (entry.rel.clone(), entry.stem().to_owned(), folder.clone())
            }),
            _ => None,
        };
        match (button, row) {
            (Button::Cancel, _) => self.templates.manager = None,
            (Button::Edit, Some(ManagerRow::Template(e, folder))) => {
                let file = self.templates.manager.take().and_then(|m| {
                    m.lib.entries[e]
                        .variants()
                        .into_iter()
                        .find(|v| v.folder == folder)
                        .map(|v| (PathBuf::from(v.path), v.text.to_owned()))
                });
                if let Some((path, text)) = file {
                    self.open_template_file(path, &text);
                }
            }
            (Button::New, row) => {
                let section = match row {
                    Some(ManagerRow::Section(s)) => s,
                    Some(ManagerRow::Template(..)) => template
                        .as_ref()
                        .and_then(|(rel, _, _)| rel.rsplit_once('/'))
                        .map(|(section, _)| section.to_owned())
                        .unwrap_or_default(),
                    None => String::new(),
                };
                self.templates.manager = None;
                self.new_template_tab(section);
            }
            (Button::NewSection, _) => {
                self.dialogs.push(dialog);
                self.open_name_dialog(TemplateNameFor::NewSection, String::new(), "");
            }
            (Button::Rename, Some(ManagerRow::Section(s))) => {
                self.dialogs.push(dialog);
                let name = s.rsplit('/').next().unwrap_or(&s).to_owned();
                self.open_name_dialog(TemplateNameFor::RenameSection, s, &name);
            }
            (Button::Rename, Some(ManagerRow::Template(..))) => {
                self.dialogs.push(dialog);
                if let Some((rel, stem, _)) = template {
                    self.open_name_dialog(TemplateNameFor::Rename, rel, &stem);
                }
            }
            (Button::Copy, Some(ManagerRow::Template(..))) => {
                self.dialogs.push(dialog);
                if let Some((rel, stem, _)) = template {
                    self.open_name_dialog(TemplateNameFor::Copy, rel, &format!("{stem}_copy"));
                }
            }
            (Button::Delete, Some(ManagerRow::Section(s))) => {
                self.dialogs.push(dialog);
                let text =
                    format!("Delete the section {s}?\n(Only an empty section can be deleted.)");
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmTemplateDelete {
                        op: LibraryOp::DeleteSection(s),
                    },
                    "Delete section",
                    &text,
                ));
            }
            (Button::Delete, Some(ManagerRow::Template(..))) => {
                self.dialogs.push(dialog);
                if let Some((rel, _, folder)) = template {
                    let note = if devtemplate::is_builtin(&format!("{folder}/{rel}")) {
                        "\n(It came with pgtc: Settings → Install templates brings it back.)"
                    } else {
                        ""
                    };
                    let text = format!("Delete the template file {folder}/{rel}?{note}");
                    self.dialogs.push(Dialog::confirm(
                        DialogKind::ConfirmTemplateDelete {
                            op: LibraryOp::Delete { rel, folder },
                        },
                        "Delete template",
                        &text,
                    ));
                }
            }
            // nothing to act on (a section for Edit / Copy, an empty list)
            _ => {
                self.dialogs.push(dialog);
                self.status = "Select a template first".to_owned();
            }
        }
    }

    fn open_name_dialog(&mut self, what: TemplateNameFor, target: String, initial: &str) {
        let (title, label) = match what {
            TemplateNameFor::NewSection => ("New section", "&Section name:"),
            TemplateNameFor::RenameSection => ("Rename section", "&Section name:"),
            TemplateNameFor::Rename => ("Rename template", "&File name:"),
            TemplateNameFor::Copy => ("Copy template", "&File name:"),
        };
        self.dialogs.push(Dialog::new(
            DialogKind::TemplateName { what, target },
            title,
            56,
            vec![
                Widget::field("name", label, initial, 15),
                Widget::button(Button::Ok, "&OK", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        ));
    }

    /// OK in a name dialog of the manager: the change is made in the background.
    pub(super) fn template_name_done(
        &mut self,
        dialog: Dialog,
        what: TemplateNameFor,
        target: &str,
    ) {
        let name = dialog.text("name").unwrap_or("").trim().to_owned();
        let name = name
            .strip_suffix(&format!(".{}", devtemplate::EXTENSION))
            .unwrap_or(&name)
            .to_owned();
        // a new section may be given with its parents: Tables/Partitions
        let parts: Vec<&str> = if what == TemplateNameFor::NewSection {
            name.split(['/', '\\']).map(str::trim).collect()
        } else {
            vec![name.as_str()]
        };
        if let Some(e) = parts
            .iter()
            .find_map(|p| devtemplate::check_file_name(p).err())
        {
            self.dialogs.push(dialog);
            self.error(&e);
            return;
        }
        let target = target.to_owned();
        let op = match what {
            TemplateNameFor::NewSection => LibraryOp::NewSection(parts.join("/")),
            TemplateNameFor::RenameSection => LibraryOp::RenameSection {
                section: target,
                name,
            },
            TemplateNameFor::Rename => LibraryOp::Rename {
                rel: target,
                stem: name,
            },
            TemplateNameFor::Copy => LibraryOp::Copy {
                rel: target,
                stem: name,
            },
        };
        self.start_library_op(op);
    }

    /// Makes a change of the manager in the templates folder, in the background
    /// (D-072).
    pub(super) fn start_library_op(&mut self, op: LibraryOp) {
        let dir = self.paths.user_templates_dir();
        self.start_file_job(FileJob::TemplateOp { dir, op });
    }

    /// A change of the manager was made (or could not be): the manager reads the
    /// folder again; open tabs follow a renamed file.
    pub(super) fn library_op_done(&mut self, op: &LibraryOp, result: Result<(), String>) {
        let dir = self.paths.user_templates_dir();
        match result {
            Err(e) => self.error(&e),
            Ok(()) => {
                self.status = match op {
                    LibraryOp::NewSection(s) => format!("Section {s} created"),
                    LibraryOp::RenameSection { name, .. } => format!("Section renamed to {name}"),
                    LibraryOp::DeleteSection(s) => format!("Section {s} deleted"),
                    LibraryOp::Rename { stem, .. } => format!("Template renamed to {stem}"),
                    LibraryOp::Copy { stem, .. } => format!("Template copied to {stem}"),
                    LibraryOp::Delete { rel, folder } => format!("Deleted {folder}/{rel}"),
                };
                // (version folder, path below it) → the new path below it
                let moved = |rest: &str| -> Option<String> {
                    match op {
                        LibraryOp::Rename { rel, stem } if rest == rel => {
                            let file = format!("{stem}.{}", devtemplate::EXTENSION);
                            Some(match rel.rsplit_once('/') {
                                Some((section, _)) => format!("{section}/{file}"),
                                None => file,
                            })
                        }
                        LibraryOp::RenameSection { section, name } => {
                            let rest = rest.strip_prefix(section.as_str())?.strip_prefix('/')?;
                            Some(match section.rsplit_once('/') {
                                Some((parent, _)) => format!("{parent}/{name}/{rest}"),
                                None => format!("{name}/{rest}"),
                            })
                        }
                        _ => None,
                    }
                };
                for tab in &mut self.tabs {
                    let Some(t) = tab.template_mut() else {
                        continue;
                    };
                    let Some(rel) = t
                        .path
                        .as_ref()
                        .and_then(|p| template_tab::rel_path(&dir, p))
                    else {
                        continue;
                    };
                    let Some((folder, rest)) = rel.split_once('/') else {
                        continue;
                    };
                    if let Some(to) = moved(rest) {
                        let mut path = dir.join(folder);
                        path.extend(to.split('/'));
                        (t.folder, t.section) = template_tab::place_of(&dir, &path);
                        t.path = Some(path);
                    }
                }
            }
        }
        if self.templates.manager.is_some() {
            self.templates.manager = Some(Manager::scan(&dir));
            self.sync_template_manager();
        }
    }

    // ---- template tabs ------------------------------------------------------------

    /// A template file in a tab of its own; a file that is open already is shown.
    pub(super) fn open_template_file(&mut self, path: PathBuf, text: &str) {
        if let Some(i) = self.tabs.iter().position(|t| {
            t.template()
                .is_some_and(|t| t.path.as_deref() == Some(path.as_path()))
        }) {
            self.active_tab = i;
            self.focus = Focus::Tabs;
            return;
        }
        let dir = self.paths.user_templates_dir();
        let (t, body) = TemplateTab::from_file(&dir, path, text, self.tab_width());
        let mut view = TextView::new(false, self.tab_width());
        view.load(body);
        self.add_tab(TabBody::Template(Box::new(t)), view);
        self.sync_template_tab();
    }

    /// A tab with a new, empty template for `section`.
    pub(super) fn new_template_tab(&mut self, section: String) {
        let t = TemplateTab::new(section, self.tab_width());
        let view = TextView::new(false, self.tab_width());
        self.add_tab(TabBody::Template(Box::new(t)), view);
        self.sync_template_tab();
        self.status =
            "New template: Enter on Name, Insert adds a parameter, F7 helps in the body".to_owned();
    }

    /// `F2` in the header list.
    fn open_header_menu(&mut self) {
        let entries = vec![
            menu::item("&Add parameter…", Command::TemplateAddParam),
            menu::item("&Edit…", Command::TemplateEditParam),
            menu::item("&Delete parameter", Command::TemplateDeleteParam),
            menu::item("Move &up", Command::TemplateParamUp),
            menu::item("Move do&wn", Command::TemplateParamDown),
            MenuEntry::Separator,
            menu::item("&Raw header (TOML)", Command::TemplateRawHeader),
            menu::item("&Test…", Command::TemplateTest),
        ];
        let area = self.frame.template.header;
        // below the selected row: the parameters come after a line, and may be scrolled
        let row = self.template().map_or(0, |t| {
            if t.cursor >= FIXED_ROWS {
                (t.cursor + 1).saturating_sub(t.offset.get())
            } else {
                t.cursor
            }
        }) as u16;
        let anchor = (area.x + 2, area.y + row.min(area.height) + 1);
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }

    /// `F7`: the helper menu of the body (D-074).
    fn open_template_helpers(&mut self) {
        let entries = vec![
            menu::item("Insert &parameter…", Command::TemplateInsertParam),
            menu::item("Insert &construct…", Command::TemplateInsertConstruct),
            menu::item("Insert &filter…", Command::TemplateInsertFilter),
            menu::item(
                "&Make parameter from selection…",
                Command::TemplateMakeParam,
            ),
            MenuEntry::Separator,
            menu::item("Add &variant for version…", Command::TemplateAddVariant),
            menu::item("&Raw header (TOML)", Command::TemplateRawHeader),
            menu::item("&Test…", Command::TemplateTest),
            menu::item("Save &as…", Command::SaveFileAs),
        ];
        let area = self.frame.template.body;
        let anchor = (area.x + 2, area.y + 1);
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }

    /// Enter in the header list: name and description, or the parameter.
    fn edit_header_row(&mut self) {
        let id = self.tabs[self.active_tab].id;
        let Some(t) = self.template().filter(|t| !t.raw) else {
            return;
        };
        if let Some(i) = t.selected_param() {
            self.open_param_dialog(Some(i), None);
            return;
        }
        let mut dialog = Dialog::new(
            DialogKind::TemplateProps { tab: id },
            "Template",
            66,
            vec![
                Widget::field("name", "&Name:", &t.name, 14),
                Widget::field("description", "&Description:", &t.description, 14),
                Widget::button(Button::Ok, "&OK", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        );
        if t.cursor == 1 {
            dialog.focus_on("description");
        }
        self.dialogs.push(dialog);
    }

    pub(super) fn template_props_done(&mut self, dialog: &Dialog, tab: u64) {
        let Some(t) = self.tab_mut(tab).and_then(Tab::template_mut) else {
            return;
        };
        let name = dialog.text("name").unwrap_or("").trim().to_owned();
        let description = dialog.text("description").unwrap_or("").trim().to_owned();
        if name != t.name || description != t.description {
            t.name = name;
            t.description = description;
            t.touch();
        }
    }

    // ---- parameters ---------------------------------------------------------------

    /// The dialog of parameter `index` (`None`: a new one, starting as `start`).
    fn open_param_dialog(&mut self, index: Option<usize>, start: Option<(Param, (usize, usize))>) {
        let id = self.tabs[self.active_tab].id;
        let Some(t) = self.template() else {
            return;
        };
        if t.raw {
            self.status = "The header is shown as TOML text — Ctrl+T shows the list".to_owned();
            return;
        }
        let (param, replace) = match (index.and_then(|i| t.params.get(i)), start) {
            (Some(p), _) => (p.clone(), None),
            (None, Some((p, range))) => (p, Some(range)),
            (None, None) => (Param::new("", ParamType::Text), None),
        };
        let title = if index.is_some() {
            "Parameter"
        } else {
            "New parameter"
        };
        let mut draft = Draft::new(param);
        let dialog = param_dialog(id, false, title, &mut draft);
        self.templates.param_edit = Some(ParamEdit {
            index,
            draft,
            field: None,
            replace,
        });
        self.dialogs.push(dialog);
    }

    /// The parameter dialog shows the inputs of the chosen type: when the type (or
    /// where a choice takes its values from) changes, it is built again.
    pub(super) fn sync_param_dialog(&mut self) {
        let Some(d) = self.dialogs.last() else {
            return;
        };
        let DialogKind::TemplateParam { tab, field } = d.kind else {
            return;
        };
        // while a list is open nothing is chosen yet
        if d.combo_open() {
            return;
        }
        let Some(edit) = self.templates.param_edit.as_mut() else {
            return;
        };
        let draft = match (field, edit.field.as_mut()) {
            (true, Some((_, draft))) => draft,
            (true, None) => return,
            (false, _) => &mut edit.draft,
        };
        let now = (
            d.selected("type").unwrap_or(draft.seen.0),
            d.selected("values").unwrap_or(draft.values),
        );
        if now == draft.seen {
            return;
        }
        read_param(d, draft, field);
        let focus = d.focused_id();
        let mut new = param_dialog(tab, field, &d.title.clone(), draft);
        if let Some(id) = focus {
            new.focus_on(id);
        }
        if let Some(top) = self.dialogs.last_mut() {
            *top = new;
        }
    }

    /// Esc in a parameter dialog.
    pub(super) fn param_dialog_cancelled(&mut self, field: bool) {
        if field {
            if let Some(edit) = self.templates.param_edit.as_mut() {
                edit.field = None;
            }
        } else {
            self.templates.param_edit = None;
        }
    }

    /// A button of the parameter dialog (`field`: of the dialog of a list's field).
    pub(super) fn param_dialog_button(
        &mut self,
        dialog: Dialog,
        tab: u64,
        field: bool,
        button: Button,
    ) {
        let Some(mut edit) = self.templates.param_edit.take() else {
            return;
        };
        match (field, button) {
            (true, Button::Ok) => {
                let Some((at, mut draft)) = edit.field.take() else {
                    return;
                };
                read_param(&dialog, &mut draft, true);
                let p = finished(&draft, true);
                let others: Vec<&Param> = edit
                    .draft
                    .param
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| Some(*i) != at)
                    .map(|(_, f)| f)
                    .collect();
                if let Err(e) = check_param(&p, &others, true) {
                    edit.field = Some((at, draft));
                    self.templates.param_edit = Some(edit);
                    self.dialogs.push(dialog);
                    self.error(&e);
                    return;
                }
                let fields = &mut edit.draft.param.fields;
                let at = match at {
                    Some(i) if i < fields.len() => {
                        fields[i] = p;
                        i
                    }
                    _ => {
                        fields.push(p);
                        fields.len() - 1
                    }
                };
                self.templates.param_edit = Some(edit);
                self.rebuild_param_dialog(tab, at);
            }
            (true, _) => {
                edit.field = None;
                self.templates.param_edit = Some(edit);
            }
            (false, Button::Ok) => {
                read_param(&dialog, &mut edit.draft, false);
                let p = finished(&edit.draft, false);
                let Some(i) = self.tab_index(tab) else {
                    return;
                };
                let Tab { view, body, .. } = &mut self.tabs[i];
                let TabBody::Template(t) = body else {
                    return;
                };
                let others: Vec<&Param> = t
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| Some(*i) != edit.index)
                    .map(|(_, p)| p)
                    .collect();
                if let Err(e) = check_param(&p, &others, false) {
                    self.templates.param_edit = Some(edit);
                    self.dialogs.push(dialog);
                    self.error(&e);
                    return;
                }
                let at = match edit.index {
                    Some(at) if at < t.params.len() => {
                        t.params[at] = p.clone();
                        at
                    }
                    _ => {
                        t.params.push(p.clone());
                        t.params.len() - 1
                    }
                };
                t.cursor = FIXED_ROWS + at;
                t.touch();
                // made from a selection of the body: the parameter takes its place
                if let Some((a, b)) = edit.replace {
                    view.editor.set_cursor(a, false);
                    view.editor.set_cursor(b, true);
                    view.editor
                        .apply(Action::Insert(devtemplate::param_snippet(&p)));
                    t.part = Part::Body;
                }
            }
            (false, Button::New | Button::Edit | Button::Delete | Button::Up | Button::Down) => {
                read_param(&dialog, &mut edit.draft, false);
                let fields = &mut edit.draft.param.fields;
                let at = dialog.selected("fields").filter(|i| *i < fields.len());
                match (button, at) {
                    (Button::New, _) => {
                        let mut draft = Draft::new(Param::new("", ParamType::Text));
                        let d = param_dialog(tab, true, "New field", &mut draft);
                        edit.field = Some((None, draft));
                        self.templates.param_edit = Some(edit);
                        self.dialogs.push(dialog);
                        self.dialogs.push(d);
                    }
                    (Button::Edit, Some(i)) => {
                        let mut draft = Draft::new(fields[i].clone());
                        let d = param_dialog(tab, true, "Field", &mut draft);
                        edit.field = Some((Some(i), draft));
                        self.templates.param_edit = Some(edit);
                        self.dialogs.push(dialog);
                        self.dialogs.push(d);
                    }
                    (Button::Delete, Some(i)) => {
                        fields.remove(i);
                        self.templates.param_edit = Some(edit);
                        self.dialogs.push(dialog);
                        self.rebuild_param_dialog(tab, i);
                    }
                    (Button::Up, Some(i)) if i > 0 => {
                        fields.swap(i, i - 1);
                        self.templates.param_edit = Some(edit);
                        self.dialogs.push(dialog);
                        self.rebuild_param_dialog(tab, i - 1);
                    }
                    (Button::Down, Some(i)) if i + 1 < fields.len() => {
                        fields.swap(i, i + 1);
                        self.templates.param_edit = Some(edit);
                        self.dialogs.push(dialog);
                        self.rebuild_param_dialog(tab, i + 1);
                    }
                    _ => {
                        self.templates.param_edit = Some(edit);
                        self.dialogs.push(dialog);
                    }
                }
            }
            // Cancel: the draft is dropped
            (false, _) => {}
        }
    }

    /// The parameter dialog on top again from its draft, field `at` selected.
    fn rebuild_param_dialog(&mut self, tab: u64, at: usize) {
        let Some(edit) = self.templates.param_edit.as_mut() else {
            return;
        };
        let Some(top) = self
            .dialogs
            .last_mut()
            .filter(|d| d.kind == (DialogKind::TemplateParam { tab, field: false }))
        else {
            return;
        };
        let title = top.title.clone();
        let mut d = param_dialog(tab, false, &title, &mut edit.draft);
        d.list_select("fields", at);
        d.focus_on("fields");
        *top = d;
    }

    /// Delete in the header list: asks first (there is no undo for the list).
    fn confirm_delete_param(&mut self) {
        let id = self.tabs[self.active_tab].id;
        let Some((index, name)) = self
            .template()
            .filter(|t| !t.raw)
            .and_then(|t| t.selected_param().map(|i| (i, t.params[i].name.clone())))
        else {
            return;
        };
        self.dialogs.push(Dialog::confirm(
            DialogKind::ConfirmTemplateParamDelete { tab: id, index },
            "Delete parameter",
            &format!("Delete the parameter {name}?"),
        ));
    }

    pub(super) fn delete_param(&mut self, tab: u64, index: usize) {
        let Some(t) = self.tab_mut(tab).and_then(Tab::template_mut) else {
            return;
        };
        if index < t.params.len() {
            t.params.remove(index);
            t.touch();
            t.move_cursor(0);
        }
    }

    // ---- helpers of the body ------------------------------------------------------

    /// The parameters as they are declared now (also from a raw header).
    fn declared_params(&self) -> Result<Vec<Param>, String> {
        let Some(t) = self.template() else {
            return Ok(Vec::new());
        };
        if t.raw {
            devtemplate::parse_header(&t.header.text()).map(|(_, _, params)| params)
        } else {
            Ok(t.params.clone())
        }
    }

    /// A list to choose what is inserted into the body.
    fn open_insert_dialog(&mut self, what: Insert) {
        let id = self.tabs[self.active_tab].id;
        let (title, items): (&str, Vec<(String, String, String)>) = match what {
            Insert::Param => {
                let params = match self.declared_params() {
                    Ok(p) => p,
                    Err(e) => {
                        self.error(&format!("The header has an error:\n{e}"));
                        return;
                    }
                };
                let mut items: Vec<(String, String, String)> = params
                    .iter()
                    .map(|p| {
                        let note = p.help.clone().unwrap_or_else(|| p.label().to_owned());
                        (
                            format!("{:<20} {}", p.name, p.kind.name()),
                            devtemplate::param_snippet(p),
                            note,
                        )
                    })
                    .collect();
                for (name, note) in devtemplate::CONTEXT_VARIABLES {
                    items.push((
                        format!("{name:<20} (always there)"),
                        format!("{{{{ {name} }}}}"),
                        note.to_owned(),
                    ));
                }
                ("Insert parameter", items)
            }
            Insert::Construct => ("Insert construct", constructs()),
            Insert::Filter => (
                "Insert filter",
                devtemplate::FILTERS
                    .iter()
                    .map(|(name, text, note)| {
                        ((*name).to_owned(), (*text).to_owned(), (*note).to_owned())
                    })
                    .collect(),
            ),
        };
        let labels = items.iter().map(|i| i.0.clone()).collect();
        self.templates.insert = items.into_iter().map(|i| (i.1, i.2)).collect();
        self.dialogs.push(Dialog::new(
            DialogKind::TemplateInsert { tab: id },
            title,
            72,
            vec![
                Widget::list("items", "", labels, 0, 12),
                Widget::label_id(
                    "note", " 
 ",
                ),
                Widget::button(Button::Ok, "&OK", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        ));
        self.sync_insert_dialog();
    }

    /// Shows what the selected row of an insert dialog is for.
    pub(super) fn sync_insert_dialog(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if !matches!(d.kind, DialogKind::TemplateInsert { .. }) {
            return;
        }
        let note = d
            .selected("items")
            .and_then(|i| self.templates.insert.get(i))
            .map(|i| i.1.as_str())
            .unwrap_or("");
        // two rows, so the dialog keeps its height
        let width = d.width.saturating_sub(8) as usize;
        let mut rows = crate::help::wrap(note, width);
        rows.resize(2, String::new());
        d.set_label(
            "note",
            &format!(
                "{} 
{} ",
                rows[0], rows[1]
            ),
        );
    }

    /// OK in an insert dialog: the chosen text goes into the body.
    pub(super) fn insert_chosen(&mut self, dialog: &Dialog, tab: u64) {
        let chosen = dialog
            .selected("items")
            .and_then(|i| self.templates.insert.get(i))
            .map(|i| i.0.clone());
        self.templates.insert.clear();
        let (Some(text), Some(i)) = (chosen, self.tab_index(tab)) else {
            return;
        };
        let Tab { view, body, .. } = &mut self.tabs[i];
        let TabBody::Template(t) = body else {
            return;
        };
        t.part = Part::Body;
        if text.starts_with('|') {
            insert_filter(&mut view.editor, &text);
        } else {
            insert_snippet(&mut view.editor, &text);
        }
        self.focus = Focus::Tabs;
    }

    /// *Make parameter from selection*: the selected text of the body becomes the
    /// default of a new parameter that takes its place.
    fn make_param(&mut self) {
        let tab = &self.tabs[self.active_tab];
        let editor = &tab.view.editor;
        let (Some(range), Some(text)) = (editor.selection(), editor.selected_text()) else {
            self.status = "Select the text in the body that becomes a parameter first".to_owned();
            return;
        };
        let kind = if text.contains('\n') {
            ParamType::Sql
        } else {
            ParamType::Text
        };
        let mut param = Param::new("", kind);
        param.default = Some(toml::Value::String(text));
        self.open_param_dialog(None, Some((param, range)));
    }

    // ---- autocomplete in the body ---------------------------------------------------

    /// What is typed at the cursor of the active template tab's body, and what fits
    /// there (D-074). `None`: another part has the focus, or nothing is offered.
    pub(super) fn template_completion(
        &self,
    ) -> Option<(
        crate::template_complete::Found,
        Vec<crate::completion::Item>,
    )> {
        let tab = &self.tabs[self.active_tab];
        let t = tab.template().filter(|t| t.part == Part::Body)?;
        let editor = &tab.view.editor;
        // the parameters as they are declared now; none while a raw header is broken
        let params = if t.raw {
            devtemplate::parse_header(&t.header.text())
                .map(|(_, _, params)| params)
                .unwrap_or_default()
        } else {
            t.params.clone()
        };
        let found = crate::template_complete::analyze(&editor.text(), editor.cursor())?;
        let items = crate::template_complete::items(&found, &params, pgc_editor::lexer::keywords());
        Some((found, items))
    }

    fn template_popup_analysis(
        found: &crate::template_complete::Found,
    ) -> pgc_editor::complete::Analysis {
        pgc_editor::complete::Analysis {
            prefix: found.prefix.clone(),
            start: found.start,
            want: pgc_editor::complete::Want::Any,
            tables: Vec::new(),
        }
    }

    /// Opens the list in the body of the active template tab (`manual`: by key).
    pub(super) fn open_template_completion(&mut self, manual: bool) {
        let Some((found, items)) = self.template_completion() else {
            if manual {
                self.status = "No suggestions".to_owned();
            }
            return;
        };
        if !manual && !crate::template_complete::opens_by_typing(&found) {
            return;
        }
        if items.is_empty() {
            if manual {
                self.status = "No suggestions".to_owned();
            }
            return;
        }
        let mut popup = crate::completion::Popup {
            tab: self.tabs[self.active_tab].id,
            in_form: false,
            analysis: Self::template_popup_analysis(&found),
            items: Vec::new(),
            loading: false,
            selected: 0,
            offset: 0,
            auto: !manual,
        };
        popup.set_items(items, false);
        self.completion = Some(popup);
    }

    /// The body changed while the list is open: new entries; it closes when the
    /// cursor left the word or nothing fits any more.
    pub(super) fn refresh_template_completion(&mut self) {
        use crate::template_complete::Place;
        let Some(popup) = self.completion.as_ref() else {
            return;
        };
        let (start, auto) = (popup.analysis.start, popup.auto);
        let Some((found, items)) = self.template_completion() else {
            self.completion = None;
            return;
        };
        let open_place = matches!(found.place, Place::Attr(_) | Place::Filter | Place::Feature);
        if items.is_empty()
            || (found.prefix.is_empty() && auto && !open_place)
            || (found.start != start && !found.prefix.is_empty())
        {
            self.completion = None;
            return;
        }
        if let Some(p) = self.completion.as_mut() {
            p.analysis = Self::template_popup_analysis(&found);
            p.set_items(items, false);
        }
    }

    // ---- variants -----------------------------------------------------------------

    /// *Add variant for version…*: the template copied into another version folder.
    fn open_variant_dialog(&mut self) {
        let id = self.tabs[self.active_tab].id;
        let Some(t) = self.template() else {
            return;
        };
        if t.path.is_none() {
            self.error("Save the template first: a variant is another file of the same name\nin the folder of a server version.");
            return;
        }
        let version = self
            .tree
            .selected_server()
            .and_then(|s| s.version)
            .map(|v| v.major().to_string())
            .unwrap_or_default();
        self.dialogs.push(Dialog::new(
            DialogKind::TemplateVariant { tab: id },
            "Add variant for version",
            62,
            vec![
                Widget::label(&format!("This file is in the version folder {}.", t.folder)),
                Widget::field("version", "From PostgreSQL &version:", &version, 26),
                Widget::label("The variant is used on servers of that version and newer."),
                Widget::button(Button::Ok, "&OK", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        ));
    }

    pub(super) fn template_variant_chosen(&mut self, dialog: Dialog, tab: u64) {
        let text = dialog.text("version").unwrap_or("").trim().to_owned();
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(t) = self.tabs[i].template() else {
            return;
        };
        let folder = match text.parse::<u32>() {
            Ok(major) if (1..1000).contains(&major) => format!("{major}_plus"),
            _ if text.eq_ignore_ascii_case("default") => "default".to_owned(),
            _ => {
                self.dialogs.push(dialog);
                self.error("Type a major version of PostgreSQL, for example 16.");
                return;
            }
        };
        if folder == t.folder {
            self.dialogs.push(dialog);
            self.error("That is the version folder of this template.");
            return;
        }
        let Some(file) = t.path.as_ref().and_then(|p| p.file_name()) else {
            return;
        };
        let dir = self.paths.user_templates_dir();
        let mut path = dir.join(&folder);
        path.extend(t.section.split('/').filter(|s| !s.is_empty()));
        path.push(file);
        let text = t.file_text(&self.tabs[i].view.editor.text());
        // a variant that is there already is opened as it is
        let rel = template_tab::rel_path(&dir, &path).unwrap_or_default();
        let lib = TemplateLibrary::scan(&dir);
        let existing = lib.entries.iter().find_map(|e| {
            e.variants()
                .into_iter()
                .find(|v| v.folder == folder && format!("{folder}/{}", e.rel) == rel)
                .map(|v| v.text.to_owned())
        });
        match existing {
            Some(text) => {
                self.open_template_file(path, &text);
                self.status = format!("The variant in {folder} exists already");
            }
            None => {
                self.open_template_file(path, &text);
                // not written yet: Save writes it
                if let Some(t) = self.template_mut() {
                    t.mark_unsaved();
                }
                self.status = format!("New variant in {folder} — Ctrl+S saves it");
            }
        }
    }

    // ---- saving -------------------------------------------------------------------

    /// Save (`save_as`: always ask where). A template that does not parse is not
    /// saved (D-074).
    pub(super) fn save_template(&mut self, id: u64, save_as: bool) {
        let Some(i) = self.tab_index(id) else {
            return;
        };
        let tab = &self.tabs[i];
        let Some(t) = tab.template() else {
            return;
        };
        if let Err((e, line)) = t.template(&tab.view.editor.text()) {
            self.active_tab = i;
            if line.is_some() {
                self.jump_to_template_error();
            }
            self.error(&format!(
                "The template is not saved — it has an error:\n{e}"
            ));
            return;
        }
        if t.saving {
            self.status = "The template is still being saved".to_owned();
            return;
        }
        match (t.path.clone(), save_as) {
            (Some(path), false) => {
                let dir = self.paths.user_templates_dir();
                let builtin = template_tab::rel_path(&dir, &path)
                    .is_some_and(|rel| devtemplate::is_builtin(&rel));
                if builtin && !t.builtin_ok {
                    self.dialogs.push(Dialog::confirm_buttons(
                        DialogKind::ConfirmBuiltinSave { tab: id },
                        "Save template",
                        "This template came with pgtc. Installing templates\n\
                         (Settings → Install templates) overwrites this file.",
                        &[
                            (Button::Copy, "Save as &copy"),
                            (Button::Yes, "&Save anyway"),
                            (Button::Cancel, "C&ancel"),
                        ],
                    ));
                } else {
                    self.write_template(id, path, true);
                }
            }
            _ => self.open_template_save_dialog(id, None),
        }
    }

    /// The answer to "this template came with pgtc".
    pub(super) fn builtin_save_answer(&mut self, tab: u64, button: Button) {
        let Some(t) = self.tab_mut(tab).and_then(Tab::template_mut) else {
            return;
        };
        match button {
            Button::Yes => {
                t.builtin_ok = true;
                if let Some(path) = t.path.clone() {
                    self.write_template(tab, path, true);
                }
            }
            Button::Copy => {
                let stem = t
                    .path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| format!("{}_copy", s.to_string_lossy()));
                self.open_template_save_dialog(tab, stem);
            }
            _ => {}
        }
    }

    /// *Save template*: section, version folder and file name (D-074).
    fn open_template_save_dialog(&mut self, id: u64, file: Option<String>) {
        let Some(t) = self.tab_index(id).and_then(|i| self.tabs[i].template()) else {
            return;
        };
        let lib = TemplateLibrary::scan(&self.paths.user_templates_dir());
        let mut sections = vec![NO_SECTION.to_owned()];
        sections.extend(lib.sections.iter().cloned());
        sections.push(NEW_SECTION.to_owned());
        let (selected, new_section) = if t.section.is_empty() {
            (0, "")
        } else {
            match sections.iter().position(|s| *s == t.section) {
                Some(at) => (at, ""),
                None => (sections.len() - 1, t.section.as_str()),
            }
        };
        let stem = file
            .or_else(|| {
                t.path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| devtemplate::file_stem_for(&t.name));
        let lw = 17;
        self.dialogs.push(Dialog::new(
            DialogKind::TemplateSave { tab: id },
            "Save template",
            66,
            vec![
                Widget::combo("section", "&Section:", sections, selected, lw),
                Widget::field("new_section", "&New section:", new_section, lw),
                Widget::field("folder", "&Version folder:", &t.folder, lw),
                Widget::field("file", "&File name:", &stem, lw),
                Widget::label(""),
                Widget::label("Version folder: default for every server, or 16_plus for"),
                Widget::label("PostgreSQL 16 and newer."),
                Widget::button(Button::Ok, "&OK", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        ));
        if let Some(d) = self.dialogs.last_mut() {
            d.focus_on("file");
        }
    }

    /// OK in *Save template*.
    pub(super) fn template_save_chosen(&mut self, dialog: Dialog, tab: u64) {
        let text = |id: &str| dialog.text(id).unwrap_or("").trim().to_owned();
        let chosen = dialog.selected_combo_item("section").unwrap_or(NO_SECTION);
        let section = match chosen {
            NO_SECTION => String::new(),
            NEW_SECTION => text("new_section"),
            other => other.to_owned(),
        };
        let parts: Vec<&str> = section
            .split(['/', '\\'])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let folder = text("folder");
        let file = text("file");
        let file = file
            .strip_suffix(&format!(".{}", devtemplate::EXTENSION))
            .unwrap_or(&file)
            .to_owned();
        let problem = if chosen == NEW_SECTION && parts.is_empty() {
            Some("Type the name of the new section.".to_owned())
        } else if !devtemplate::is_version_folder(&folder) {
            Some(
                "The version folder is default or a version with _plus, for example 16_plus."
                    .to_owned(),
            )
        } else {
            parts
                .iter()
                .find_map(|p| devtemplate::check_file_name(p).err())
                .or_else(|| devtemplate::check_file_name(&file).err())
        };
        let mut path = self.paths.user_templates_dir().join(&folder);
        path.extend(&parts);
        path.push(format!("{file}.{}", devtemplate::EXTENSION));
        let open_elsewhere = self.tabs.iter().any(|t| {
            t.id != tab
                && t.template()
                    .is_some_and(|t| t.path.as_deref() == Some(path.as_path()))
        });
        let problem = problem.or_else(|| {
            open_elsewhere.then(|| format!("{} is open in another tab.", path.display()))
        });
        if let Some(e) = problem {
            self.dialogs.push(dialog);
            self.error(&e);
            return;
        }
        let own = self
            .tab_index(tab)
            .and_then(|i| self.tabs[i].template())
            .is_some_and(|t| t.path.as_deref() == Some(path.as_path()));
        self.write_template(tab, path, own);
    }

    /// Writes the template of tab `tab` to `path` in the background (D-072);
    /// `overwrite`: also over an existing file.
    pub(super) fn write_template(&mut self, tab: u64, path: PathBuf, overwrite: bool) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Tab { view, body, .. } = &mut self.tabs[i];
        let TabBody::Template(t) = body else {
            return;
        };
        t.saving = true;
        let text = t.file_text(&view.editor.text());
        let marks = t.save_marks(&view.editor);
        self.status = format!("Saving {}…", path.display());
        self.start_file_job(FileJob::SaveTemplate {
            tab,
            path,
            text,
            marks,
            overwrite,
        });
    }

    /// The template of tab `tab` was written to `path`, was not because the file
    /// `exists`, or could not be.
    pub(super) fn template_saved(
        &mut self,
        tab: u64,
        path: PathBuf,
        marks: SaveMarks,
        exists: bool,
        result: Result<(), String>,
    ) {
        let dir = self.paths.user_templates_dir();
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Tab { view, body, .. } = &mut self.tabs[i];
        let TabBody::Template(t) = body else {
            return;
        };
        t.saving = false;
        if exists {
            self.status.clear();
            let text = format!("{} exists. Overwrite it?", path.display());
            self.dialogs.push(Dialog::confirm(
                DialogKind::ConfirmTemplateOverwrite { tab, path },
                "Save template",
                &text,
            ));
            return;
        }
        if let Err(e) = result {
            // closing / quitting that waited for this save stops
            self.closing.saves_off();
            self.closing.questions_off();
            self.status.clear();
            self.error(&e);
            return;
        }
        (t.folder, t.section) = template_tab::place_of(&dir, &path);
        t.mark_saved(&mut view.editor, marks);
        self.status = format!("Saved {}", path.display());
        t.path = Some(path);
        self.after_save(tab);
    }

    // ---- test ---------------------------------------------------------------------

    /// The tree's connection to the database of the selected row, when it is open.
    pub(super) fn tree_database_handle(&self) -> Option<pgc_db::ConnectionHandle> {
        let row = self.tree.selected()?;
        let nid = match row.kind {
            RowKind::Node(nid) | RowKind::Loading(nid) => nid,
            RowKind::Server => return None,
        };
        let (_, chain) = self.tree.chain(nid)?;
        chain
            .iter()
            .filter_map(|(_, _, nid)| self.tree.node(*nid))
            .find(|n| n.is_database())?
            .conn
            .as_ref()
            .filter(|c| c.state == ConnState::Connected)?
            .handle
            .clone()
    }

    /// `F5`: the template as it is in the tab, in its real form (D-074). OK there only
    /// closes it. Choice lists are read through the tree's connection to the selected
    /// database.
    fn test_template(&mut self) {
        let tab = &self.tabs[self.active_tab];
        let id = tab.id;
        let Some(t) = tab.template() else {
            return;
        };
        let file = match t.template(&tab.view.editor.text()) {
            Ok(file) => file,
            Err((e, line)) => {
                if line.is_some() {
                    self.jump_to_template_error();
                }
                self.error(&format!("The template has an error:\n{e}"));
                return;
            }
        };
        let name = file.name.clone().unwrap_or_else(|| "template".to_owned());
        let database = self.query_target().map(|(_, db)| db).unwrap_or_default();
        let ctx = devtemplate::TemplateContext {
            version: t.version,
            database,
        };
        let title = format!("Test: {name}");
        let mut form = TemplateForm::new(id, title, file, ctx, self.tab_width());
        self.template_defaults(&mut form);
        self.templates.form = Some(form);
        self.templates.row_choices.clear();
        self.start_lookups();
    }
}
