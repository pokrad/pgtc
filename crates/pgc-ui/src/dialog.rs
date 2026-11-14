//! mc-style dialog framework (D-016): grey box with a frame, labels, inputs,
//! checkboxes, lists, choices and `[< OK >]`-style buttons.
//!
//! Widgets are stacked vertically; all buttons go into one centred row at the bottom.
//! Keys arrive as keymap [`Command`]s from the `[dialog]` context; typed characters go
//! into the focused input; hotkey letters (`Alt+letter`, or the plain letter when the
//! focus is not in an input) activate the widget with that letter.

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

/// What the dialog is for; the app uses it (and [`Dialog::tag`]) to handle the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Message,
    Settings,
    Connection,
    Login,
    ConfirmDelete,
    Find,
    ReplaceConfirm,
    /// Close a query tab with unsaved text (D-019).
    SaveChanges,
    /// Quit with a query tab with unsaved text (D-019).
    QuitSaveChanges,
    /// Close a query tab with an open transaction (D-019).
    CloseTransaction,
    /// Quit with open transactions (D-019).
    QuitTransactions,
    /// The server did not react to a cancel request (D-041).
    CancelWait,
    OpenFile,
    SaveFile,
    ConfirmOverwrite,
    /// Export result (D-042).
    ExportFile,
    ConfirmExportOverwrite,
    /// Developer templates (D-046): the picker and a row of a list parameter.
    TemplatePicker,
    TemplateRow,
    /// Export / load database definition (D-048).
    SaveDefinition,
    OpenDefinition,
    ConfirmDefinitionOverwrite,
    /// Install templates into an existing templates folder (D-013).
    ConfirmTemplatesOverwrite,
    /// Help windows: scrolling text with a Close button (D-014).
    Viewer,
    /// psql-style command line (D-054).
    PsqlCommand,
    /// Find object in the tree (D-055).
    FindObject,
    /// Dashboard: cancel query / terminate session of a session (D-057).
    ConfirmCancelBackend,
    ConfirmTerminateBackend,
    /// SQL profiler: change the logging settings? and the filter (D-058).
    ConfirmProfilerSettings,
    ProfilerFilter,
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

    pub fn button(id: &'static str, label: &str, default: bool) -> Self {
        Self {
            id,
            label: HotText::new(label),
            kind: WidgetKind::Button { default },
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
        match self.kind {
            WidgetKind::Button { default: true } => ("[< ".into(), " >]".into()),
            _ => ("[ ".into(), " ]".into()),
        }
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
    /// Button with this id pressed.
    Button(&'static str),
}

/// Caller data a dialog carries until it is answered. It goes with the dialog, so a
/// cancelled question leaves nothing behind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Payload {
    #[default]
    None,
    /// The file a question is about (overwrite it?).
    Path(PathBuf),
    /// Result export (D-042): the file and the chosen options.
    Export(PathBuf, ExportOptions),
    /// Definition export / load (D-048): server uid and database, and the file a
    /// question is about.
    Database {
        uid: u64,
        database: String,
        path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone)]
pub struct Dialog {
    pub kind: DialogKind,
    /// Caller data, e.g. the server the dialog is about.
    pub tag: u64,
    pub payload: Payload,
    pub title: String,
    pub widgets: Vec<Widget>,
    pub focus: usize,
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
            tag: 0,
            payload: Payload::None,
            title: title.to_owned(),
            widgets,
            focus,
            width,
            error: false,
        }
    }

    pub fn with_tag(mut self, tag: u64) -> Self {
        self.tag = tag;
        self
    }

    pub fn with_payload(mut self, payload: Payload) -> Self {
        self.payload = payload;
        self
    }

    /// A message box with an OK button.
    pub fn message(title: &str, text: &str, error: bool) -> Self {
        let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
        let width = width.max(title.chars().count() as u16 + 4).max(20) + CHROME_X;
        let mut d = Self::new(
            DialogKind::Message,
            title,
            width,
            vec![Widget::label(text), Widget::button("ok", "&OK", true)],
        );
        d.error = error;
        d
    }

    /// A question with Yes / No buttons (`yes` / `no`).
    pub fn confirm(kind: DialogKind, title: &str, text: &str) -> Self {
        let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
        let width = width.max(24) + CHROME_X;
        let mut d = Self::new(
            kind,
            title,
            width,
            vec![
                Widget::label(text),
                Widget::button("yes", "&Yes", true),
                Widget::button("no", "&No", false),
            ],
        );
        d.error = true;
        d.focus = 1;
        d
    }

    /// A question with the given buttons (`(id, label)`; the first is the default).
    pub fn confirm_buttons(
        kind: DialogKind,
        title: &str,
        text: &str,
        buttons: &[(&'static str, &str)],
    ) -> Self {
        let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
        let buttons_width: u16 = buttons
            .iter()
            .map(|(_, l)| l.chars().count() as u16 + 5)
            .sum();
        let width = width.max(buttons_width).max(24) + CHROME_X;
        let mut widgets = vec![Widget::label(text)];
        for (i, (id, label)) in buttons.iter().enumerate() {
            widgets.push(Widget::button(id, label, i == 0));
        }
        let mut d = Self::new(kind, title, width, widgets);
        d.focus = 1;
        d
    }

    /// Id of the focused widget.
    pub fn focused_id(&self) -> Option<&'static str> {
        self.widgets.get(self.focus).map(|w| w.id)
    }

    /// Moves the focus to the widget `id`.
    pub fn focus_on(&mut self, id: &str) {
        if let Some(i) = self.widgets.iter().position(|w| w.id == id) {
            self.focus = i;
        }
    }

    // ---- values -------------------------------------------------------------------

    fn find(&self, id: &str) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }

    pub fn checked(&self, id: &str) -> bool {
        matches!(
            self.find(id).map(|w| &w.kind),
            Some(WidgetKind::Checkbox { checked: true })
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
        }) = self
            .widgets
            .iter_mut()
            .find(|w| w.id == id)
            .map(|w| &mut w.kind)
        {
            *selected = new_selected.min(new_items.len().saturating_sub(1));
            *offset = selected.saturating_sub(*rows as usize - 1);
            *items = new_items;
            *highlight = marks;
        }
    }

    /// Replaces the text of an input (cursor at the end).
    pub fn set_text(&mut self, id: &str, text: &str) {
        if let Some(WidgetKind::Input { value, cursor, .. }) = self
            .widgets
            .iter_mut()
            .find(|w| w.id == id)
            .map(|w| &mut w.kind)
        {
            *value = text.to_owned();
            *cursor = text.chars().count();
        }
    }

    /// Selected index of a list or choice.
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
            .find(|w| matches!(w.kind, WidgetKind::Button { default: true }))
        {
            Some(w) => DialogEvent::Button(w.id),
            None => DialogEvent::None,
        }
    }

    pub fn handle_key(&mut self, cmd: Option<Command>, ev: &KeyEvent) -> DialogEvent {
        // Ctrl+Enter: the default button from any field; an open combo list first
        // takes its highlighted item (D-016)
        if cmd == Some(Command::DialogOk) {
            if self.combo_open() {
                self.combo_key(Some(Command::Accept), ev);
            }
            return match self
                .widgets
                .iter()
                .find(|w| matches!(w.kind, WidgetKind::Button { default: true }))
            {
                Some(w) => DialogEvent::Button(w.id),
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
                if !self.input_cursor(|c, _| c.saturating_sub(1)) && !self.choice_move(-1) {
                    self.move_focus(-1);
                }
            }
            Command::Right => {
                if !self.input_cursor(|c, len| (c + 1).min(len)) && !self.choice_move(1) {
                    self.move_focus(1);
                }
            }
            Command::Home => {
                if !self.input_cursor(|_, _| 0) {
                    self.list_move(isize::MIN / 2);
                }
            }
            Command::End => {
                if !self.input_cursor(|_, len| len) {
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
                if self.widgets[self.focus].is_button() {
                    return DialogEvent::Button(self.widgets[self.focus].id);
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

    /// Bracketed paste: into the focused input, newlines replaced by spaces.
    pub fn paste(&mut self, text: &str) {
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
            WidgetKind::Button { .. } => return DialogEvent::Button(w.id),
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
                ..
            } = &mut w.kind
            {
                let label = u16::from(!w.label.text.is_empty());
                *rows = (items.len() as u16).clamp(1, free.saturating_sub(label).max(1));
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
        if let Some(i) = self.widgets.iter().position(|w| w.id == id) {
            let focus = std::mem::replace(&mut self.focus, i);
            if let WidgetKind::List { selected, .. } = &self.widgets[i].kind {
                let delta = index as isize - *selected as isize;
                self.list_move(delta);
            }
            self.focus = focus;
        }
    }

    /// Moves the selection of the list `id` by `delta` rows, whatever has the focus.
    pub fn list_step(&mut self, id: &str, delta: isize) {
        if let Some(i) = self.widgets.iter().position(|w| w.id == id) {
            let focus = std::mem::replace(&mut self.focus, i);
            self.list_move(delta);
            self.focus = focus;
        }
    }

    /// Mouse wheel: scrolls the selection of the (first) list; false if there is none.
    pub fn list_wheel(&mut self, delta: isize) -> bool {
        let Some(i) = self
            .widgets
            .iter()
            .position(|w| matches!(w.kind, WidgetKind::List { .. }))
        else {
            return false;
        };
        let focus = std::mem::replace(&mut self.focus, i);
        self.list_move(delta);
        self.focus = focus;
        true
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
        if matches!(self.widgets[self.focus].kind, WidgetKind::Text { .. }) {
            scroll_text(&mut self.widgets[self.focus].kind, delta);
            return true;
        }
        let WidgetKind::List {
            items,
            selected,
            offset,
            rows,
            ..
        } = &mut self.widgets[self.focus].kind
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

    /// Moves the cursor of the focused input; false if the focus is not an input.
    fn input_cursor(&mut self, f: impl Fn(usize, usize) -> usize) -> bool {
        let WidgetKind::Input { value, cursor, .. } = &mut self.widgets[self.focus].kind else {
            return false;
        };
        *cursor = f(*cursor, value.chars().count());
        true
    }

    fn insert_text(&mut self, text: &str) {
        if let WidgetKind::Input { value, cursor, .. } = &mut self.widgets[self.focus].kind {
            let at = byte_index(value, *cursor);
            value.insert_str(at, text);
            *cursor += text.chars().count();
        }
    }

    fn input_edit(&mut self, backspace: bool) {
        if let WidgetKind::Input { value, cursor, .. } = &mut self.widgets[self.focus].kind {
            let pos = if backspace {
                if *cursor == 0 {
                    return;
                }
                *cursor -= 1;
                *cursor
            } else {
                *cursor
            };
            if pos < value.chars().count() {
                let at = byte_index(value, pos);
                value.remove(at);
            }
        }
    }

    // ---- mouse --------------------------------------------------------------------

    /// Left click at (`x`, `y`) inside widget `i` whose area is `area`; `double`: the
    /// second click of a double click, which on a list row acts as `Enter` (D-067).
    pub fn click(&mut self, i: usize, area: Rect, x: u16, y: u16, double: bool) -> DialogEvent {
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
                ..
            } => {
                self.focus = i;
                let len = text.chars().count();
                let visible_from = input_offset(*cursor, value.width);
                *cursor = (visible_from + x.saturating_sub(value.x) as usize).min(len);
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
                    let mut spans = vec![Span::styled(if *checked { "[x] " } else { "[ ] " }, st)];
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
                    let from = input_offset(*cursor, value.width);
                    let shown: String = text
                        .chars()
                        .skip(from)
                        .take(value.width as usize)
                        .map(|c| if *masked { '*' } else { c })
                        .collect();
                    let field = Rect { height: 1, ..value };
                    frame.render_widget(Paragraph::new(shown).style(style), field);
                    if focused {
                        let pos = Position::new(field.x + (*cursor - from) as u16, field.y);
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
            WidgetKind::Input { .. } | WidgetKind::Label | WidgetKind::Text { .. } => return None,
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

/// First visible char of an input `width` wide with the cursor at `cursor`.
fn input_offset(cursor: usize, width: u16) -> usize {
    cursor.saturating_sub((width as usize).saturating_sub(1))
}

fn byte_index(s: &str, char_index: usize) -> usize {
    s.char_indices().nth(char_index).map_or(s.len(), |(b, _)| b)
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
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
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
        assert_eq!(press(&mut d, "Enter"), DialogEvent::Button("ok"));
        assert_eq!(press(&mut d, "c"), DialogEvent::Button("cancel"));
        assert_eq!(press(&mut d, "Esc"), DialogEvent::Cancelled);
        // Ctrl+Enter: the default button from anywhere, also with a combo list open
        d.focus = d.widgets.iter().position(|w| w.id == "cancel").unwrap();
        assert_eq!(press(&mut d, "Ctrl+Enter"), DialogEvent::Button("ok"));
        d.focus = d.widgets.iter().position(|w| w.id == "mode").unwrap();
        press(&mut d, "Space");
        press(&mut d, "Down");
        assert_eq!(press(&mut d, "Ctrl+Enter"), DialogEvent::Button("ok"));
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
                Widget::button("ok", "&OK", true),
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
        assert_eq!(press(&mut d, "Enter"), DialogEvent::Button("ok"));
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
}
