//! The template tab of the developer template editor (D-074): the header of a
//! `.pgtemplate` file as a list of parameters (or as raw TOML), the Jinja body in the
//! tab's own editor, and a live preview of the SQL the template gives.
//!
//! The body is the tab's editor (`Tab::view`), so every editor key works in it; the
//! raw header and the preview are editors of their own, kept here.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use pgc_core::ServerVersion;
use pgc_editor::{Editor, EditorStyles};
use pgc_sql::devtemplate::{self, BodyCheck, Param, TemplateContext, TemplateFile};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme::{Role, Theme};

/// The parts of a template tab the focus can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Name, description and the parameters — or the raw header.
    Header,
    Body,
    Preview,
}

/// Rows of the header pane before the parameters: name, description.
pub const FIXED_ROWS: usize = 2;
/// Buttons under the parameter list.
pub const BUTTONS: [&str; 5] = ["Add", "Edit", "Delete", "Up", "Down"];

/// Screen areas of a template tab from the last frame.
#[derive(Debug, Default, Clone)]
pub struct TplHits {
    /// The header pane (list or raw editor).
    pub header: Rect,
    /// Rows of the header list: (row index, area).
    pub rows: Vec<(usize, Rect)>,
    /// The buttons under the list: (index in [`BUTTONS`], area).
    pub buttons: Vec<(usize, Rect)>,
    pub body: Rect,
    pub preview: Rect,
    /// The line above the preview (drag handle) and the area it divides.
    pub split: Rect,
    pub split_area: Rect,
}

#[derive(Debug)]
pub struct TemplateTab {
    /// The file; `None`: a new template that was not saved yet.
    pub path: Option<PathBuf>,
    /// Section and version folder: of the file, or proposed for the first save.
    pub section: String,
    pub folder: String,
    pub name: String,
    pub description: String,
    pub params: Vec<Param>,
    /// The header is edited as TOML text (`header`), not as the list.
    pub raw: bool,
    pub header: Editor,
    pub preview: Editor,
    pub part: Part,
    /// Selected row of the header list (0 name, 1 description, then the parameters).
    pub cursor: usize,
    /// First parameter row shown (adjusted while drawing).
    pub offset: Cell<usize>,
    pub preview_visible: bool,
    pub preview_percent: u16,
    /// Counts the changes made in the header list; `saved_changes`: its value when
    /// the file was written.
    changes: u64,
    saved_changes: u64,
    /// The header as it is in the file (or as it was typed as raw text): written
    /// back as it is until the list is changed, so a hand-written header keeps its
    /// layout and comments.
    kept_header: Option<String>,
    /// A save is being written.
    pub saving: bool,
    /// Saving over a template that came with pgtc was agreed to (asked once).
    pub builtin_ok: bool,
    /// The checks of the body, the error of a raw header, the error of the preview.
    pub check: BodyCheck,
    pub header_error: Option<String>,
    pub render_error: Option<String>,
    /// The server version the preview is made for; `assumed`: no connected server is
    /// selected in the tree.
    pub version: ServerVersion,
    pub assumed: bool,
    /// What the preview was last made from.
    seen: Option<(u64, u64, u64, u32)>,
}

/// What a save wrote: the revisions of body and raw header and the count of list
/// changes at that moment.
pub type SaveMarks = (u64, u64, u64);

impl TemplateTab {
    /// A new, empty template for `section`.
    pub fn new(section: String, tab_width: usize) -> Self {
        Self {
            path: None,
            section,
            folder: "default".to_owned(),
            name: String::new(),
            description: String::new(),
            params: Vec::new(),
            raw: false,
            header: Editor::new("", false, tab_width),
            preview: Editor::new("", true, tab_width),
            part: Part::Header,
            cursor: 0,
            offset: Cell::new(0),
            preview_visible: true,
            preview_percent: 40,
            changes: 0,
            saved_changes: 0,
            kept_header: None,
            saving: false,
            builtin_ok: false,
            check: BodyCheck::default(),
            header_error: None,
            render_error: None,
            version: ServerVersion(0),
            assumed: true,
            seen: None,
        }
    }

    /// The template of the file `path` (below the templates folder `dir`) with the
    /// text `text`; returns it with the body for the tab's editor. A header that
    /// cannot be read is shown as raw text, so it can be repaired.
    pub fn from_file(dir: &Path, path: PathBuf, text: &str, tab_width: usize) -> (Self, String) {
        let (folder, section) = place_of(dir, &path);
        let mut t = Self::new(section, tab_width);
        t.folder = folder;
        t.path = Some(path);
        t.part = Part::Body;
        let (header, body) = match devtemplate::split(text) {
            Ok(parts) => parts,
            // not a template file at all: everything is body, the header is empty
            Err(_) => (String::new(), text.to_owned()),
        };
        match devtemplate::parse_header(&header) {
            Ok((name, description, params)) => {
                t.name = name.unwrap_or_default();
                t.description = description;
                t.params = params;
                t.kept_header = Some(header);
            }
            Err(_) => {
                t.raw = true;
                t.header.set_text(&header);
            }
        }
        (t, body)
    }

    /// Label in the tab strip.
    pub fn label(&self) -> String {
        let name = if !self.name.trim().is_empty() {
            self.name.trim().to_owned()
        } else if let Some(stem) = self.path.as_ref().and_then(|p| p.file_stem()) {
            stem.to_string_lossy().into_owned()
        } else {
            "New template".to_owned()
        };
        format!("Template: {name}")
    }

    /// File bar of a template without a file: the section and the version folder it
    /// was made for (where the first save proposes to put it).
    pub fn place_note(&self) -> String {
        let section = if self.section.is_empty() {
            "(none)"
        } else {
            &self.section
        };
        format!(
            "New template — section: {section} · version folder: {} · not saved yet",
            self.folder
        )
    }

    /// The header as it is written to the file.
    pub fn header_text(&self) -> String {
        if self.raw {
            self.header.text()
        } else if let Some(kept) = &self.kept_header {
            kept.clone()
        } else {
            devtemplate::header_toml(&self.name, &self.description, &self.params)
        }
    }

    /// The text of the file for `body`.
    pub fn file_text(&self, body: &str) -> String {
        devtemplate::join(&self.header_text(), body)
    }

    /// The template as it would be saved, or why it cannot be: a header that does not
    /// parse, a Jinja error in the body (with its line), or a body that cannot be
    /// rendered with the sample values (an unknown filter or feature name).
    pub fn template(&self, body: &str) -> Result<TemplateFile, (String, Option<usize>)> {
        let (name, description, params) = if self.raw {
            devtemplate::parse_header(&self.header.text()).map_err(|e| (e, None))?
        } else {
            devtemplate::check_declarations(&self.params, false).map_err(|e| (e, None))?;
            (
                Some(self.name.trim().to_owned()).filter(|n| !n.is_empty()),
                self.description.clone(),
                self.params.clone(),
            )
        };
        if let Some((message, line)) = devtemplate::check_body(body, &params).error {
            let at = line.map_or(String::new(), |l| format!("line {l}: "));
            return Err((format!("body, {at}{message}"), line));
        }
        let file = TemplateFile {
            name,
            description,
            params,
            body: body.to_owned(),
        };
        let ctx = TemplateContext {
            version: self.version,
            database: "database".to_owned(),
        };
        let values = devtemplate::sample_values(&file.params);
        devtemplate::render(&file, &values, &ctx)
            .map_err(|e| (format!("it cannot be rendered: {e}"), None))?;
        Ok(file)
    }

    /// A change was made in the header list.
    pub fn touch(&mut self) {
        self.changes += 1;
        self.kept_header = None;
    }

    /// The template differs from its file though nothing was edited (a new variant
    /// that is not written yet).
    pub fn mark_unsaved(&mut self) {
        self.changes += 1;
    }

    /// Something differs from the file.
    pub fn modified(&self, body: &Editor) -> bool {
        body.modified()
            || (self.raw && self.header.modified())
            || self.changes != self.saved_changes
    }

    pub fn save_marks(&self, body: &Editor) -> SaveMarks {
        (body.revision(), self.header.revision(), self.changes)
    }

    /// The file was written with the state `marks`: what did not change since is
    /// saved now.
    pub fn mark_saved(&mut self, body: &mut Editor, marks: SaveMarks) {
        if body.revision() == marks.0 {
            body.set_modified(false);
        }
        if self.header.revision() == marks.1 {
            self.header.set_modified(false);
        }
        self.saved_changes = marks.2;
    }

    /// Switches between the parameter list and the raw header. Back to the list the
    /// text must parse; the error says why it does not.
    pub fn set_raw(&mut self, raw: bool) -> Result<(), String> {
        if raw == self.raw {
            return Ok(());
        }
        if raw {
            let text = self.header_text();
            self.header.set_text(&text);
        } else {
            let (name, description, params) = devtemplate::parse_header(&self.header.text())?;
            if self.header.modified() {
                self.changes += 1;
            }
            // the text as typed stays the header until the list is changed
            self.kept_header = Some(self.header.text());
            self.name = name.unwrap_or_default();
            self.description = description;
            self.params = params;
            self.cursor = self.cursor.min(self.rows() - 1);
        }
        self.raw = raw;
        self.seen = None;
        Ok(())
    }

    /// Rows of the header list.
    pub fn rows(&self) -> usize {
        FIXED_ROWS + self.params.len()
    }

    /// Index of the parameter under the list cursor.
    pub fn selected_param(&self) -> Option<usize> {
        self.cursor
            .checked_sub(FIXED_ROWS)
            .filter(|i| *i < self.params.len())
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.rows() as isize - 1;
        self.cursor = (self.cursor as isize).saturating_add(delta).clamp(0, last) as usize;
    }

    /// Moves the selected parameter up (`-1`) or down (`1`).
    pub fn move_param(&mut self, delta: isize) -> bool {
        let Some(i) = self.selected_param() else {
            return false;
        };
        let to = i as isize + delta;
        if to < 0 || to as usize >= self.params.len() {
            return false;
        }
        self.params.swap(i, to as usize);
        self.cursor = FIXED_ROWS + to as usize;
        self.touch();
        true
    }

    /// The focus moves to the next (`forward`) or previous part that is shown.
    pub fn cycle_part(&mut self, forward: bool) {
        let mut parts = vec![Part::Header, Part::Body];
        if self.preview_visible {
            parts.push(Part::Preview);
        }
        let at = parts.iter().position(|p| *p == self.part).unwrap_or(1);
        let n = parts.len();
        self.part = parts[if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        }];
    }

    /// Makes the preview and the checks again when the body, the header or the
    /// version changed.
    pub fn refresh(&mut self, body: &Editor, version: ServerVersion, assumed: bool) {
        let key = (
            body.revision(),
            self.header.revision(),
            self.changes,
            version.0,
        );
        self.version = version;
        self.assumed = assumed;
        if self.seen == Some(key) {
            return;
        }
        self.seen = Some(key);
        let body = body.text();
        let params = if self.raw {
            match devtemplate::parse_header(&self.header.text()) {
                Ok((_, _, params)) => {
                    self.header_error = None;
                    params
                }
                Err(e) => {
                    self.header_error = Some(e);
                    Vec::new()
                }
            }
        } else {
            self.header_error = devtemplate::check_declarations(&self.params, false).err();
            self.params.clone()
        };
        self.check = if self.header_error.is_some() {
            BodyCheck::default()
        } else {
            devtemplate::check_body(&body, &params)
        };
        self.render_error = None;
        let text = if let Some(e) = &self.header_error {
            format!("The header has an error:\n{e}")
        } else if let Some((message, line)) = &self.check.error {
            let at = line.map_or(String::new(), |l| format!("Line {l}: "));
            format!("The body has an error:\n{at}{message}")
        } else {
            let file = TemplateFile {
                name: None,
                description: String::new(),
                body,
                params,
            };
            let ctx = TemplateContext {
                version,
                database: "database".to_owned(),
            };
            let values = devtemplate::sample_values(&file.params);
            match devtemplate::render(&file, &values, &ctx) {
                Ok(sql) => sql,
                Err(e) => {
                    self.render_error = Some(e.clone());
                    format!("The template cannot be rendered:\n{e}")
                }
            }
        };
        if self.preview.text() != text {
            self.preview.set_text(&text);
        }
    }

    /// Line of the body (0-based) the Jinja error is on.
    pub fn error_line(&self) -> Option<usize> {
        self.check
            .error
            .as_ref()
            .and_then(|(_, line)| *line)
            .map(|l| l.saturating_sub(1))
    }

    /// Left part of the tab's status row: the first problem, or that there is none.
    pub fn status(&self) -> String {
        if let Some(e) = &self.header_error {
            return format!("Header: {}", first_line(e));
        }
        if let Some((message, line)) = &self.check.error {
            let at = line.map_or(String::new(), |l| format!("line {l}: "));
            return format!("Body, {at}{}", first_line(message));
        }
        if let Some(e) = &self.render_error {
            return format!("Preview: {}", first_line(e));
        }
        let mut notes = Vec::new();
        if !self.check.undeclared.is_empty() {
            notes.push(format!(
                "Not declared: {}",
                self.check.undeclared.join(", ")
            ));
        }
        if !self.check.unused.is_empty() {
            notes.push(format!("Not used: {}", self.check.unused.join(", ")));
        }
        if notes.is_empty() {
            "No problems found".to_owned()
        } else {
            notes.join(" · ")
        }
    }

    /// Right part of the status row: the version the preview assumes.
    pub fn version_note(&self) -> String {
        let assumed = if self.assumed { " (assumed)" } else { "" };
        format!("Preview: PostgreSQL {}{assumed}", self.version.major())
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

/// Version folder and section of the template file `path` below the templates folder
/// `dir` (`default`, `Tables`); a file elsewhere has neither.
pub fn place_of(dir: &Path, path: &Path) -> (String, String) {
    let Ok(rel) = path.strip_prefix(dir) else {
        return ("default".to_owned(), String::new());
    };
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.pop();
    if parts.is_empty() {
        return ("default".to_owned(), String::new());
    }
    let folder = parts.remove(0);
    (folder, parts.join("/"))
}

/// The path of a template file below the templates folder, with `/`:
/// `default/Tables/table.pgtemplate`.
pub fn rel_path(dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(dir).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn fit(text: &str, width: usize) -> String {
    let cut: String = text.chars().take(width).collect();
    format!("{cut:<width$}")
}

/// What [`render`] needs besides the tab.
pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub styles: &'a EditorStyles,
    pub line_numbers: bool,
    /// The right frame has the focus.
    pub focused: bool,
    /// Draws a separator line across the window, joined to its frame.
    pub separator: &'a dyn Fn(&mut Frame, Rect),
}

/// Draws the tab into `area` (the tab's content, below the tab strip). Returns the
/// areas and the screen position of the text cursor, if a text has the focus.
pub fn render(
    frame: &mut Frame,
    t: &TemplateTab,
    body: &Editor,
    area: Rect,
    ctx: &RenderContext<'_>,
) -> (TplHits, Option<Position>) {
    let mut hits = TplHits::default();
    let theme = ctx.theme;
    if area.height < 6 || area.width < 20 {
        return (hits, None);
    }
    // the file bar (as D-064)
    let bar = Rect { height: 1, ..area };
    let path = match &t.path {
        Some(p) => p.display().to_string(),
        None => t.place_note(),
    };
    let mark = if t.modified(body) { " *" } else { "" };
    let room = (bar.width as usize).saturating_sub(2 + mark.len());
    let text = format!(" {}{mark}", crate::view::shorten_left(&path, room));
    frame.render_widget(
        Paragraph::new(fit(&text, bar.width as usize)).style(theme.get(Role::TabsInactive)),
        bar,
    );
    let area = Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    };
    let border = theme.get(Role::CoreBorder);

    // header pane left of the body, or above it on a narrow screen
    let (header, right) = if area.width >= 70 {
        let w = (area.width * 42 / 100).clamp(34, 48);
        let line_x = area.x + w;
        for y in area.y..area.bottom() {
            frame
                .buffer_mut()
                .set_string(line_x, y, theme.vline(), border);
        }
        frame.buffer_mut().set_string(
            line_x,
            area.bottom(),
            theme.plain_junctions().bottom,
            border,
        );
        (
            Rect { width: w, ..area },
            Rect {
                x: line_x + 1,
                width: area.width - w - 1,
                ..area
            },
        )
    } else {
        let h = (area.height / 3).clamp(4, 9);
        let line = Rect {
            y: area.y + h,
            height: 1,
            ..area
        };
        (ctx.separator)(frame, line);
        (
            Rect { height: h, ..area },
            Rect {
                y: line.y + 1,
                height: area.height - h - 1,
                ..area
            },
        )
    };
    hits.header = header;
    let mut cursor = None;

    if t.raw {
        let pos = pgc_editor::render(&t.header, header, frame.buffer_mut(), ctx.styles, false);
        if t.part == Part::Header {
            cursor = pos;
        }
    } else {
        render_list(frame, t, header, ctx, &mut hits);
    }

    // body above, preview below
    let (body_area, preview_area) = if t.preview_visible && right.height >= 8 {
        let h = (right.height as u32 * t.preview_percent as u32 / 100) as u16;
        let h = h.clamp(3, right.height - 3);
        let split = Rect {
            y: right.bottom() - h,
            height: 1,
            ..right
        };
        // a caption bar of its own, not a title on a line (it is also the split's
        // drag handle)
        let style = if ctx.focused && t.part == Part::Preview {
            theme.get(Role::TabsActive)
        } else {
            theme.get(Role::TabsInactive)
        };
        frame.render_widget(
            Paragraph::new(fit(" Preview", split.width as usize)).style(style),
            split,
        );
        hits.split = split;
        hits.split_area = right;
        (
            Rect {
                height: right.height - h,
                ..right
            },
            Some(Rect {
                y: split.y + 1,
                height: h - 1,
                ..right
            }),
        )
    } else {
        (right, None)
    };
    hits.body = body_area;
    let pos = pgc_editor::render(
        body,
        body_area,
        frame.buffer_mut(),
        ctx.styles,
        ctx.line_numbers,
    );
    if t.part == Part::Body {
        cursor = pos;
    }
    if let Some(pv) = preview_area {
        hits.preview = pv;
        let pos = pgc_editor::render(&t.preview, pv, frame.buffer_mut(), ctx.styles, false);
        if t.part == Part::Preview {
            cursor = pos;
        }
    }
    (hits, cursor.filter(|_| ctx.focused))
}

/// The header list: name, description, the parameters, the buttons.
fn render_list(
    frame: &mut Frame,
    t: &TemplateTab,
    area: Rect,
    ctx: &RenderContext<'_>,
    hits: &mut TplHits,
) {
    let theme = ctx.theme;
    let normal = theme.get(Role::CoreNormal);
    let hint = theme.get(Role::CoreHint);
    let selected = if ctx.focused && t.part == Part::Header {
        theme.get(Role::CoreSelected)
    } else {
        theme.get(Role::CoreSelectedUnfocused)
    };
    let width = area.width as usize;
    let style_of = |row: usize| if row == t.cursor { selected } else { normal };
    let mut y = area.y;
    let put = |frame: &mut Frame, y: u16, text: String, style| {
        frame.render_widget(
            Paragraph::new(fit(&text, width)).style(style),
            Rect::new(area.x, y, area.width, 1),
        );
    };
    let unnamed = "(no name)".to_owned();
    let fixed = [
        ("Name:", if t.name.is_empty() { &unnamed } else { &t.name }),
        ("Description:", &t.description),
    ];
    for (row, (label, value)) in fixed.into_iter().enumerate() {
        if y >= area.bottom() {
            return;
        }
        put(frame, y, format!(" {label:<13}{value}"), style_of(row));
        hits.rows.push((row, Rect::new(area.x, y, area.width, 1)));
        y += 1;
    }
    if y >= area.bottom() {
        return;
    }
    // caption bar of the parameter list
    put(
        frame,
        y,
        " Parameters".to_owned(),
        theme.get(Role::TabsInactive),
    );
    y += 1;
    // the last row is for the buttons
    let list_rows = area.bottom().saturating_sub(y + 1) as usize;
    if list_rows > 0 {
        let at = t.selected_param().unwrap_or(0);
        let mut offset = t.offset.get().min(t.params.len().saturating_sub(1));
        if at < offset {
            offset = at;
        } else if at >= offset + list_rows {
            offset = at + 1 - list_rows;
        }
        t.offset.set(offset);
        let name_w = t
            .params
            .iter()
            .map(|p| p.name.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(4, 16);
        if t.params.is_empty() {
            put(frame, y, " (none — Insert adds one)".to_owned(), hint);
        }
        for (i, p) in t.params.iter().enumerate().skip(offset).take(list_rows) {
            let row = FIXED_ROWS + i;
            let required = if p.required { " *" } else { "" };
            let detail = match (p.kind, p.source) {
                (_, Some(s)) => s.name().to_owned(),
                (devtemplate::ParamType::List, _) => {
                    format!("list ({})", p.fields.len())
                }
                _ => p.kind.name().to_owned(),
            };
            let name: String = p.name.chars().take(name_w).collect();
            let ry = y + (i - offset) as u16;
            put(
                frame,
                ry,
                format!(" {name:<name_w$}  {detail}{required}"),
                style_of(row),
            );
            hits.rows.push((row, Rect::new(area.x, ry, area.width, 1)));
        }
    }
    // buttons on the last row
    let by = area.bottom() - 1;
    if by <= y && list_rows == 0 {
        return;
    }
    let button = theme.get(Role::TabsInactive);
    let mut x = area.x + 1;
    let mut spans = vec![Span::styled(" ", normal)];
    for (i, label) in BUTTONS.iter().enumerate() {
        let text = format!(" {label} ");
        let w = text.chars().count() as u16;
        if x + w > area.right() {
            break;
        }
        hits.buttons.push((i, Rect::new(x, by, w, 1)));
        spans.push(Span::styled(text, button));
        spans.push(Span::styled(" ", normal));
        x += w + 1;
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(normal),
        Rect::new(area.x, by, area.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "+++\nname = \"Demo\"\n\n[[param]]\nname = \"n\"\ntype = \"identifier\"\nrequired = true\n+++\nCREATE {{ n|ident }};\n";

    fn tab() -> (TemplateTab, Editor) {
        let dir = Path::new("/t");
        let path = dir.join("16_plus").join("Tables").join("demo.pgtemplate");
        let (t, body) = TemplateTab::from_file(dir, path, TEXT, 4);
        let editor = Editor::new(&body, false, 4);
        (t, editor)
    }

    #[test]
    fn reads_a_file_and_writes_it_back() {
        let (mut t, body) = tab();
        assert_eq!(
            (t.folder.as_str(), t.section.as_str()),
            ("16_plus", "Tables")
        );
        assert_eq!(t.name, "Demo");
        assert_eq!(t.label(), "Template: Demo");
        assert!(!t.modified(&body));
        // what is written parses to the same template
        let file = devtemplate::parse(&t.file_text(&body.text())).unwrap();
        assert_eq!(file, devtemplate::parse(TEXT).unwrap());
        t.refresh(&body, ServerVersion(170000), false);
        assert_eq!(t.preview.text(), "CREATE n;\n");
        assert_eq!(t.status(), "No problems found");
        assert_eq!(t.version_note(), "Preview: PostgreSQL 17");
    }

    #[test]
    fn a_hand_written_header_is_kept_until_the_list_changes() {
        let text = "+++
# ours
name   = \"Demo\"

[[param]]
name = \"n\"
type = \"text\"
+++
{{ n }}
";
        let dir = Path::new("/t");
        let (mut t, body) =
            TemplateTab::from_file(dir, dir.join("default").join("d.pgtemplate"), text, 4);
        // only the body changes: the header is written as it was
        assert_eq!(
            t.file_text(
                "{{ n }};
"
            ),
            text.replace("{{ n }}", "{{ n }};")
        );
        // through the raw header and back, unchanged: still the same
        t.set_raw(true).unwrap();
        assert!(t.header.text().contains("# ours"));
        t.set_raw(false).unwrap();
        assert!(!t.modified(&Editor::new(&body, false, 4)));
        assert!(t.header_text().contains("# ours"));
        // a change in the list: pgtc writes the header
        t.name = "Other".into();
        t.touch();
        assert_eq!(
            t.header_text(),
            "name = \"Other\"

[[param]]
name = \"n\"
type = \"text\"
"
        );
    }

    #[test]
    fn raw_header_and_back() {
        let (mut t, mut body) = tab();
        t.set_raw(true).unwrap();
        assert!(t.header.text().contains("name = \"Demo\""));
        assert!(!t.modified(&body));
        // a broken header keeps the raw text
        t.header.set_text("name = ");
        assert!(t.set_raw(false).is_err());
        assert!(t.raw);
        t.refresh(&body, ServerVersion(170000), true);
        assert!(t.status().starts_with("Header:"));
        assert!(t.template(&body.text()).is_err());
        t.header.set_text("name = \"Other\"\n");
        t.header.set_modified(true);
        t.set_raw(false).unwrap();
        assert_eq!(t.name, "Other");
        assert!(t.params.is_empty());
        assert!(t.modified(&body));
        // saved: nothing differs any more
        let marks = t.save_marks(&body);
        t.mark_saved(&mut body, marks);
        assert!(!t.modified(&body));
    }

    #[test]
    fn checks_and_errors() {
        let (mut t, mut body) = tab();
        body.set_text("{{ n }} {{ other }}\n{% if %}");
        t.refresh(&body, ServerVersion(170000), true);
        assert!(t.status().starts_with("Body, line 2:"), "{}", t.status());
        assert_eq!(t.error_line(), Some(1));
        let (message, line) = t.template(&body.text()).unwrap_err();
        assert!(message.starts_with("body, line 2:"));
        assert_eq!(line, Some(2));
        // a body that compiles but cannot be rendered is refused as well
        body.set_text("{{ n|nosuchfilter }}");
        t.refresh(&body, ServerVersion(170000), true);
        assert!(t.status().starts_with("Preview:"), "{}", t.status());
        let (message, line) = t.template(&body.text()).unwrap_err();
        assert!(message.starts_with("it cannot be rendered:"), "{message}");
        assert_eq!(line, None);
        body.set_text("{{ other }}");
        t.refresh(&body, ServerVersion(170000), true);
        assert!(t.template(&body.text()).is_ok());
        assert_eq!(t.status(), "Not declared: other · Not used: n");
        assert_eq!(t.version_note(), "Preview: PostgreSQL 17 (assumed)");
    }

    #[test]
    fn list_cursor_and_order() {
        let (mut t, _) = tab();
        t.params.push(Param::new("b", devtemplate::ParamType::Text));
        t.cursor = FIXED_ROWS;
        assert!(!t.move_param(-1));
        assert!(t.move_param(1));
        assert_eq!(t.params[0].name, "b");
        assert_eq!(t.selected_param(), Some(1));
        t.move_cursor(10);
        assert_eq!(t.cursor, 3);
        t.move_cursor(-10);
        assert_eq!(t.cursor, 0);
        assert_eq!(t.selected_param(), None);
        t.part = Part::Body;
        t.cycle_part(true);
        assert_eq!(t.part, Part::Preview);
        t.cycle_part(true);
        assert_eq!(t.part, Part::Header);
        t.preview_visible = false;
        t.cycle_part(false);
        assert_eq!(t.part, Part::Body);
    }

    #[test]
    fn a_new_template_says_where_it_will_go() {
        let t = TemplateTab::new("Tables/Partitions".to_owned(), 4);
        assert_eq!(
            t.place_note(),
            "New template — section: Tables/Partitions · version folder: default · not saved yet"
        );
        let t = TemplateTab::new(String::new(), 4);
        assert!(t.place_note().contains("section: (none)"));
    }

    #[test]
    fn places() {
        let dir = Path::new("/t");
        assert_eq!(
            place_of(dir, &dir.join("default").join("a.pgtemplate")),
            ("default".to_owned(), String::new())
        );
        assert_eq!(
            place_of(
                dir,
                &dir.join("default").join("A").join("B").join("a.pgtemplate")
            ),
            ("default".to_owned(), "A/B".to_owned())
        );
        assert_eq!(
            rel_path(dir, &dir.join("default").join("A").join("a.pgtemplate")).as_deref(),
            Some("default/A/a.pgtemplate")
        );
        assert_eq!(rel_path(dir, Path::new("/other/a.pgtemplate")), None);
    }
}
