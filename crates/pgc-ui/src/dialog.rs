//! mc-style dialog framework (D-016): grey box with a frame, labels, inputs,
//! checkboxes, lists, choices and `[< OK >]`-style buttons.
//!
//! Widgets are stacked vertically; all buttons go into one centred row at the bottom.
//! Keys arrive as keymap [`Command`]s from the `[dialog]` context; typed characters go
//! into the focused input; hotkey letters (`Alt+letter`, or the plain letter when the
//! focus is not in an input) activate the widget with that letter. A memo (multi-line
//! text, D-073) gets the editor's keys; `Tab` leaves it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use std::path::PathBuf;

use crate::combo::{self, ComboList, ListEvent};
use crate::export::ExportOptions;
use crate::hotkey::HotText;
use crate::keymap::Command;
use crate::theme::{Role, Theme};
use pgc_editor::{Action, Editor};

/// What a dialog is for, with the caller's data it is about (a server, a tab, a file);
/// the app handles the result by it. The data goes with the dialog, so a cancelled
/// question leaves nothing behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    Message,
    Settings,
    /// New connection (`server` is `None`) or Edit connection (D-035).
    Connection {
        server: Option<u64>,
    },
    Login {
        server: u64,
    },
    ConfirmDelete {
        server: u64,
    },
    Find,
    ReplaceConfirm,
    /// Close a query tab with unsaved text (D-019).
    SaveChanges {
        tab: u64,
    },
    /// Quit with a query tab with unsaved text (D-019).
    QuitSaveChanges {
        tab: u64,
    },
    /// Close a query tab with an open transaction (D-019).
    CloseTransaction {
        tab: u64,
    },
    /// Quit with open transactions (D-019).
    QuitTransactions,
    /// The server did not react to a cancel request (D-041).
    CancelWait {
        tab: u64,
    },
    OpenFile,
    SaveFile,
    ConfirmOverwrite {
        path: PathBuf,
    },
    /// Export result (D-042).
    ExportFile,
    ConfirmExportOverwrite {
        path: PathBuf,
        options: ExportOptions,
    },
    /// Developer templates (D-046): the picker and a row of a list parameter.
    TemplatePicker,
    TemplateRow,
    /// Export / load database definition (D-048): server uid and database.
    SaveDefinition {
        server: u64,
        database: String,
    },
    OpenDefinition {
        server: u64,
        database: String,
    },
    ConfirmDefinitionOverwrite {
        server: u64,
        database: String,
        path: PathBuf,
    },
    /// Install templates into an existing templates folder (D-013); `all_steps`: as
    /// part of Run setup (D-047).
    ConfirmTemplatesOverwrite {
        all_steps: bool,
    },
    /// Help windows: scrolling text with a Close button (D-014).
    Viewer,
    /// psql-style command line (D-054).
    PsqlCommand,
    /// Settings → AI… (D-073).
    AiSettings,
    /// A model name typed by hand, over the AI settings dialog (D-073).
    AiModelOther,
    /// The `F8` prompt of query tab `tab` (D-073).
    AskAi {
        tab: u64,
    },
    /// Find object in the tree (D-055).
    FindObject,
    /// Dashboard tab `tab`: cancel query / terminate session of a session (D-057).
    ConfirmCancelBackend {
        tab: u64,
    },
    ConfirmTerminateBackend {
        tab: u64,
    },
    /// SQL profiler tab `tab`: change the logging settings? and the filter (D-058).
    ConfirmProfilerSettings {
        tab: u64,
    },
    ProfilerFilter {
        tab: u64,
    },
    /// Logging settings the profiler left changed on server `server`: put them back?
    /// (D-058)
    ConfirmProfilerRestore {
        server: u64,
    },
    /// Tools → Developer templates… (D-074).
    TemplateManager,
    /// A name the template manager asks for: of the section or template `target`
    /// (its path below the version folders).
    TemplateName {
        what: TemplateNameFor,
        target: String,
    },
    ConfirmTemplateDelete {
        op: pgc_sql::devtemplate::LibraryOp,
    },
    /// Template tab `tab` (D-074): name and description; a parameter (`field`: a
    /// field of a list parameter, over the parameter's dialog); what to insert into
    /// the body; where to save; the questions of saving; the version of a variant.
    TemplateProps {
        tab: u64,
    },
    TemplateParam {
        tab: u64,
        field: bool,
    },
    TemplateInsert {
        tab: u64,
    },
    TemplateSave {
        tab: u64,
    },
    ConfirmTemplateOverwrite {
        tab: u64,
        path: PathBuf,
    },
    ConfirmBuiltinSave {
        tab: u64,
    },
    TemplateVariant {
        tab: u64,
    },
    ConfirmTemplateParamDelete {
        tab: u64,
        index: usize,
    },
}

/// What a [`DialogKind::TemplateName`] dialog names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateNameFor {
    NewSection,
    RenameSection,
    Rename,
    Copy,
}

/// A `&'static str` for a dynamic widget id (template parameter names). Each distinct
/// name is leaked once.
pub fn intern(name: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = NAMES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(s) = set.get(name) {
        return s;
    }
    let s: &'static str = Box::leak(name.to_owned().into_boxed_str());
    set.insert(s);
    s
}

#[derive(Debug, Clone)]
pub enum WidgetKind {
    Label,
    /// Text input. `label_width` 0 = label on its own row above the field.
    Input {
        value: String,
        cursor: usize,
        masked: bool,
        label_width: u16,
    },
    Checkbox {
        checked: bool,
    },
    /// List box (the psql command list, D-054).
    List {
        items: Vec<String>,
        /// Items drawn in the hotkey colour (e.g. `*.sql` files).
        highlight: Vec<bool>,
        selected: usize,
        offset: usize,
        rows: u16,
        /// The height the list was made with: `rows` on a screen with room for it.
        made_rows: u16,
    },
    /// Combo box (drop-down): shows the selected value; Enter / Space / click opens the
    /// list (crate::combo).
    Combo {
        items: Vec<String>,
        selected: usize,
        label_width: u16,
        /// The open list.
        open: Option<ComboList>,
    },
    Button {
        default: bool,
        button: Button,
    },
    /// Multi-line text in its own frame, the label as the frame's title (D-073).
    Memo {
        editor: MemoEditor,
        rows: u16,
    },
    /// Read-only text that scrolls (Help windows). `headers` lines are drawn in the
    /// hotkey colour.
    Text {
        lines: Vec<String>,
        headers: Vec<bool>,
        offset: usize,
        rows: u16,
    },
}

/// The editor of a memo. A copy (a cloned dialog) has the text and the cursor, not the
/// undo history.
#[derive(Debug)]
pub struct MemoEditor(pub Box<Editor>);

impl Clone for MemoEditor {
    fn clone(&self) -> Self {
        let mut e = Editor::new(&self.0.text(), false, self.0.tab_width());
        e.set_cursor(self.0.cursor(), false);
        Self(Box::new(e))
    }
}

#[derive(Debug, Clone)]
pub struct Widget {
    pub id: &'static str,
    pub label: HotText,
    pub kind: WidgetKind,
}

impl Widget {
    /// A label whose text can be changed later ([`Dialog::set_label`]).
    pub fn label_id(id: &'static str, text: &str) -> Self {
        Self {
            id,
            ..Self::label(text)
        }
    }

    pub fn label(text: &str) -> Self {
        Self {
            id: "",
            label: HotText {
                text: text.to_owned(),
                hot: None,
            },
            kind: WidgetKind::Label,
        }
    }

    /// Input with its label inline, the field starting at column `label_width`.
    pub fn field(id: &'static str, label: &str, value: &str, label_width: u16) -> Self {
        Self {
            id,
            label: HotText::new(label),
            kind: WidgetKind::Input {
                value: value.to_owned(),
                cursor: value.chars().count(),
                masked: false,
                label_width,
            },
        }
    }

    /// Input showing `*` instead of the characters.
    pub fn masked(mut self) -> Self {
        if let WidgetKind::Input { masked, .. } = &mut self.kind {
            *masked = true;
        }
        self
    }

    pub fn checkbox(id: &'static str, label: &str, checked: bool) -> Self {
        Self {
            id,
            label: HotText::new(label),
            kind: WidgetKind::Checkbox { checked },
        }
    }

    pub fn list(
        id: &'static str,
        label: &str,
        items: Vec<String>,
        selected: usize,
        rows: u16,
    ) -> Self {
        let selected = selected.min(items.len().saturating_sub(1));
        let rows = rows.max(1);
        let offset = selected.saturating_sub(rows as usize - 1);
        Self {
            id,
            label: HotText::new(label),
            kind: WidgetKind::List {
                items,
                highlight: Vec::new(),
                selected,
                offset,
                rows,
                made_rows: rows,
            },
        }
    }

    /// Marks list items to be drawn highlighted.
    pub fn highlighted(mut self, marks: Vec<bool>) -> Self {
        if let WidgetKind::List { highlight, .. } = &mut self.kind {
            *highlight = marks;
        }
        self
    }

    pub fn combo(
        id: &'static str,
        label: &str,
        items: Vec<String>,
        selected: usize,
        label_width: u16,
    ) -> Self {
        let selected = selected.min(items.len().saturating_sub(1));
        Self {
            id,
            label: HotText::new(label),
            kind: WidgetKind::Combo {
                items,
                selected,
                label_width,
                open: None,
            },
        }
    }

    /// Scrolling read-only text; [`Dialog::fit_text`] sizes it to the screen.
    pub fn text(id: &'static str, lines: Vec<String>, headers: Vec<bool>) -> Self {
        let rows = lines.len().clamp(1, u16::MAX as usize) as u16;
        Self {
            id,
            label: HotText {
                text: String::new(),
                hot: None,
            },
            kind: WidgetKind::Text {
                lines,
                headers,
                offset: 0,
                rows,
            },
        }
    }

    /// Multi-line text `rows` lines high (D-073).
    pub fn memo(id: &'static str, label: &str, text: &str, rows: u16) -> Self {
        let mut editor = Editor::new(text, false, 4);
        editor.set_cursor(text.chars().count(), false);
        Self {
            id,
            label: HotText::new(label),
            kind: WidgetKind::Memo {
                editor: MemoEditor(Box::new(editor)),
                rows: rows.max(1),
            },
        }
    }

    pub fn button(button: Button, label: &str, default: bool) -> Self {
        Self {
            id: button.id(),
            label: HotText::new(label),
            kind: WidgetKind::Button { default, button },
        }
    }

    /// Which button the widget is, if it is one.
    fn button_of(&self) -> Option<Button> {
        match self.kind {
            WidgetKind::Button { button, .. } => Some(button),
            _ => None,
        }
    }

    fn focusable(&self) -> bool {
        !matches!(self.kind, WidgetKind::Label)
    }

    fn is_button(&self) -> bool {
        matches!(self.kind, WidgetKind::Button { .. })
    }

    fn has_label_row(&self) -> bool {
        !self.label.text.is_empty()
            && matches!(
                self.kind,
                WidgetKind::Input { label_width: 0, .. } | WidgetKind::List { .. }
            )
    }

    /// Rows used by a non-button widget.
    fn height(&self) -> u16 {
        let label_rows = u16::from(self.has_label_row());
        match &self.kind {
            WidgetKind::Label => self.label.text.lines().count().max(1) as u16,
            WidgetKind::Input { .. } => label_rows + 1,
            WidgetKind::List { rows, .. } | WidgetKind::Text { rows, .. } => label_rows + rows,
            WidgetKind::Memo { rows, .. } => rows + 2,
            WidgetKind::Checkbox { .. } | WidgetKind::Combo { .. } | WidgetKind::Button { .. } => 1,
        }
    }

    /// Area of the value part (input field / combo / list rows) inside `area`.
    fn value_rect(&self, area: Rect) -> Rect {
        let lw = match self.kind {
            WidgetKind::Input { label_width, .. } | WidgetKind::Combo { label_width, .. } => {
                label_width
            }
            _ => 0,
        };
        if lw > 0 {
            let lw = lw.min(area.width);
            let mut width = area.width - lw;
            if let WidgetKind::Combo { items, .. } = &self.kind {
                width = combo::field_width(items, width);
            }
            Rect::new(area.x + lw, area.y, width, area.height.min(1))
        } else if self.has_label_row() {
            Rect::new(
                area.x,
                area.y + 1,
                area.width,
                area.height.saturating_sub(1),
            )
        } else {
            area
        }
    }

    fn button_text(&self) -> (String, String) {
        let default = matches!(self.kind, WidgetKind::Button { default: true, .. });
        let (left, right) = button_frame(default);
        (left.into(), right.into())
    }

    fn button_width(&self) -> u16 {
        let (l, r) = self.button_text();
        (l.len() + self.label.width() + r.len()) as u16
    }
}

/// Result of a key or click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogEvent {
    None,
    /// Closed with Esc / F10 / Cancel.
    Cancelled,
    /// This button was pressed.
    Button(Button),
}

/// The buttons of the dialogs. What a button means depends on the dialog it is in
/// ([`DialogKind`]); its text is given where the dialog is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Ok,
    Cancel,
    Yes,
    No,
    /// Open transactions when a tab closes or pgtc quits (D-019).
    Commit,
    Rollback,
    /// The server did not react to a cancel request (D-041): close the connection, or
    /// keep waiting.
    Close,
    Wait,
    /// File dialogs: show the places (D-070).
    Places,
    /// Find / replace (D-038).
    Find,
    Replace,
    ReplaceAll,
    Skip,
    /// Settings the profiler left changed (D-058): drop the note about them.
    Forget,
    /// The template manager and the parameter dialog (D-074).
    New,
    NewSection,
    Edit,
    Copy,
    Rename,
    Delete,
    Up,
    Down,
}

impl Button {
    /// The widget id of the button (for the focus and the tests).
    pub fn id(self) -> &'static str {
        match self {
            Button::Ok => "ok",
            Button::Cancel => "cancel",
            Button::Yes => "yes",
            Button::No => "no",
            Button::Commit => "commit",
            Button::Rollback => "rollback",
            Button::Close => "close",
            Button::Wait => "wait",
            Button::Places => "places",
            Button::Find => "find",
            Button::Replace => "replace",
            Button::ReplaceAll => "all",
            Button::Skip => "skip",
            Button::Forget => "forget",
            Button::New => "new",
            Button::NewSection => "new_section",
            Button::Edit => "edit",
            Button::Copy => "copy",
            Button::Rename => "rename",
            Button::Delete => "delete",
            Button::Up => "up",
            Button::Down => "down",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dialog {
    pub kind: DialogKind,
    pub title: String,
    pub widgets: Vec<Widget>,
    pub focus: usize,
    /// The widget that is not a button and had the focus last: what a button acts
    /// on (OK in a file dialog takes the list's row or the typed name).
    field: usize,
    pub width: u16,
    /// Use the error colours (red box).
    pub error: bool,
}

const CHROME_X: u16 = 6; // padding + frame + margin on both sides
const CHROME_Y: u16 = 4; // padding + frame on both sides

impl Dialog {
    pub fn new(kind: DialogKind, title: &str, width: u16, widgets: Vec<Widget>) -> Self {
        let focus = widgets.iter().position(Widget::focusable).unwrap_or(0);
        Self {
            kind,
            title: title.to_owned(),
            widgets,
            focus,
            field: focus,
            width,
            error: false,
        }
    }

    /// A message box with an OK button.
    pub fn message(title: &str, text: &str, error: bool) -> Self {
        let width = widest_line(text);
        let width = width.max(title.chars().count() as u16 + 4).max(20) + CHROME_X;
        let mut d = Self::new(
            DialogKind::Message,
            title,
            width,
            vec![Widget::label(text), Widget::button(Button::Ok, "&OK", true)],
        );
        d.error = error;
        d
    }

    /// A question with Yes / No buttons (`yes` / `no`).
    pub fn confirm(kind: DialogKind, title: &str, text: &str) -> Self {
        let buttons = [(Button::Yes, "&Yes"), (Button::No, "&No")];
        let mut d = Self::confirm_buttons(kind, title, text, &buttons);
        d.error = true;
        d
    }

    /// A question with the given buttons (`(id, label)`; the first is the default).
    pub fn confirm_buttons(
        kind: DialogKind,
        title: &str,
        text: &str,
        buttons: &[(Button, &str)],
    ) -> Self {
        let width = widest_line(text);
        let buttons_width: u16 = buttons
            .iter()
            .map(|(_, l)| l.chars().count() as u16 + 5)
            .sum();
        let width = width.max(buttons_width).max(24) + CHROME_X;
        let mut widgets = vec![Widget::label(text)];
        for (i, (button, label)) in buttons.iter().enumerate() {
            widgets.push(Widget::button(*button, label, i == 0));
        }
        let mut d = Self::new(kind, title, width, widgets);
        d.focus = 1;
        d
    }

    /// Id of the focused widget.
    pub fn focused_id(&self) -> Option<&'static str> {
        self.widgets.get(self.focus).map(|w| w.id)
    }

    /// The focused widget, or — while a button has the focus — the one that had it
    /// before (clicking OK moves the focus to OK first).
    pub fn field_id(&self) -> Option<&'static str> {
        let i = if self.widgets.get(self.focus).is_some_and(Widget::is_button) {
            self.field
        } else {
            self.focus
        };
        self.widgets.get(i).map(|w| w.id)
    }

    /// Remembers the focused widget when it is not a button (before a key or click
    /// may move the focus to one).
    fn note_field(&mut self) {
        if self.widgets.get(self.focus).is_some_and(|w| !w.is_button()) {
            self.field = self.focus;
        }
    }

    /// The focus is in a masked input (a password): what is typed there must not be
    /// shown or logged.
    pub fn focused_masked(&self) -> bool {
        matches!(
            self.widgets.get(self.focus).map(|w| &w.kind),
            Some(WidgetKind::Input { masked: true, .. })
        )
    }

    /// Moves the focus to the widget `id`.
    pub fn focus_on(&mut self, id: &str) {
        if let Some(i) = self.widgets.iter().position(|w| w.id == id) {
            self.focus = i;
        }
    }

    // ---- values -------------------------------------------------------------------

    /// The kind of widget `id`, to change its value.
    fn kind_mut(&mut self, id: &str) -> Option<&mut WidgetKind> {
        self.widgets
            .iter_mut()
            .find(|w| w.id == id)
            .map(|w| &mut w.kind)
    }

    fn find(&self, id: &str) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }

    pub fn checked(&self, id: &str) -> bool {
        matches!(
            self.find(id).map(|w| &w.kind),
            Some(WidgetKind::Checkbox { checked: true })
        )
    }

    /// Text of memo `id`.
    pub fn memo_text(&self, id: &str) -> Option<String> {
        match self.find(id).map(|w| &w.kind) {
            Some(WidgetKind::Memo { editor, .. }) => Some(editor.0.text()),
            _ => None,
        }
    }

    /// Replaces the text of memo `id` (cursor at the end).
    pub fn set_memo_text(&mut self, id: &str, text: &str) {
        if let Some(WidgetKind::Memo { editor, .. }) = self.kind_mut(id) {
            editor.0.set_text(text);
            editor.0.set_cursor(text.chars().count(), false);
        }
    }

    /// The editor of memo `id` (autocomplete puts names into it, D-073).
    pub fn memo_editor_mut(&mut self, id: &str) -> Option<&mut Editor> {
        match self.kind_mut(id) {
            Some(WidgetKind::Memo { editor, .. }) => Some(&mut editor.0),
            _ => None,
        }
    }

    /// The editor of memo `id`.
    pub fn memo_editor(&self, id: &str) -> Option<&Editor> {
        match self.find(id).map(|w| &w.kind) {
            Some(WidgetKind::Memo { editor, .. }) => Some(&editor.0),
            _ => None,
        }
    }

    /// Where [`Self::render`] puts the text cursor of memo `id` on `screen`.
    pub fn memo_cursor_at(&self, id: &str, screen: Rect) -> Option<Position> {
        let i = self.widgets.iter().position(|w| w.id == id)?;
        let WidgetKind::Memo { editor, .. } = &self.widgets[i].kind else {
            return None;
        };
        let area = *self.widget_areas(self.area(screen)).get(i)?;
        let (inner, row, col, offset) = memo_layout(&editor.0, area);
        (inner.width > 0 && inner.height > 0).then(|| {
            Position::new(
                inner.x + (col as u16).min(inner.width - 1),
                inner.y + (row - offset) as u16,
            )
        })
    }

    /// True while a memo has the focus: keys go to its editor.
    pub fn memo_focused(&self) -> bool {
        matches!(
            self.widgets.get(self.focus).map(|w| &w.kind),
            Some(WidgetKind::Memo { .. })
        )
    }

    pub fn text(&self, id: &str) -> Option<&str> {
        match self.find(id).map(|w| &w.kind) {
            Some(WidgetKind::Input { value, .. }) => Some(value),
            _ => None,
        }
    }

    /// Replaces the text of a label made with [`Widget::label_id`].
    pub fn set_label(&mut self, id: &str, text: &str) {
        if let Some(w) = self
            .widgets
            .iter_mut()
            .find(|w| w.id == id && matches!(w.kind, WidgetKind::Label))
        {
            w.label.text = text.to_owned();
        }
    }

    /// Visible rows of list `id` (its height).
    pub fn set_list_rows(&mut self, id: &str, new_rows: u16) {
        if let Some(WidgetKind::List {
            rows, made_rows, ..
        }) = self.kind_mut(id)
        {
            *rows = new_rows.max(1);
            *made_rows = *rows;
        }
    }

    /// Replaces the items of a list; the selection moves to `selected`.
    pub fn set_list(
        &mut self,
        id: &str,
        new_items: Vec<String>,
        marks: Vec<bool>,
        new_selected: usize,
    ) {
        if let Some(WidgetKind::List {
            items,
            highlight,
            selected,
            offset,
            rows,
            ..
        }) = self.kind_mut(id)
        {
            *selected = new_selected.min(new_items.len().saturating_sub(1));
            *offset = selected.saturating_sub(*rows as usize - 1);
            *items = new_items;
            *highlight = marks;
        }
    }

    /// Where [`Self::render`] puts the text cursor of input `id` on `screen`.
    pub fn input_cursor_at(&self, id: &str, screen: Rect) -> Option<Position> {
        let i = self.widgets.iter().position(|w| w.id == id)?;
        let WidgetKind::Input {
            value: text,
            cursor,
            masked,
            ..
        } = &self.widgets[i].kind
        else {
            return None;
        };
        let area = *self.widget_areas(self.area(screen)).get(i)?;
        let value = self.widgets[i].value_rect(area);
        let view = input_view(text, *cursor, value.width, *masked);
        Some(Position::new(value.x + view.cursor_x, value.y))
    }

    /// Replaces the text of an input (cursor at the end).
    pub fn set_text(&mut self, id: &str, text: &str) {
        if let Some(WidgetKind::Input { value, cursor, .. }) = self.kind_mut(id) {
            *value = text.to_owned();
            *cursor = text.chars().count();
        }
    }

    /// Replaces the items of combo `id` and selects `index`.
    pub fn set_combo(&mut self, id: &str, new_items: Vec<String>, index: usize) {
        if let Some(WidgetKind::Combo {
            items,
            selected,
            open,
            ..
        }) = self.kind_mut(id)
        {
            *selected = index.min(new_items.len().saturating_sub(1));
            *items = new_items;
            *open = None;
        }
    }

    /// Selects item `index` of combo `id`.
    pub fn set_combo_selected(&mut self, id: &str, index: usize) {
        if let Some(WidgetKind::Combo {
            items, selected, ..
        }) = self.kind_mut(id)
            && index < items.len()
        {
            *selected = index;
        }
    }

    /// Text of the selected item of combo `id`.
    pub fn selected_combo_item(&self, id: &str) -> Option<&str> {
        match self.find(id).map(|w| &w.kind) {
            Some(WidgetKind::Combo {
                selected, items, ..
            }) => items.get(*selected).map(String::as_str),
            _ => None,
        }
    }

    /// Selected index of a list or choice.
    /// Text of the selected row of list `id`, as it is shown.
    pub fn selected_item(&self, id: &str) -> Option<&str> {
        match self.find(id).map(|w| &w.kind) {
            Some(WidgetKind::List {
                selected, items, ..
            }) => items.get(*selected).map(String::as_str),
            _ => None,
        }
    }

    pub fn selected(&self, id: &str) -> Option<usize> {
        match self.find(id).map(|w| &w.kind) {
            Some(WidgetKind::List {
                selected, items, ..
            })
            | Some(WidgetKind::Combo {
                selected, items, ..
            }) if !items.is_empty() => Some(*selected),
            _ => None,
        }
    }

    // ---- keys ---------------------------------------------------------------------

    /// The default button pressed (`Enter` outside a button).
    fn default_button(&self) -> DialogEvent {
        match self
            .widgets
            .iter()
            .find(|w| matches!(w.kind, WidgetKind::Button { default: true, .. }))
            .and_then(Widget::button_of)
        {
            Some(button) => DialogEvent::Button(button),
            None => DialogEvent::None,
        }
    }

    #[cfg(test)]
    pub fn handle_key(&mut self, cmd: Option<Command>, ev: &KeyEvent) -> DialogEvent {
        self.handle_key_editor(cmd, None, ev)
    }

    /// As [`Self::handle_key`]; `editor` is the editor action of the key, used when a
    /// memo has the focus (D-073).
    pub fn handle_key_editor(
        &mut self,
        cmd: Option<Command>,
        editor: Option<Action>,
        ev: &KeyEvent,
    ) -> DialogEvent {
        self.note_field();
        if cmd != Some(Command::DialogOk) && self.memo_focused() {
            return self.memo_key(cmd, editor, ev);
        }
        // Ctrl+Enter: the default button from any field; an open combo list first
        // takes its highlighted item (D-016)
        if cmd == Some(Command::DialogOk) {
            if self.combo_open() {
                self.combo_key(Some(Command::Accept), ev);
            }
            return match self
                .widgets
                .iter()
                .find(|w| matches!(w.kind, WidgetKind::Button { default: true, .. }))
                .and_then(Widget::button_of)
            {
                Some(button) => DialogEvent::Button(button),
                None => DialogEvent::None,
            };
        }
        if self.combo_open() {
            self.combo_key(cmd, ev);
            return DialogEvent::None;
        }
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let focused_input = matches!(
            self.widgets.get(self.focus).map(|w| &w.kind),
            Some(WidgetKind::Input { .. })
        );

        if let KeyCode::Char(c) = ev.code {
            if !alt && !ctrl && focused_input {
                self.insert_text(&c.to_string());
                return DialogEvent::None;
            }
            if c.is_alphanumeric() && !ctrl && (alt || !focused_input) {
                let c = c.to_lowercase().next().unwrap_or(c);
                if let Some(i) = self.widgets.iter().position(|w| w.label.key() == Some(c)) {
                    return self.activate(i);
                }
            }
        }

        let Some(cmd) = cmd else {
            return DialogEvent::None;
        };
        match cmd {
            Command::NextField => self.move_focus(1),
            Command::PrevField => self.move_focus(-1),
            Command::Up => {
                if !self.list_move(-1) {
                    self.move_focus(-1);
                }
            }
            Command::Down => {
                if !self.list_move(1) {
                    self.move_focus(1);
                }
            }
            Command::PageUp => {
                let rows = self.list_rows();
                self.list_move(-rows);
            }
            Command::PageDown => {
                let rows = self.list_rows();
                self.list_move(rows);
            }
            Command::Left => {
                if !self.input_cursor(cmd) && !self.choice_move(-1) {
                    self.move_focus(-1);
                }
            }
            Command::Right => {
                if !self.input_cursor(cmd) && !self.choice_move(1) {
                    self.move_focus(1);
                }
            }
            Command::Home => {
                if !self.input_cursor(cmd) {
                    self.list_move(isize::MIN / 2);
                }
            }
            Command::End => {
                if !self.input_cursor(cmd) {
                    self.list_move(isize::MAX / 2);
                }
            }
            // Enter on a combo opens its list, like Space
            Command::Accept
                if matches!(self.widgets[self.focus].kind, WidgetKind::Combo { .. }) =>
            {
                self.combo_set_open(true);
            }
            Command::Accept => {
                if let Some(button) = self.widgets[self.focus].button_of() {
                    return DialogEvent::Button(button);
                }
                return self.default_button();
            }
            Command::Cancel => return DialogEvent::Cancelled,
            Command::Toggle => return self.activate(self.focus),
            Command::Backspace => self.input_edit(true),
            Command::Delete => self.input_edit(false),
            _ => {}
        }
        DialogEvent::None
    }

    /// A key in the focused memo: `Tab` / `Shift+Tab` leave it, `Esc` cancels,
    /// `Alt+letter` goes to the hotkeys, `Enter` is a new line, the rest edits.
    fn memo_key(
        &mut self,
        cmd: Option<Command>,
        editor: Option<Action>,
        ev: &KeyEvent,
    ) -> DialogEvent {
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        match cmd {
            Some(Command::NextField) => {
                self.move_focus(1);
                return DialogEvent::None;
            }
            Some(Command::PrevField) => {
                self.move_focus(-1);
                return DialogEvent::None;
            }
            Some(Command::Cancel) => return DialogEvent::Cancelled,
            _ => {}
        }
        if let KeyCode::Char(c) = ev.code {
            if alt && !ctrl && c.is_alphanumeric() {
                let c = c.to_lowercase().next().unwrap_or(c);
                if let Some(i) = self.widgets.iter().position(|w| w.label.key() == Some(c)) {
                    return self.activate(i);
                }
                return DialogEvent::None;
            }
            if !alt && !ctrl {
                self.memo_apply(Action::Insert(c.to_string()));
                return DialogEvent::None;
            }
        }
        // Up / Down by the rows as they are shown (the text wraps)
        if matches!(cmd, Some(Command::Up | Command::Down))
            && !ev.modifiers.contains(KeyModifiers::SHIFT)
            && self.memo_row_step(cmd == Some(Command::Down))
        {
            return DialogEvent::None;
        }
        if cmd == Some(Command::Accept) {
            self.memo_apply(Action::Insert("\n".to_owned()));
        } else if let Some(action) = editor {
            self.memo_apply(action);
        }
        DialogEvent::None
    }

    /// Moves the cursor of the focused memo one shown row up or down, keeping its
    /// column; false when there is no such row.
    fn memo_row_step(&mut self, down: bool) -> bool {
        let width = self.width.saturating_sub(CHROME_X + 2) as usize;
        let Some(WidgetKind::Memo { editor, .. }) =
            self.widgets.get_mut(self.focus).map(|w| &mut w.kind)
        else {
            return false;
        };
        let e = &mut editor.0;
        let chars: Vec<char> = e.text().chars().collect();
        let rows = wrap_rows(&chars, width);
        let (row, col) = memo_cursor(&rows, e.cursor());
        let target = if down { row + 1 } else { row.wrapping_sub(1) };
        let Some(&(from, to)) = rows.get(target) else {
            return false;
        };
        // the end of a wrapped row is the start of the next one: stay before it
        let wrapped = rows.get(target + 1).is_some_and(|next| next.0 == to);
        let last = if wrapped {
            (to - from).saturating_sub(1)
        } else {
            to - from
        };
        e.set_cursor(from + col.min(last), false);
        true
    }

    fn memo_apply(&mut self, action: Action) {
        let width = self.width.saturating_sub(CHROME_X + 2) as usize;
        if let Some(WidgetKind::Memo { editor, rows }) =
            self.widgets.get_mut(self.focus).map(|w| &mut w.kind)
        {
            // the view (for scrolling to the cursor) is the frame's inside
            editor.0.set_view(*rows as usize, width);
            editor.0.apply(action);
        }
    }

    /// Bracketed paste: into the focused input, newlines replaced by spaces (a memo
    /// keeps them).
    pub fn paste(&mut self, text: &str) {
        if self.memo_focused() {
            // (a tab would leave the box: pasted ones become spaces)
            self.memo_apply(Action::Insert(
                text.replace("\r\n", "\n").replace('\t', "    "),
            ));
            return;
        }
        let clean: String = text
            .chars()
            .map(|c| {
                if c == '\n' || c == '\r' || c == '\t' {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        self.insert_text(&clean);
    }

    fn activate(&mut self, i: usize) -> DialogEvent {
        if !self.widgets[i].focusable() {
            return DialogEvent::None;
        }
        let was_focused = self.focus == i;
        self.focus = i;
        let w = &mut self.widgets[i];
        match &mut w.kind {
            WidgetKind::Checkbox { checked } => *checked = !*checked,
            WidgetKind::Button { button, .. } => return DialogEvent::Button(*button),
            WidgetKind::Combo { .. } if was_focused => self.combo_set_open(true),
            _ => {}
        }
        DialogEvent::None
    }

    fn move_focus(&mut self, delta: isize) {
        // Visit order: non-button widgets top to bottom, then the button row.
        let mut order: Vec<usize> = (0..self.widgets.len())
            .filter(|&i| !self.widgets[i].is_button())
            .collect();
        order.extend((0..self.widgets.len()).filter(|&i| self.widgets[i].is_button()));
        order.retain(|&i| self.widgets[i].focusable());
        let Some(pos) = order.iter().position(|&i| i == self.focus) else {
            return;
        };
        let len = order.len() as isize;
        self.focus = order[(pos as isize + delta).rem_euclid(len) as usize];
    }

    fn list_rows(&self) -> isize {
        match self.widgets[self.focus].kind {
            WidgetKind::List { rows, .. } | WidgetKind::Text { rows, .. } => rows as isize,
            _ => 0,
        }
    }

    /// Sizes every text widget to fit `screen` (called on each update, so a resized
    /// terminal is followed).
    pub fn fit_text(&mut self, screen: Rect) {
        let others: u16 = self
            .widgets
            .iter()
            .filter(|w| {
                !w.is_button()
                    && !matches!(w.kind, WidgetKind::Text { .. } | WidgetKind::List { .. })
            })
            .map(Widget::height)
            .sum();
        let buttons = u16::from(self.widgets.iter().any(Widget::is_button)) * 2;
        // one free row above and below the dialog
        let free = screen
            .height
            .saturating_sub(CHROME_Y + buttons + others + 2)
            .max(3);
        for w in &mut self.widgets {
            if let WidgetKind::Text {
                lines,
                offset,
                rows,
                ..
            } = &mut w.kind
            {
                *rows = (lines.len() as u16).clamp(1, free);
                *offset = (*offset).min(lines.len().saturating_sub(*rows as usize));
            }
            if let WidgetKind::List {
                items,
                selected,
                offset,
                rows,
                made_rows,
                ..
            } = &mut w.kind
            {
                let label = u16::from(!w.label.text.is_empty());
                // the height it was made with, not the number of its items now: a
                // list that is filtered down gets its rows back
                *rows = (*made_rows).clamp(1, free.saturating_sub(label).max(1));
                *offset = (*offset)
                    .min(items.len().saturating_sub(*rows as usize))
                    .min(*selected);
                if *selected >= *offset + *rows as usize {
                    *offset = *selected + 1 - *rows as usize;
                }
            }
        }
    }

    /// Selects row `index` of the list `id` (scrolled into view).
    pub fn list_select(&mut self, id: &str, index: usize) {
        if let Some(i) = self.widgets.iter().position(|w| w.id == id)
            && let WidgetKind::List { selected, .. } = &self.widgets[i].kind
        {
            let delta = index as isize - *selected as isize;
            self.list_move_at(i, delta);
        }
    }

    /// Moves the selection of the list `id` by `delta` rows, whatever has the focus.
    pub fn list_step(&mut self, id: &str, delta: isize) {
        if let Some(i) = self.widgets.iter().position(|w| w.id == id) {
            self.list_move_at(i, delta);
        }
    }

    /// Mouse wheel: scrolls the selection of the (first) list; false if there is none.
    pub fn list_wheel(&mut self, delta: isize) -> bool {
        let list = self
            .widgets
            .iter()
            .position(|w| matches!(w.kind, WidgetKind::List { .. }));
        if let Some(i) = list {
            self.list_move_at(i, delta);
        }
        list.is_some()
    }

    /// Visible rows of the list `id`.
    pub fn list_rows_of(&self, id: &str) -> usize {
        match self.find(id).map(|w| &w.kind) {
            Some(WidgetKind::List { rows, .. }) => *rows as usize,
            _ => 0,
        }
    }

    /// Top line of the text widget `id`.
    #[cfg(test)]
    pub fn text_offset(&self, id: &str) -> Option<usize> {
        match self.find(id).map(|w| &w.kind) {
            Some(WidgetKind::Text { offset, .. }) => Some(*offset),
            _ => None,
        }
    }

    /// Mouse wheel: scrolls the (first) text widget; false if there is none.
    pub fn text_wheel(&mut self, delta: isize) -> bool {
        let Some(w) = self
            .widgets
            .iter_mut()
            .find(|w| matches!(w.kind, WidgetKind::Text { .. }))
        else {
            return false;
        };
        scroll_text(&mut w.kind, delta);
        true
    }

    /// Moves the selection of the focused list (or scrolls the focused text); false if
    /// the focus is neither.
    fn list_move(&mut self, delta: isize) -> bool {
        self.list_move_at(self.focus, delta)
    }

    /// [`Self::list_move`] for widget `i`, whatever has the focus.
    fn list_move_at(&mut self, i: usize, delta: isize) -> bool {
        if matches!(self.widgets[i].kind, WidgetKind::Text { .. }) {
            scroll_text(&mut self.widgets[i].kind, delta);
            return true;
        }
        let WidgetKind::List {
            items,
            selected,
            offset,
            rows,
            ..
        } = &mut self.widgets[i].kind
        else {
            return false;
        };
        if items.is_empty() {
            return true;
        }
        let max = items.len() as isize - 1;
        *selected = (*selected as isize).saturating_add(delta).clamp(0, max) as usize;
        let rows = *rows as usize;
        if *selected < *offset {
            *offset = *selected;
        } else if *selected >= *offset + rows {
            *offset = *selected + 1 - rows;
        }
        true
    }

    /// True while the focused combo box shows its list.
    pub fn combo_open(&self) -> bool {
        matches!(
            self.widgets.get(self.focus).map(|w| &w.kind),
            Some(WidgetKind::Combo { open: Some(_), .. })
        )
    }

    fn combo_set_open(&mut self, want: bool) {
        if let WidgetKind::Combo {
            items,
            selected,
            open,
            ..
        } = &mut self.widgets[self.focus].kind
        {
            *open = (want && !items.is_empty()).then(|| ComboList::open(Some(*selected)));
        }
    }

    /// Applies what the open list did.
    fn combo_event(&mut self, event: ListEvent) {
        match event {
            ListEvent::None => {}
            ListEvent::Chosen(i) => {
                if let WidgetKind::Combo { selected, .. } = &mut self.widgets[self.focus].kind {
                    *selected = i;
                }
                self.combo_set_open(false);
            }
            ListEvent::Closed => self.combo_set_open(false),
            ListEvent::Next | ListEvent::Prev => {
                self.combo_set_open(false);
                self.move_focus(if event == ListEvent::Next { 1 } else { -1 });
            }
        }
    }

    /// Keys of an open combo list.
    fn combo_key(&mut self, cmd: Option<Command>, ev: &KeyEvent) {
        let WidgetKind::Combo {
            items,
            open: Some(list),
            ..
        } = &mut self.widgets[self.focus].kind
        else {
            return;
        };
        let event = list.key(cmd, ev, items);
        self.combo_event(event);
    }

    /// Area of the open combo list (framed, below its field), if a combo is open.
    pub fn combo_list_area(&self, outer: Rect, screen: Rect) -> Option<Rect> {
        let WidgetKind::Combo {
            items,
            open: Some(list),
            ..
        } = &self.widgets.get(self.focus)?.kind
        else {
            return None;
        };
        let areas = self.widget_areas(outer);
        let value = self.widgets[self.focus].value_rect(areas[self.focus]);
        Some(list.area(value, items, screen))
    }

    /// Mouse click while a combo list is open: a row selects it, a click outside
    /// closes the list. Returns false if no combo is open.
    pub fn combo_click(&mut self, outer: Rect, screen: Rect, x: u16, y: u16) -> bool {
        let Some(area) = self.combo_list_area(outer, screen) else {
            return false;
        };
        let WidgetKind::Combo {
            items,
            open: Some(list),
            ..
        } = &self.widgets[self.focus].kind
        else {
            return false;
        };
        let event = list.click(area, Position::new(x, y), items);
        self.combo_event(event);
        true
    }

    /// Mouse wheel while a combo list is open; false if none is open.
    pub fn combo_wheel(&mut self, delta: isize) -> bool {
        let Some(w) = self.widgets.get_mut(self.focus) else {
            return false;
        };
        let WidgetKind::Combo {
            items,
            open: Some(list),
            ..
        } = &mut w.kind
        else {
            return false;
        };
        list.wheel(delta, items);
        true
    }

    /// Changes the value of the focused (closed) combo; false if the focus is not a
    /// combo.
    fn choice_move(&mut self, delta: isize) -> bool {
        let WidgetKind::Combo {
            items, selected, ..
        } = &mut self.widgets[self.focus].kind
        else {
            return false;
        };
        if let Some(s) = combo::step(Some(*selected), items.len(), delta) {
            *selected = s;
        }
        true
    }

    /// Moves the cursor of the focused input as key `cmd` does; false if the focus is
    /// not an input.
    fn input_cursor(&mut self, cmd: Command) -> bool {
        let WidgetKind::Input { value, cursor, .. } = &mut self.widgets[self.focus].kind else {
            return false;
        };
        line::step(cmd, value, cursor);
        true
    }

    fn insert_text(&mut self, text: &str) {
        if let WidgetKind::Input { value, cursor, .. } = &mut self.widgets[self.focus].kind {
            line::insert(value, cursor, text);
        }
    }

    fn input_edit(&mut self, backspace: bool) {
        if let WidgetKind::Input { value, cursor, .. } = &mut self.widgets[self.focus].kind {
            if backspace {
                line::backspace(value, cursor);
            } else {
                line::delete(value, *cursor);
            }
        }
    }

    // ---- mouse --------------------------------------------------------------------

    /// Left click at (`x`, `y`) inside widget `i` whose area is `area`; `double`: the
    /// second click of a double click, which on a list row acts as `Enter` (D-067).
    pub fn click(&mut self, i: usize, area: Rect, x: u16, y: u16, double: bool) -> DialogEvent {
        self.note_field();
        if !self.widgets[i].focusable() {
            return DialogEvent::None;
        }
        let value = self.widgets[i].value_rect(area);
        match &mut self.widgets[i].kind {
            WidgetKind::List {
                items,
                selected,
                offset,
                ..
            } => {
                self.focus = i;
                let mut on_row = false;
                if y >= value.y {
                    let row = (y - value.y) as usize;
                    if *offset + row < items.len() {
                        *selected = *offset + row;
                        on_row = true;
                    }
                }
                if double && on_row {
                    return self.default_button();
                }
                DialogEvent::None
            }
            WidgetKind::Input {
                value: text,
                cursor,
                masked,
                ..
            } => {
                self.focus = i;
                let view = input_view(text, *cursor, value.width, *masked);
                *cursor = input_char_at(text, view.from, x.saturating_sub(value.x), *masked);
                DialogEvent::None
            }
            WidgetKind::Combo { open, .. } => {
                let was_open = open.is_some();
                self.focus = i;
                self.combo_set_open(!was_open);
                DialogEvent::None
            }
            _ => self.activate(i),
        }
    }

    // ---- layout and drawing -------------------------------------------------------

    /// Outer rectangle of the dialog, centred in `screen`.
    pub fn area(&self, screen: Rect) -> Rect {
        let content: u16 = self
            .widgets
            .iter()
            .filter(|w| !w.is_button())
            .map(Widget::height)
            .sum();
        let buttons = u16::from(self.widgets.iter().any(Widget::is_button)) * 2;
        let width = self.width.min(screen.width);
        let height = (content + buttons + CHROME_Y).min(screen.height);
        Rect {
            x: screen.x + (screen.width - width) / 2,
            y: screen.y + (screen.height - height) / 3,
            width,
            height,
        }
    }

    /// Area of every widget inside `outer`.
    pub fn widget_areas(&self, outer: Rect) -> Vec<Rect> {
        let inner = Rect {
            x: outer.x + CHROME_X / 2,
            y: outer.y + CHROME_Y / 2,
            width: outer.width.saturating_sub(CHROME_X),
            height: outer.height.saturating_sub(CHROME_Y),
        };
        let bottom = inner.y + inner.height;
        let mut areas = vec![Rect::default(); self.widgets.len()];
        let mut y = inner.y;
        for (i, w) in self.widgets.iter().enumerate() {
            if w.is_button() {
                continue;
            }
            let h = w.height().min(bottom.saturating_sub(y));
            areas[i] = Rect::new(inner.x, y, inner.width, h);
            y += h;
        }
        let total: u16 = self
            .widgets
            .iter()
            .filter(|w| w.is_button())
            .map(|w| w.button_width() + 1)
            .sum::<u16>()
            .saturating_sub(1);
        let mut x = inner.x + inner.width.saturating_sub(total) / 2;
        let row = bottom.saturating_sub(1);
        for (i, w) in self.widgets.iter().enumerate() {
            if w.is_button() {
                let width = w
                    .button_width()
                    .min((inner.x + inner.width).saturating_sub(x));
                areas[i] = Rect::new(x, row, width, 1);
                x += w.button_width() + 1;
            }
        }
        areas
    }

    /// Draws the dialog; returns its outer area and the widget areas (for mouse hits).
    pub fn render(&self, frame: &mut Frame, theme: &Theme) -> (Rect, Vec<Rect>) {
        let screen = frame.area();
        let outer = self.area(screen);
        let (normal, title_style) = if self.error {
            (theme.get(Role::ErrorNormal), theme.get(Role::ErrorTitle))
        } else {
            (theme.get(Role::DialogNormal), theme.get(Role::DialogTitle))
        };
        let hot = theme.get(Role::DialogHotkey);
        let focus = theme.get(Role::DialogFocus);
        let focus_hot = theme.get(Role::DialogFocusHotkey);

        draw_shadow(frame, outer, theme.get(Role::DialogShadow));
        frame.render_widget(Clear, outer);
        frame.render_widget(Block::new().style(normal), outer);
        let framed = Rect {
            x: outer.x + 1,
            y: outer.y + 1,
            width: outer.width.saturating_sub(2),
            height: outer.height.saturating_sub(2),
        };
        frame.render_widget(
            Block::bordered()
                .border_set(theme.plain_border())
                .border_style(normal)
                .title_top(
                    Line::from(Span::styled(format!(" {} ", self.title), title_style)).centered(),
                )
                .style(normal),
            framed,
        );

        let areas = self.widget_areas(outer);
        for (i, (w, area)) in self.widgets.iter().zip(&areas).enumerate() {
            if area.height == 0 || area.width == 0 {
                continue;
            }
            let focused = i == self.focus;
            let (st, st_hot) = if focused {
                (focus, focus_hot)
            } else {
                (normal, hot)
            };
            let label_line = Rect { height: 1, ..*area };
            let value = w.value_rect(*area);
            match &w.kind {
                WidgetKind::Label => {
                    let lines: Vec<Line> = w
                        .label
                        .text
                        .lines()
                        .map(|l| Line::from(Span::styled(l.to_owned(), normal)))
                        .collect();
                    frame.render_widget(Paragraph::new(lines), *area);
                }
                WidgetKind::Checkbox { checked } => {
                    let mut spans = vec![Span::styled(check_mark(*checked), st)];
                    spans.extend(w.label.spans(st, st_hot));
                    frame.render_widget(Paragraph::new(Line::from(spans)), *area);
                }
                WidgetKind::Button { .. } => {
                    let (l, r) = w.button_text();
                    let mut spans = vec![Span::styled(l, st)];
                    spans.extend(w.label.spans(st, st_hot));
                    spans.push(Span::styled(r, st));
                    frame.render_widget(Paragraph::new(Line::from(spans)), *area);
                }
                WidgetKind::Input {
                    value: text,
                    cursor,
                    masked,
                    ..
                } => {
                    frame.render_widget(
                        Paragraph::new(Line::from(w.label.spans(normal, hot))),
                        label_line,
                    );
                    if value.height == 0 || value.width == 0 {
                        continue;
                    }
                    let style = if focused {
                        theme.get(Role::DialogInputFocus)
                    } else {
                        theme.get(Role::DialogInput)
                    };
                    let field = Rect { height: 1, ..value };
                    let cursor_x = draw_input(frame, field, text, *cursor, *masked, style);
                    if focused {
                        let pos = Position::new(field.x + cursor_x, field.y);
                        frame.set_cursor_position(pos);
                        show_cursor_cell(frame, pos);
                    }
                }
                WidgetKind::Combo {
                    items, selected, ..
                } => {
                    frame.render_widget(
                        Paragraph::new(Line::from(w.label.spans(normal, hot))),
                        label_line,
                    );
                    let style = if focused {
                        theme.get(Role::DialogInputFocus)
                    } else {
                        theme.get(Role::DialogInput)
                    };
                    let item = items.get(*selected).map_or("", String::as_str);
                    let text = combo::closed_text(item, value.width);
                    frame.render_widget(Paragraph::new(text).style(style), value);
                }
                WidgetKind::List {
                    items,
                    highlight,
                    selected,
                    offset,
                    rows,
                    ..
                } => {
                    if w.has_label_row() {
                        frame.render_widget(
                            Paragraph::new(Line::from(w.label.spans(normal, hot))),
                            label_line,
                        );
                    }
                    let input = theme.get(Role::DialogInput);
                    let width = value.width as usize;
                    let lines: Vec<Line> = items
                        .iter()
                        .enumerate()
                        .skip(*offset)
                        .take(*rows as usize)
                        .map(|(n, item)| {
                            let style = if n != *selected {
                                if highlight.get(n).copied().unwrap_or(false) {
                                    // bold in the list's own colours: always readable
                                    input.add_modifier(ratatui::style::Modifier::BOLD)
                                } else {
                                    input
                                }
                            } else if focused {
                                focus
                            } else {
                                theme.get(Role::CoreSelectedUnfocused)
                            };
                            let text: String = format!(" {item}").chars().take(width).collect();
                            Line::from(Span::styled(format!("{text:<width$}"), style))
                        })
                        .collect();
                    frame.render_widget(Paragraph::new(lines).style(input), value);
                }
                WidgetKind::Memo { editor, .. } => {
                    let mut title = vec![Span::styled(" ", normal)];
                    title.extend(w.label.spans(
                        if focused { focus } else { normal },
                        if focused { focus_hot } else { hot },
                    ));
                    title.push(Span::styled(" ", normal));
                    frame.render_widget(
                        Block::bordered()
                            .border_set(theme.plain_border())
                            .border_style(normal)
                            .title_top(Line::from(title))
                            .style(normal),
                        *area,
                    );
                    // plain text, wrapped at word ends: a prompt is prose, not SQL
                    let text_style = theme.get(Role::EditorText);
                    let selected = theme.get(Role::EditorSelection);
                    let e = &editor.0;
                    let (inner, row, col, offset) = memo_layout(e, *area);
                    frame.render_widget(Block::new().style(text_style), inner);
                    let chars: Vec<char> = e.text().chars().collect();
                    let rows = wrap_rows(&chars, inner.width as usize);
                    let selection = e.selection();
                    let lines: Vec<Line> = rows
                        .iter()
                        .skip(offset)
                        .take(inner.height as usize)
                        .map(|&(from, to)| {
                            let spans: Vec<Span> = (from..to)
                                .map(|i| {
                                    let st = if selection.is_some_and(|(a, b)| a <= i && i < b) {
                                        selected
                                    } else {
                                        text_style
                                    };
                                    Span::styled(chars[i].to_string(), st)
                                })
                                .collect();
                            Line::from(spans)
                        })
                        .collect();
                    frame.render_widget(Paragraph::new(lines).style(text_style), inner);
                    if focused && inner.width > 0 && inner.height > 0 {
                        let pos = Position::new(
                            inner.x + (col as u16).min(inner.width - 1),
                            inner.y + (row - offset) as u16,
                        );
                        frame.set_cursor_position(pos);
                        show_cursor_cell(frame, pos);
                    }
                }
                WidgetKind::Text {
                    lines,
                    headers,
                    offset,
                    ..
                } => {
                    // the last column holds the scroll marks
                    let width = area.width.saturating_sub(1) as usize;
                    let shown: Vec<Line> = lines
                        .iter()
                        .enumerate()
                        .skip(*offset)
                        .take(area.height as usize)
                        .map(|(n, l)| {
                            let style = if headers.get(n).copied().unwrap_or(false) {
                                hot
                            } else {
                                normal
                            };
                            let text: String = l.chars().take(width).collect();
                            Line::from(Span::styled(text, style))
                        })
                        .collect();
                    frame.render_widget(Paragraph::new(shown).style(normal), *area);
                    let mark_x = area.right() - 1;
                    let mark = if focused { focus } else { normal };
                    if *offset > 0 {
                        frame.render_widget(
                            Paragraph::new("▲").style(mark),
                            Rect::new(mark_x, area.y, 1, 1),
                        );
                    }
                    if *offset + (area.height as usize) < lines.len() {
                        frame.render_widget(
                            Paragraph::new("▼").style(mark),
                            Rect::new(mark_x, area.bottom() - 1, 1, 1),
                        );
                    }
                }
            }
        }
        // the terminal cursor on the focused control (inputs placed it already)
        if let Some(pos) = self.focus_cursor(&areas) {
            frame.set_cursor_position(pos);
        }
        // An open combo list is drawn last, over the widgets below it.
        if let Some(area) = self.combo_list_area(outer, screen)
            && let WidgetKind::Combo {
                items,
                open: Some(list),
                ..
            } = &self.widgets[self.focus].kind
        {
            list.render(frame, area, items, theme);
            frame.set_cursor_position(list.cursor(area));
        }
        (outer, areas)
    }

    /// Cursor position on the focused control without a text cursor (as in mc):
    /// inside `[ ]`, on a combo's value, on a button's label, on a list's row.
    fn focus_cursor(&self, areas: &[Rect]) -> Option<Position> {
        let w = self.widgets.get(self.focus)?;
        let area = *areas.get(self.focus)?;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let value = w.value_rect(area);
        let at = |r: Rect, dx: u16, dy: u16| {
            Position::new(r.x + dx.min(r.width.saturating_sub(1)), r.y + dy)
        };
        Some(match &w.kind {
            WidgetKind::Input { .. }
            | WidgetKind::Label
            | WidgetKind::Text { .. }
            | WidgetKind::Memo { .. } => return None,
            WidgetKind::Checkbox { .. } => at(area, 1, 0),
            WidgetKind::Combo { .. } => at(value, 1, 0),
            WidgetKind::Button { .. } => at(area, w.button_text().0.len() as u16, 0),
            WidgetKind::List {
                selected, offset, ..
            } => {
                let row = selected.saturating_sub(*offset) as u16;
                at(value, 1, row.min(value.height.saturating_sub(1)))
            }
        })
    }
}

/// The rows of a memo `width` columns wide: (first char, end char) of each, the
/// lines broken after a space where they can be (one column stays free for the
/// cursor at the end of a row).
fn wrap_rows(chars: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.saturating_sub(1).max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    loop {
        let end = chars[start..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(chars.len(), |n| start + n);
        let mut pos = start;
        while end - pos > width {
            let cut = chars[pos..pos + width]
                .iter()
                .rposition(|&c| c == ' ')
                .filter(|&n| n > 0)
                .map_or(pos + width, |n| pos + n + 1);
            rows.push((pos, cut));
            pos = cut;
        }
        rows.push((pos, end));
        if end >= chars.len() {
            return rows;
        }
        start = end + 1;
    }
}

/// The text area inside a memo's frame `area`, the cursor's row and column there, and
/// the first row shown (the cursor stays visible).
fn memo_layout(e: &Editor, area: Rect) -> (Rect, usize, usize, usize) {
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let chars: Vec<char> = e.text().chars().collect();
    let rows = wrap_rows(&chars, inner.width as usize);
    let (row, col) = memo_cursor(&rows, e.cursor());
    let offset = row.saturating_sub(inner.height.saturating_sub(1) as usize);
    (inner, row, col, offset)
}

/// Row and column of char index `cursor` in the rows of [`wrap_rows`].
fn memo_cursor(rows: &[(usize, usize)], cursor: usize) -> (usize, usize) {
    let row = rows
        .iter()
        .rposition(|&(from, _)| from <= cursor)
        .unwrap_or(0);
    (row, cursor - rows.get(row).map_or(0, |r| r.0))
}

/// Scrolls a text widget by `delta` lines, keeping the last page full.
fn scroll_text(kind: &mut WidgetKind, delta: isize) {
    if let WidgetKind::Text {
        lines,
        offset,
        rows,
        ..
    } = kind
    {
        let max = lines.len().saturating_sub(*rows as usize) as isize;
        *offset = (*offset as isize).saturating_add(delta).clamp(0, max) as usize;
    }
}

/// Draws the cell under the text cursor in reverse video. The terminal's own cursor
/// colour may match the background (e.g. a white cursor on a white editor), so the
/// cursor is made visible in every colour scheme this way as well.
pub fn show_cursor_cell(frame: &mut Frame, pos: Position) {
    let area = frame.area();
    if pos.x < area.right() && pos.y < area.bottom() {
        frame.buffer_mut()[(pos.x, pos.y)]
            .modifier
            .toggle(ratatui::style::Modifier::REVERSED);
    }
}

/// The width of the widest line of `text`, in characters.
fn widest_line(text: &str) -> u16 {
    text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16
}

// ---- what a field looks like -----------------------------------------------------
// The pieces every field is made of, here and in the developer template form
// (crate::template_dialog), which lays its fields out itself.

/// What stands left and right of a button's label; the default button has `< >`.
pub(crate) fn button_frame(default: bool) -> (&'static str, &'static str) {
    if default {
        ("[< ", " >]")
    } else {
        ("[ ", " ]")
    }
}

/// The box of a check box, before its label.
pub(crate) fn check_mark(checked: bool) -> &'static str {
    if checked { "[x] " } else { "[ ] " }
}

/// Draws the one row `field` of a text input: the part of `text` that is in view with
/// the cursor at char `cursor`, on the input's colour. Returns the cursor's cell,
/// from the left edge of `field`.
pub(crate) fn draw_input(
    frame: &mut Frame,
    field: Rect,
    text: &str,
    cursor: usize,
    masked: bool,
    style: Style,
) -> u16 {
    let view = input_view(text, cursor, field.width, masked);
    frame.render_widget(Paragraph::new(view.shown).style(style), field);
    view.cursor_x
}

/// What an input shows of its text.
pub(crate) struct InputView {
    /// Char index of the first character shown.
    pub from: usize,
    pub shown: String,
    /// The cell of the cursor, from the left edge of the input.
    pub cursor_x: u16,
}

/// The cells a character takes in an input (`masked`: it is drawn as `*`).
fn input_cells(c: char, masked: bool) -> usize {
    if masked {
        1
    } else {
        unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
    }
}

/// What an input `width` cells wide shows of `text` with the cursor at char `cursor`,
/// counted in cells: a wide character (CJK, emoji) takes two, a combining one none.
/// The text scrolls so that the cursor has a cell.
pub(crate) fn input_view(text: &str, cursor: usize, width: u16, masked: bool) -> InputView {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let cells = |from: usize, to: usize| -> usize {
        chars[from..to]
            .iter()
            .map(|c| input_cells(*c, masked))
            .sum()
    };
    let width = width as usize;
    // as few characters hidden on the left as leave the cursor a cell
    let mut from = cursor.saturating_sub(width.saturating_sub(1));
    while from < cursor && cells(from, cursor) + 1 > width {
        from += 1;
    }
    let mut shown = String::new();
    let mut used = 0;
    for c in &chars[from..] {
        let w = input_cells(*c, masked);
        if used + w > width {
            break;
        }
        used += w;
        shown.push(if masked { '*' } else { *c });
    }
    InputView {
        from,
        shown,
        cursor_x: cells(from, cursor) as u16,
    }
}

/// The char index of the character at cell `x` of an input that shows `text` from
/// char `from` (a click); behind the text: its end.
pub(crate) fn input_char_at(text: &str, from: usize, x: u16, masked: bool) -> usize {
    let mut used = 0;
    for (i, c) in text.chars().enumerate().skip(from) {
        used += input_cells(c, masked);
        if used > x as usize {
            return i;
        }
    }
    text.chars().count()
}

/// One line of text with a cursor (a char index): the editing that the inputs of the
/// dialogs and the text fields of the developer template form have in common.
pub(crate) mod line {
    fn byte_index(s: &str, char_index: usize) -> usize {
        s.char_indices().nth(char_index).map_or(s.len(), |(b, _)| b)
    }

    /// Puts `new` in at the cursor, which goes behind it.
    pub fn insert(text: &mut String, cursor: &mut usize, new: &str) {
        text.insert_str(byte_index(text, *cursor), new);
        *cursor += new.chars().count();
    }

    /// Removes the character before the cursor.
    pub fn backspace(text: &mut String, cursor: &mut usize) {
        if *cursor > 0 {
            *cursor -= 1;
            delete(text, *cursor);
        }
    }

    /// Removes the character at `cursor`, when there is one.
    pub fn delete(text: &mut String, cursor: usize) {
        if cursor < text.chars().count() {
            text.remove(byte_index(text, cursor));
        }
    }

    /// Moves the cursor as `cmd` does (`Left`, `Right`, `Home`, `End`); false for any
    /// other command.
    pub fn step(cmd: crate::keymap::Command, text: &str, cursor: &mut usize) -> bool {
        use crate::keymap::Command;
        let len = text.chars().count();
        *cursor = match cmd {
            Command::Left => cursor.saturating_sub(1),
            Command::Right => (*cursor + 1).min(len),
            Command::Home => 0,
            Command::End => len,
            _ => return false,
        };
        true
    }
}

pub(crate) fn draw_shadow(frame: &mut Frame, area: Rect, style: Style) {
    let screen = frame.area();
    let buf = frame.buffer_mut();
    let right = area.x + area.width;
    let bottom = area.y + area.height;
    for y in area.y + 1..=bottom {
        for x in [right, right + 1] {
            if x < screen.right() && y < screen.bottom() {
                buf[(x, y)].set_style(style);
            }
        }
    }
    if bottom < screen.bottom() {
        for x in area.x + 2..right {
            if x < screen.right() {
                buf[(x, bottom)].set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An input counts in cells: wide characters take two, combining ones none.
    #[test]
    fn inputs_count_display_cells() {
        // latin: as before
        let v = input_view("hello", 5, 10, false);
        assert_eq!((v.from, v.shown.as_str(), v.cursor_x), (0, "hello", 5));
        // wide characters: the cursor is two cells further for each
        let v = input_view("日本語", 2, 10, false);
        assert_eq!((v.shown.as_str(), v.cursor_x), ("日本語", 4));
        // only what fits whole is shown, and the cursor keeps a cell
        let v = input_view("日本語abc", 6, 5, false);
        assert_eq!((v.from, v.shown.as_str(), v.cursor_x), (3, "abc", 3));
        let v = input_view("日本語abc", 3, 5, false);
        assert_eq!((v.from, v.shown.as_str(), v.cursor_x), (1, "本語a", 4));
        // a combining mark takes no cell
        let v = input_view("e\u{301}x", 3, 10, false);
        assert_eq!(v.cursor_x, 2);
        // masked: one star a character
        let v = input_view("日本", 2, 10, true);
        assert_eq!((v.shown.as_str(), v.cursor_x), ("**", 2));
        // a click: the character at that cell
        assert_eq!(input_char_at("日本語", 0, 0, false), 0);
        assert_eq!(input_char_at("日本語", 0, 1, false), 0);
        assert_eq!(input_char_at("日本語", 0, 2, false), 1);
        assert_eq!(input_char_at("abc", 0, 9, false), 3);
    }
    use crate::keymap::{Context, Keymap};
    use crate::keys::KeyCombo;

    fn press(d: &mut Dialog, key: &str) -> DialogEvent {
        let combo = KeyCombo::parse(key).unwrap();
        let ev = KeyEvent::new(combo.code, combo.mods);
        let cmd = Keymap::default().lookup_only(Context::Dialog, combo);
        d.handle_key(cmd, &ev)
    }

    fn sample() -> Dialog {
        Dialog::new(
            DialogKind::Settings,
            "Test",
            40,
            vec![
                Widget::field("name", "&Name:", "ab", 8),
                Widget::checkbox("flag", "&Flag", false),
                Widget::list(
                    "scheme",
                    "&Scheme:",
                    vec!["A".into(), "B".into(), "C".into()],
                    0,
                    2,
                ),
                Widget::combo("mode", "&Mode:", vec!["x".into(), "y".into()], 0, 8),
                Widget::button(Button::Ok, "&OK", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        )
    }

    #[test]
    fn typing_goes_into_input() {
        let mut d = sample();
        press(&mut d, "X");
        press(&mut d, "Left");
        press(&mut d, "Backspace");
        assert_eq!(d.text("name"), Some("ax"));
        d.paste("1\n2");
        assert_eq!(d.text("name"), Some("a1 2x"));
    }

    #[test]
    fn tab_moves_focus_and_space_toggles() {
        let mut d = sample();
        press(&mut d, "Tab");
        assert_eq!(d.focus, 1);
        press(&mut d, "Space");
        assert!(d.checked("flag"));
        press(&mut d, "Shift+Tab");
        assert_eq!(d.focus, 0);
    }

    #[test]
    fn hotkeys_lists_and_choices() {
        let mut d = sample();
        press(&mut d, "Alt+F");
        assert!(d.checked("flag"));
        assert_eq!(d.focus, 1);
        press(&mut d, "s"); // plain hotkey letter works outside inputs
        assert_eq!(d.focus, 2);
        press(&mut d, "Down");
        press(&mut d, "Down");
        press(&mut d, "Down");
        assert_eq!(d.selected("scheme"), Some(2));
        press(&mut d, "m");
        press(&mut d, "Right");
        assert_eq!(d.selected("mode"), Some(1));
        press(&mut d, "Space"); // opens the list
        assert!(d.combo_open());
        press(&mut d, "Up");
        press(&mut d, "Enter");
        assert_eq!(d.selected("mode"), Some(0));
        press(&mut d, "Tab"); // Enter on the combo would open its list again
        assert_eq!(press(&mut d, "Enter"), DialogEvent::Button(Button::Ok));
        assert_eq!(press(&mut d, "c"), DialogEvent::Button(Button::Cancel));
        assert_eq!(press(&mut d, "Esc"), DialogEvent::Cancelled);
        // Ctrl+Enter: the default button from anywhere, also with a combo list open
        d.focus = d.widgets.iter().position(|w| w.id == "cancel").unwrap();
        assert_eq!(press(&mut d, "Ctrl+Enter"), DialogEvent::Button(Button::Ok));
        d.focus = d.widgets.iter().position(|w| w.id == "mode").unwrap();
        press(&mut d, "Space");
        press(&mut d, "Down");
        assert_eq!(press(&mut d, "Ctrl+Enter"), DialogEvent::Button(Button::Ok));
        assert!(!d.combo_open());
        assert_eq!(d.selected("mode"), Some(1));
    }

    #[test]
    fn combo_opens_selects_and_cancels() {
        let mut d = Dialog::new(
            DialogKind::Settings,
            "Test",
            40,
            vec![
                Widget::combo(
                    "c",
                    "&Colour:",
                    vec!["A".into(), "B".into(), "C".into()],
                    0,
                    10,
                ),
                Widget::button(Button::Ok, "&OK", true),
            ],
        );
        press(&mut d, "Right"); // closed: Left/Right change the value
        assert_eq!(d.selected("c"), Some(1));
        press(&mut d, "Space");
        assert!(d.combo_open());
        press(&mut d, "Down");
        assert_eq!(
            press(&mut d, "Esc"),
            DialogEvent::None,
            "Esc closes only the list"
        );
        assert!(!d.combo_open());
        assert_eq!(d.selected("c"), Some(1));
        press(&mut d, "Space");
        press(&mut d, "Down");
        assert_eq!(
            press(&mut d, "Enter"),
            DialogEvent::None,
            "Enter picks the item"
        );
        assert_eq!(d.selected("c"), Some(2));
        // Enter on a closed combo opens its list (it does not press OK)
        assert_eq!(press(&mut d, "Enter"), DialogEvent::None);
        assert!(d.combo_open());
        press(&mut d, "Tab"); // closes the list and moves on
        assert!(!d.combo_open());
        assert_eq!(press(&mut d, "Enter"), DialogEvent::Button(Button::Ok));
        // mouse: open with a click on the field, pick the first row
        let screen = Rect::new(0, 0, 80, 25);
        let outer = d.area(screen);
        let field = d.widget_areas(outer)[0];
        d.click(0, field, field.x + 12, field.y, false);
        assert!(d.combo_open());
        let list = d.combo_list_area(outer, screen).unwrap();
        assert!(
            d.combo_click(outer, screen, list.x + 2, list.y + 1),
            "first row"
        );
        assert_eq!(d.selected("c"), Some(0));
        assert!(!d.combo_open());
    }

    #[test]
    fn layout_fits_inside_dialog() {
        let d = sample();
        let outer = d.area(Rect::new(0, 0, 80, 25));
        let areas = d.widget_areas(outer);
        for a in &areas {
            assert!(a.x >= outer.x && a.right() <= outer.right());
            assert!(a.y >= outer.y && a.bottom() <= outer.bottom());
        }
        assert_eq!(areas[0].height, 1, "inline label");
        assert_eq!(areas[4].y, areas[5].y, "buttons share a row");
    }

    #[test]
    fn memo_rows_wrap_at_spaces() {
        let chars: Vec<char> = "total due per customer
by id"
            .chars()
            .collect();
        // 11 columns: 10 for text, one for the cursor
        let rows = wrap_rows(&chars, 11);
        let text = |r: &(usize, usize)| chars[r.0..r.1].iter().collect::<String>();
        let shown: Vec<String> = rows.iter().map(text).collect();
        assert_eq!(shown, ["total due ", "per ", "customer", "by id"]);
        assert_eq!(memo_cursor(&rows, 0), (0, 0));
        // the end of a wrapped row is the start of the next one
        assert_eq!(memo_cursor(&rows, 10), (1, 0));
        // the end of a line stays on it
        assert_eq!(memo_cursor(&rows, 22), (2, 8));
        assert_eq!(memo_cursor(&rows, chars.len()), (3, 5));
        // a word longer than the row is cut
        let long: Vec<char> = "abcdefghijkl".chars().collect();
        assert_eq!(wrap_rows(&long, 6), [(0, 5), (5, 10), (10, 12)]);
        assert_eq!(wrap_rows(&[], 6), [(0, 0)]);
    }
}
