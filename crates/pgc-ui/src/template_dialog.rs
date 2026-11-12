//! The developer template form (D-013, D-046): every parameter of the template, a live
//! read-only preview of the generated SQL, OK / Cancel.
//!
//! Layout: a dialog window (margin, single frame, shadow) with the form on the left
//! and the preview on the right, split by a line joined to the frame (on narrow
//! screens the preview is below the form); a line above the help line and the
//! buttons. The form scrolls when it is taller than the screen.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgc_editor::{Action, Editor, EditorStyles};
use pgc_sql::devtemplate::{
    self, Param, ParamType, ParamValue, QualName, Shape, TemplateContext, TemplateFile,
};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::combo::{self, ComboList, ListEvent};
use crate::keymap::Command;
use crate::theme::{Role, Theme};

/// Where the parts of the form are drawn ([`TemplateForm::layout`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The window with its margin (the shadow is outside).
    pub outer: Rect,
    /// The frame lines.
    pub frame: Rect,
    /// The parameters (scrolled).
    pub form: Rect,
    /// The generated SQL.
    pub preview: Rect,
    pub split: Split,
    /// Row of the line above the help line.
    pub separator: u16,
    /// Help of the focused field.
    pub help: Rect,
    /// OK / Cancel.
    pub buttons: Rect,
}

/// The line between form and preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    /// Side by side: a vertical line at column `x`.
    Vertical { x: u16 },
    /// Preview below the form: a horizontal line at row `y`.
    Horizontal { y: u16 },
}

/// Rows of an `sql` editor box.
const SQL_ROWS: u16 = 5;
/// Visible rows of a `list` value.
const LIST_ROWS: u16 = 4;
/// Buttons under a `list` value.
pub const LIST_BUTTONS: [&str; 5] = ["Add", "Edit", "Delete", "Up", "Down"];

/// An entry of a choice list.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub label: String,
    pub value: ParamValue,
}

impl AsRef<str> for Choice {
    fn as_ref(&self) -> &str {
        &self.label
    }
}

/// State of a live choice list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceState {
    Ready,
    Loading,
    Failed(String),
}

#[derive(Debug)]
pub enum FieldValue {
    /// text, identifier, number
    Text {
        text: String,
        cursor: usize,
    },
    Bool(bool),
    Choice {
        items: Vec<Choice>,
        selected: Option<usize>,
        state: ChoiceState,
        /// Label to select once the items have loaded (default).
        wanted: Option<String>,
    },
    Sql(Box<Editor>),
    List {
        rows: Vec<BTreeMap<String, ParamValue>>,
        selected: usize,
    },
}

#[derive(Debug)]
pub struct Field {
    pub param: Param,
    pub value: FieldValue,
}

/// Places the focus can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Field(usize),
    /// Button `b` of the list field.
    ListButton(usize, usize),
    Preview,
    Ok,
    Cancel,
}

/// Result of a key or click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEvent {
    None,
    Cancel,
    Ok,
    /// Add a row to list field `i` / edit row `row` of it.
    AddRow(usize),
    EditRow(usize, usize),
}

/// The open list of choice field `field` (the shared combo list, crate::combo).
#[derive(Debug)]
pub struct Popup {
    pub field: usize,
    pub list: ComboList,
}

#[derive(Debug)]
pub struct TemplateForm {
    /// Query tab the result goes into.
    pub tab: u64,
    pub title: String,
    pub file: TemplateFile,
    pub ctx: TemplateContext,
    pub fields: Vec<Field>,
    pub focus: Stop,
    /// First visible form row (adjusted while drawing).
    pub scroll: std::cell::Cell<u16>,
    pub preview: Editor,
    pub error: Option<String>,
    pub popup: Option<Popup>,
    /// Where the open list was drawn (for the mouse).
    pub popup_area: std::cell::Cell<Option<Rect>>,
}

impl TemplateForm {
    pub fn new(
        tab: u64,
        title: String,
        file: TemplateFile,
        ctx: TemplateContext,
        tab_width: usize,
    ) -> Self {
        let fields = file
            .params
            .iter()
            .map(|p| Field {
                value: initial_value(p, tab_width),
                param: p.clone(),
            })
            .collect();
        let preview = Editor::new("", true, tab_width);
        let mut form = Self {
            tab,
            title,
            file,
            ctx,
            fields,
            focus: Stop::Ok,
            scroll: std::cell::Cell::new(0),
            preview,
            error: None,
            popup: None,
            popup_area: std::cell::Cell::new(None),
        };
        form.focus = form.stops().first().copied().unwrap_or(Stop::Ok);
        form.refresh();
        form
    }

    /// The values as entered.
    pub fn values(&self) -> BTreeMap<String, ParamValue> {
        let mut out = BTreeMap::new();
        for f in &self.fields {
            let v = match &f.value {
                FieldValue::Text { text, .. } => Some(ParamValue::Text(text.clone())),
                FieldValue::Bool(b) => Some(ParamValue::Bool(*b)),
                FieldValue::Choice {
                    items, selected, ..
                } => selected.and_then(|i| items.get(i)).map(|c| c.value.clone()),
                FieldValue::Sql(e) => Some(ParamValue::Text(e.text().trim_end().to_owned())),
                FieldValue::List { rows, .. } => Some(ParamValue::List(rows.clone())),
            };
            if let Some(v) = v {
                out.insert(f.param.name.clone(), v);
            }
        }
        out
    }

    /// Renders the template into the preview (or the error).
    pub fn refresh(&mut self) {
        match devtemplate::render(&self.file, &self.values(), &self.ctx) {
            Ok(sql) => {
                self.error = None;
                if self.preview.text() != sql {
                    self.preview.set_text(&sql);
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// The generated SQL, or why there is none.
    pub fn result(&self) -> Result<String, String> {
        devtemplate::render(&self.file, &self.values(), &self.ctx)
    }

    /// Sets the items of the choice field `name` once loaded (`Err`: why not).
    pub fn set_choices(&mut self, name: &str, result: Result<Vec<Choice>, String>) {
        let Some(f) = self.fields.iter_mut().find(|f| f.param.name == name) else {
            return;
        };
        if let FieldValue::Choice {
            items,
            selected,
            state,
            wanted,
        } = &mut f.value
        {
            match result {
                Ok(list) => {
                    *items = list;
                    *state = ChoiceState::Ready;
                    *selected = wanted
                        .take()
                        .and_then(|w| items.iter().position(|c| c.label == w));
                }
                Err(e) => *state = ChoiceState::Failed(e),
            }
        }
        self.refresh();
    }

    /// Prefers `label` as the default of choice field `name` (from the tree selection).
    pub fn want(&mut self, name: &str, label: String) {
        if let Some(Field {
            value: FieldValue::Choice { wanted, .. },
            ..
        }) = self.fields.iter_mut().find(|f| f.param.name == name)
        {
            *wanted = Some(label);
        }
    }

    pub fn list_rows_mut(
        &mut self,
        field: usize,
    ) -> Option<&mut Vec<BTreeMap<String, ParamValue>>> {
        match &mut self.fields.get_mut(field)?.value {
            FieldValue::List { rows, .. } => Some(rows),
            _ => None,
        }
    }

    pub fn list_select(&mut self, field: usize, row: usize) {
        if let Some(Field {
            value: FieldValue::List { selected, .. },
            ..
        }) = self.fields.get_mut(field)
        {
            *selected = row;
        }
    }

    /// Focus stops in order.
    pub fn stops(&self) -> Vec<Stop> {
        let mut out = Vec::new();
        for (i, f) in self.fields.iter().enumerate() {
            out.push(Stop::Field(i));
            if matches!(f.value, FieldValue::List { .. }) {
                out.extend((0..LIST_BUTTONS.len()).map(|b| Stop::ListButton(i, b)));
            }
        }
        out.extend([Stop::Preview, Stop::Ok, Stop::Cancel]);
        out
    }

    fn move_focus(&mut self, d: isize) {
        let stops = self.stops();
        let i = stops.iter().position(|s| *s == self.focus).unwrap_or(0) as isize;
        let n = stops.len() as isize;
        self.focus = stops[((i + d).rem_euclid(n)) as usize];
    }

    /// The focused field is an `sql` editor box (keys go to the editor).
    pub fn editing_sql(&self) -> bool {
        matches!(
            self.focus,
            Stop::Field(i) if matches!(self.fields[i].value, FieldValue::Sql(_))
        )
    }

    // ---- keys ---------------------------------------------------------------------

    /// `cmd`: the key in the dialog context; `editor`: the editor action for the key
    /// (used when an `sql` box has the focus).
    pub fn handle_key(
        &mut self,
        cmd: Option<Command>,
        editor: Option<Action>,
        ev: &KeyEvent,
    ) -> FormEvent {
        if self.popup.is_some() {
            self.popup_key(cmd, ev);
            return FormEvent::None;
        }
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        match cmd {
            Some(Command::NextField) => {
                self.move_focus(1);
                return FormEvent::None;
            }
            Some(Command::PrevField) => {
                self.move_focus(-1);
                return FormEvent::None;
            }
            Some(Command::Cancel) => return FormEvent::Cancel,
            _ => {}
        }
        match self.focus {
            Stop::Field(i) => {
                let event = self.field_key(i, cmd, editor, ev, ctrl || alt);
                self.refresh();
                event
            }
            Stop::ListButton(i, b) => match cmd {
                Some(Command::Accept | Command::Toggle) => self.list_button(i, b),
                Some(Command::Left | Command::Up) => {
                    self.move_focus(-1);
                    FormEvent::None
                }
                Some(Command::Right | Command::Down) => {
                    self.move_focus(1);
                    FormEvent::None
                }
                _ => FormEvent::None,
            },
            Stop::Preview => {
                match cmd {
                    Some(Command::Up) => self.preview.scroll_lines(-1),
                    Some(Command::Down) => self.preview.scroll_lines(1),
                    Some(Command::PageUp) => self.preview.scroll_lines(-10),
                    Some(Command::PageDown) => self.preview.scroll_lines(10),
                    Some(Command::Accept) => return FormEvent::Ok,
                    _ => {}
                }
                FormEvent::None
            }
            Stop::Ok => match cmd {
                Some(Command::Accept | Command::Toggle) => FormEvent::Ok,
                Some(Command::Right) => {
                    self.focus = Stop::Cancel;
                    FormEvent::None
                }
                _ => FormEvent::None,
            },
            Stop::Cancel => match cmd {
                Some(Command::Accept | Command::Toggle) => FormEvent::Cancel,
                Some(Command::Left) => {
                    self.focus = Stop::Ok;
                    FormEvent::None
                }
                _ => FormEvent::None,
            },
        }
    }

    fn field_key(
        &mut self,
        i: usize,
        cmd: Option<Command>,
        editor: Option<Action>,
        ev: &KeyEvent,
        modified: bool,
    ) -> FormEvent {
        let f = &mut self.fields[i];
        match &mut f.value {
            FieldValue::Text { text, cursor } => {
                if let KeyCode::Char(c) = ev.code
                    && !modified
                {
                    let at = byte_index(text, *cursor);
                    text.insert(at, c);
                    *cursor += 1;
                    return FormEvent::None;
                }
                let len = text.chars().count();
                match cmd {
                    Some(Command::Left) => *cursor = cursor.saturating_sub(1),
                    Some(Command::Right) => *cursor = (*cursor + 1).min(len),
                    Some(Command::Home) => *cursor = 0,
                    Some(Command::End) => *cursor = len,
                    Some(Command::Backspace) if *cursor > 0 => {
                        *cursor -= 1;
                        let at = byte_index(text, *cursor);
                        text.remove(at);
                    }
                    Some(Command::Delete) if *cursor < len => {
                        let at = byte_index(text, *cursor);
                        text.remove(at);
                    }
                    Some(Command::Up) => self.move_focus(-1),
                    Some(Command::Down) => self.move_focus(1),
                    Some(Command::Accept) => return FormEvent::Ok,
                    _ => {}
                }
            }
            FieldValue::Bool(b) => match cmd {
                Some(Command::Toggle) => *b = !*b,
                Some(Command::Up) => self.move_focus(-1),
                Some(Command::Down) => self.move_focus(1),
                Some(Command::Accept) => return FormEvent::Ok,
                _ => {}
            },
            FieldValue::Choice {
                items,
                selected,
                state,
                ..
            } => {
                let ready = *state == ChoiceState::Ready && !items.is_empty();
                match cmd {
                    // Enter / Space open the list (as in the dialogs, crate::combo)
                    Some(Command::Accept | Command::Toggle) if ready => {
                        self.popup = Some(Popup {
                            field: i,
                            list: ComboList::open(*selected),
                        });
                    }
                    Some(Command::Left) if ready => {
                        *selected = combo::step(*selected, items.len(), -1);
                    }
                    Some(Command::Right) if ready => {
                        *selected = combo::step(*selected, items.len(), 1);
                    }
                    Some(Command::Delete | Command::Backspace) if !f.param.required => {
                        *selected = None;
                    }
                    Some(Command::Up) => self.move_focus(-1),
                    Some(Command::Down) => self.move_focus(1),
                    _ => {}
                }
            }
            FieldValue::Sql(e) => {
                if let KeyCode::Char(c) = ev.code
                    && !modified
                {
                    e.apply(Action::Insert(c.to_string()));
                } else if cmd == Some(Command::Accept) {
                    e.apply(Action::Insert("\n".to_owned()));
                } else if let Some(action) = editor {
                    e.apply(action);
                }
            }
            FieldValue::List { rows, selected } => match cmd {
                Some(Command::Up) if *selected > 0 => *selected -= 1,
                Some(Command::Up) => self.move_focus(-1),
                Some(Command::Down) if *selected + 1 < rows.len() => *selected += 1,
                Some(Command::Down) => self.move_focus(1),
                Some(Command::Accept) if rows.is_empty() => return FormEvent::AddRow(i),
                Some(Command::Accept) => return FormEvent::EditRow(i, *selected),
                Some(Command::Delete) => return self.list_button(i, 2),
                _ => {}
            },
        }
        FormEvent::None
    }

    fn list_button(&mut self, i: usize, b: usize) -> FormEvent {
        let FieldValue::List { rows, selected } = &mut self.fields[i].value else {
            return FormEvent::None;
        };
        let event = match b {
            0 => return FormEvent::AddRow(i),
            1 if !rows.is_empty() => return FormEvent::EditRow(i, *selected),
            2 if !rows.is_empty() => {
                rows.remove(*selected);
                *selected = (*selected).min(rows.len().saturating_sub(1));
                FormEvent::None
            }
            3 if *selected > 0 => {
                rows.swap(*selected, *selected - 1);
                *selected -= 1;
                FormEvent::None
            }
            4 if *selected + 1 < rows.len() => {
                rows.swap(*selected, *selected + 1);
                *selected += 1;
                FormEvent::None
            }
            _ => FormEvent::None,
        };
        self.refresh();
        event
    }

    /// Items of choice field `i` (empty for other fields).
    fn choice_items(&self, i: usize) -> &[Choice] {
        match &self.fields[i].value {
            FieldValue::Choice { items, .. } => items,
            _ => &[],
        }
    }

    /// Labels shown by the open list (after its type-ahead filter).
    #[cfg(test)]
    pub fn popup_items(&self) -> Vec<&str> {
        let Some(p) = &self.popup else {
            return Vec::new();
        };
        let items = self.choice_items(p.field);
        p.list
            .visible(items)
            .into_iter()
            .map(|i| items[i].label.as_str())
            .collect()
    }

    fn popup_key(&mut self, cmd: Option<Command>, ev: &KeyEvent) {
        let Some(mut p) = self.popup.take() else {
            return;
        };
        let event = p.list.key(cmd, ev, self.choice_items(p.field));
        self.popup = Some(p);
        self.popup_event(event);
    }

    /// Applies what the open list did.
    fn popup_event(&mut self, event: ListEvent) {
        let Some(field) = self.popup.as_ref().map(|p| p.field) else {
            return;
        };
        match event {
            ListEvent::None => return,
            ListEvent::Chosen(c) => {
                if let FieldValue::Choice { selected, .. } = &mut self.fields[field].value {
                    *selected = Some(c);
                }
            }
            ListEvent::Closed => {}
            ListEvent::Next => self.move_focus(1),
            ListEvent::Prev => self.move_focus(-1),
        }
        self.popup = None;
        self.popup_area.set(None);
        self.refresh();
    }

    /// Mouse while a choice list is open: a click on a row chooses it, a click
    /// elsewhere closes the list, the wheel scrolls it. False if no list is open.
    pub fn popup_mouse(&mut self, kind: crossterm::event::MouseEventKind, p: Position) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        let (Some(mut popup), Some(area)) = (self.popup.take(), self.popup_area.get()) else {
            return false;
        };
        let items = self.choice_items(popup.field);
        let event = match kind {
            MouseEventKind::Down(MouseButton::Left) => popup.list.click(area, p, items),
            MouseEventKind::ScrollDown => {
                popup.list.wheel(3, items);
                ListEvent::None
            }
            MouseEventKind::ScrollUp => {
                popup.list.wheel(-3, items);
                ListEvent::None
            }
            _ => ListEvent::None,
        };
        self.popup = Some(popup);
        self.popup_event(event);
        true
    }

    /// Tells the preview how big it is drawn (for scrolling).
    pub fn set_screen(&mut self, screen: Rect) {
        let l = Self::layout(screen);
        self.preview
            .set_view(l.preview.height as usize, l.preview.width as usize);
        if let Stop::Field(i) = self.focus
            && let FieldValue::Sql(e) = &mut self.fields[i].value
        {
            // inside the box's frame
            e.set_view(SQL_ROWS as usize, l.form.width.saturating_sub(2) as usize);
        }
    }

    /// Bracketed paste into the focused text field or SQL box.
    pub fn paste(&mut self, text: &str) {
        if let Stop::Field(i) = self.focus {
            match &mut self.fields[i].value {
                FieldValue::Text { text: t, cursor } => {
                    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
                    let at = byte_index(t, *cursor);
                    t.insert_str(at, &clean);
                    *cursor += clean.chars().count();
                }
                FieldValue::Sql(e) => {
                    e.apply(Action::Insert(text.replace("\r\n", "\n")));
                }
                _ => {}
            }
            self.refresh();
        }
    }

    // ---- layout and drawing -------------------------------------------------------

    /// Rows a field takes in the form.
    fn field_height(f: &Field) -> u16 {
        match f.value {
            // the editor in its own frame, the label as the frame's title
            FieldValue::Sql(_) => SQL_ROWS + 2,
            FieldValue::List { .. } => 1 + LIST_ROWS + 1,
            _ => 1,
        }
    }

    fn label_width(&self) -> u16 {
        self.fields
            .iter()
            .filter(|f| {
                !matches!(
                    f.value,
                    FieldValue::Sql(_) | FieldValue::List { .. } | FieldValue::Bool(_)
                )
            })
            .map(|f| f.param.label().width() as u16 + 2)
            .max()
            .unwrap_or(0)
            .min(28)
    }

    /// Where everything of the form is drawn on `screen`.
    pub fn layout(screen: Rect) -> Layout {
        // like every dialog (D-014): a margin of dialog colour around the frame and
        // a shadow; as large as the screen between the menu bar and the F-key bar
        let outer = Rect {
            x: screen.x,
            y: screen.y + 1,
            width: screen.width.saturating_sub(2),
            height: screen.height.saturating_sub(3),
        };
        let frame = Rect {
            x: outer.x + 1,
            y: outer.y + 1,
            width: outer.width.saturating_sub(2),
            height: outer.height.saturating_sub(2),
        };
        // inside the frame lines
        let inner = Rect {
            x: frame.x + 1,
            y: frame.y + 1,
            width: frame.width.saturating_sub(2),
            height: frame.height.saturating_sub(2),
        };
        // bottom: separator, help line, buttons
        let buttons = Rect {
            y: inner.bottom().saturating_sub(1),
            height: 1,
            ..inner
        };
        let help = Rect {
            x: inner.x + 1,
            y: buttons.y.saturating_sub(1),
            width: inner.width.saturating_sub(2),
            height: 1,
        };
        let separator = help.y.saturating_sub(1);
        let body = Rect {
            height: separator.saturating_sub(inner.y),
            ..inner
        };
        // the form keeps one column of space on both sides
        let pad = |r: Rect| Rect {
            x: r.x + 1,
            width: r.width.saturating_sub(2),
            ..r
        };
        let (form, preview, split) = if inner.width >= 90 {
            // side by side: a vertical line, the preview's title under its own line
            let fw = inner.width * 11 / 20;
            let x = inner.x + fw;
            let pane = Rect {
                x: x + 1,
                width: inner.right().saturating_sub(x + 1),
                ..body
            };
            let preview = Rect {
                y: pane.y + 2,
                height: pane.height.saturating_sub(2),
                ..pane
            };
            (
                pad(Rect { width: fw, ..body }),
                preview,
                Split::Vertical { x },
            )
        } else {
            // one above the other: a horizontal line with the preview's title
            let fh = body.height * 3 / 5;
            let y = body.y + fh;
            let preview = Rect {
                y: y + 1,
                height: body.bottom().saturating_sub(y + 1),
                ..body
            };
            (
                pad(Rect { height: fh, ..body }),
                preview,
                Split::Horizontal { y },
            )
        };
        Layout {
            outer,
            frame,
            form,
            preview,
            split,
            separator,
            help,
            buttons,
        }
    }

    /// The frame, the line between form and preview, the line above the help line.
    fn draw_lines(&self, frame: &mut Frame, theme: &Theme, l: &Layout) {
        let normal = theme.get(Role::DialogNormal);
        let j = theme.plain_junctions();
        let f = l.frame;
        if f.width < 2 || f.height < 2 {
            return;
        }
        frame.render_widget(
            Block::bordered()
                .border_set(theme.plain_border())
                .border_style(normal)
                .style(normal),
            f,
        );
        // the title centred over the form (side by side: left of the vertical line)
        let title_w = match l.split {
            Split::Vertical { x } => x - f.x,
            Split::Horizontal { .. } => f.width,
        };
        let title = format!(" {} ", self.title);
        let tw = (title.width() as u16).min(title_w.saturating_sub(2));
        put(
            frame,
            f.x + title_w.saturating_sub(tw) / 2,
            f.y,
            &title,
            tw,
            theme.get(Role::DialogTitle),
        );
        // a horizontal line from frame to frame (or from x to the right frame)
        let hline = |frame: &mut Frame, x0: u16, y: u16, left: &str| {
            let w = f.right().saturating_sub(x0 + 1);
            if y <= f.y || y + 1 >= f.bottom() || w == 0 {
                return;
            }
            let line = format!("{left}{}{}", theme.hline().repeat(w as usize - 1), j.right);
            put(frame, x0, y, &line, w + 1, normal);
        };
        hline(frame, f.x, l.separator, j.left);
        let title = |frame: &mut Frame, x: u16, y: u16| {
            put(frame, x, y, " Preview ", 9, theme.get(Role::DialogTitle));
        };
        match l.split {
            Split::Vertical { x } => {
                put(frame, x, f.y, j.top, 1, normal);
                for y in f.y + 1..l.separator {
                    put(frame, x, y, theme.vline(), 1, normal);
                }
                put(frame, x, l.separator, j.bottom, 1, normal);
                title(frame, x + 1, f.y + 1);
                hline(frame, x, f.y + 2, j.left);
            }
            Split::Horizontal { y } => {
                hline(frame, f.x, y, j.left);
                title(frame, f.x + 2, y);
            }
        }
    }

    /// Draws the form; returns the click areas of the stops.
    pub fn render(
        &self,
        frame: &mut Frame,
        theme: &Theme,
        editor_styles: &EditorStyles,
    ) -> Vec<(Stop, Rect)> {
        let screen = frame.area();
        let layout = Self::layout(screen);
        let Layout {
            outer,
            form,
            preview: pv,
            buttons,
            ..
        } = layout;
        let normal = theme.get(Role::DialogNormal);
        let focus = theme.get(Role::DialogFocus);
        let input = theme.get(Role::DialogInput);
        let input_focus = theme.get(Role::DialogInputFocus);
        let hint = theme.get(Role::DialogDisabled);
        let mut hits = Vec::new();

        crate::dialog::draw_shadow(frame, outer, theme.get(Role::DialogShadow));
        frame.render_widget(Clear, outer);
        frame.render_widget(Block::new().style(normal), outer);
        self.draw_lines(frame, theme, &layout);

        // keep the focused field visible
        let mut tops = Vec::new();
        let mut y = 0u16;
        for f in &self.fields {
            tops.push(y);
            y += Self::field_height(f);
        }
        let total = y;
        if let Some(i) = match self.focus {
            Stop::Field(i) | Stop::ListButton(i, _) => Some(i),
            _ => None,
        } {
            let (top, h) = (tops[i], Self::field_height(&self.fields[i]));
            let visible = form.height;
            if top < self.scroll.get() {
                self.scroll.set(top);
            } else if top + h > self.scroll.get() + visible {
                self.scroll.set((top + h).saturating_sub(visible));
            }
        }
        let scroll = self.scroll.get().min(total.saturating_sub(form.height));
        self.scroll.set(scroll);

        let rows_area = form;
        let lw = self.label_width();
        let mut cursor: Option<Position> = None;
        for (i, f) in self.fields.iter().enumerate() {
            let h = Self::field_height(f);
            let top = tops[i] as i32 - scroll as i32;
            if top + h as i32 <= 0 || top >= rows_area.height as i32 {
                continue;
            }
            let focused = self.focus == Stop::Field(i);
            let label = f.param.label();
            let star = if f.param.required { "*" } else { "" };
            let row_y = |r: u16| -> Option<u16> {
                let yy = top + r as i32;
                (yy >= 0 && yy < rows_area.height as i32).then(|| rows_area.y + yy as u16)
            };
            match &f.value {
                FieldValue::Text { text, cursor: c } => {
                    if let Some(yy) = row_y(0) {
                        put(frame, form.x, yy, &format!("{label}{star}:"), lw, normal);
                        let r = Rect::new(form.x + lw, yy, form.width.saturating_sub(lw), 1);
                        let style = if focused { input_focus } else { input };
                        let w = (r.width as usize).saturating_sub(1);
                        let skip = c.saturating_sub(w.saturating_sub(1));
                        let shown: String = text.chars().skip(skip).collect();
                        put(frame, r.x, r.y, &format!(" {shown:<w$}"), r.width, style);
                        if focused && self.popup.is_none() {
                            cursor = Some(Position::new(r.x + 1 + (c - skip) as u16, r.y));
                        }
                        hits.push((Stop::Field(i), r));
                    }
                }
                FieldValue::Bool(b) => {
                    if let Some(yy) = row_y(0) {
                        let text = format!("[{}] {label}", if *b { "x" } else { " " });
                        let style = if focused { focus } else { normal };
                        let w = text.width() as u16;
                        put(frame, form.x, yy, &text, w.min(form.width), style);
                        hits.push((Stop::Field(i), Rect::new(form.x, yy, w.min(form.width), 1)));
                    }
                }
                FieldValue::Choice {
                    items,
                    selected,
                    state,
                    ..
                } => {
                    if let Some(yy) = row_y(0) {
                        put(frame, form.x, yy, &format!("{label}{star}:"), lw, normal);
                        let avail = form.width.saturating_sub(lw);
                        let style = if focused { input_focus } else { input };
                        match state {
                            ChoiceState::Ready => {
                                let r =
                                    Rect::new(form.x + lw, yy, combo::field_width(items, avail), 1);
                                let text = selected
                                    .and_then(|s| items.get(s))
                                    .map_or("", |c| c.label.as_str());
                                put(
                                    frame,
                                    r.x,
                                    r.y,
                                    &combo::closed_text(text, r.width),
                                    r.width,
                                    style,
                                );
                                hits.push((Stop::Field(i), r));
                            }
                            // not a value: plain text after the label
                            ChoiceState::Loading | ChoiceState::Failed(_) => {
                                let text = match state {
                                    ChoiceState::Failed(e) => format!("error: {e}"),
                                    _ => "loading…".to_owned(),
                                };
                                let r = Rect::new(form.x + lw, yy, avail, 1);
                                put(
                                    frame,
                                    r.x,
                                    r.y,
                                    &text,
                                    r.width,
                                    if focused { focus } else { hint },
                                );
                                hits.push((Stop::Field(i), r));
                            }
                        }
                    }
                }
                FieldValue::Sql(e) => {
                    // drawn only when fully visible (the editor needs its whole box);
                    // otherwise just the label
                    let title = format!(" {label}{star} ");
                    if let (Some(y0), Some(_)) = (row_y(0), row_y(SQL_ROWS + 1)) {
                        let boxed = Rect::new(form.x, y0, form.width, SQL_ROWS + 2);
                        frame.render_widget(
                            Block::bordered()
                                .border_set(theme.plain_border())
                                .border_style(normal)
                                .title_top(Line::from(Span::styled(
                                    title,
                                    if focused { focus } else { normal },
                                )))
                                .style(normal),
                            boxed,
                        );
                        let r =
                            Rect::new(form.x + 1, y0 + 1, form.width.saturating_sub(2), SQL_ROWS);
                        frame.render_widget(Block::new().style(editor_styles.text), r);
                        let pos =
                            pgc_editor::render(e, r, frame.buffer_mut(), editor_styles, false);
                        if focused {
                            cursor = pos;
                        }
                        hits.push((Stop::Field(i), r));
                    } else if let Some(yy) = row_y(0) {
                        put(frame, form.x, yy, &title, form.width, normal);
                    }
                }
                FieldValue::List { rows, selected } => {
                    if let Some(yy) = row_y(0) {
                        put(
                            frame,
                            form.x,
                            yy,
                            &format!("{label}{star}:"),
                            form.width,
                            normal,
                        );
                    }
                    let first = (*selected).saturating_sub(LIST_ROWS as usize - 1);
                    for r in 0..LIST_ROWS {
                        let Some(yy) = row_y(1 + r) else {
                            continue;
                        };
                        let n = first + r as usize;
                        let text = rows
                            .get(n)
                            .map(|row| row_text(&f.param, row))
                            .unwrap_or_default();
                        let style = if n == *selected && !rows.is_empty() && focused {
                            focus
                        } else {
                            input
                        };
                        let w = form.width as usize;
                        let text: String = format!(" {text}").chars().take(w).collect();
                        put(frame, form.x, yy, &format!("{text:<w$}"), form.width, style);
                        hits.push((Stop::Field(i), Rect::new(form.x, yy, form.width, 1)));
                    }
                    if rows.is_empty()
                        && let Some(yy) = row_y(1)
                    {
                        put(
                            frame,
                            form.x,
                            yy,
                            " (no rows — Enter or Add)",
                            form.width,
                            input,
                        );
                    }
                    if let Some(yy) = row_y(1 + LIST_ROWS) {
                        let mut x = form.x;
                        for (b, name) in LIST_BUTTONS.iter().enumerate() {
                            let text = format!("[ {name} ]");
                            let w = text.width() as u16;
                            if x + w > form.right() {
                                break;
                            }
                            let style = if self.focus == Stop::ListButton(i, b) {
                                focus
                            } else {
                                normal
                            };
                            put(frame, x, yy, &text, w, style);
                            hits.push((Stop::ListButton(i, b), Rect::new(x, yy, w, 1)));
                            x += w + 1;
                        }
                    }
                }
            }
        }
        // help / status line
        let help = match self.focus {
            Stop::Field(i) => self.fields[i].param.help.clone().unwrap_or_default(),
            Stop::Preview => "Read-only preview".to_owned(),
            _ => String::new(),
        };
        put(
            frame,
            layout.help.x,
            layout.help.y,
            &help,
            layout.help.width,
            hint,
        );

        // preview (its title is drawn with the lines)
        frame.render_widget(Block::new().style(editor_styles.text), pv);
        match &self.error {
            Some(e) => {
                let style = theme.get(Role::ErrorNormal);
                let lines: Vec<Line> = e
                    .lines()
                    .map(|l| Line::from(Span::styled(l.to_owned(), style)))
                    .collect();
                frame.render_widget(
                    Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
                    pv,
                );
            }
            None => {
                pgc_editor::render(&self.preview, pv, frame.buffer_mut(), editor_styles, false);
            }
        }
        hits.push((Stop::Preview, pv));

        // OK / Cancel
        let ok = "[< OK >]";
        let cancel = "[ Cancel ]";
        let total_w = (ok.width() + 1 + cancel.width()) as u16;
        let x = buttons.x + buttons.width.saturating_sub(total_w) / 2;
        let st = |s: Stop| if self.focus == s { focus } else { normal };
        put(frame, x, buttons.y, ok, ok.width() as u16, st(Stop::Ok));
        hits.push((Stop::Ok, Rect::new(x, buttons.y, ok.width() as u16, 1)));
        let cx = x + ok.width() as u16 + 1;
        put(
            frame,
            cx,
            buttons.y,
            cancel,
            cancel.width() as u16,
            st(Stop::Cancel),
        );
        hits.push((
            Stop::Cancel,
            Rect::new(cx, buttons.y, cancel.width() as u16, 1),
        ));

        // open choice list (the shared combo list), drawn over the form
        self.popup_area.set(None);
        if let Some(p) = &self.popup
            && let Some((_, r)) = hits.iter().find(|(s, _)| *s == Stop::Field(p.field))
        {
            let items = self.choice_items(p.field);
            let area = p.list.area(*r, items, outer);
            p.list.render(frame, area, items, theme);
            self.popup_area.set(Some(area));
            frame.set_cursor_position(p.list.cursor(area));
        } else if let Some(pos) = cursor {
            // a text cursor: also drawn reversed, as it may be invisible otherwise
            frame.set_cursor_position(pos);
            crate::dialog::show_cursor_cell(frame, pos);
        } else if let Some(pos) = self.focus_cursor(&hits) {
            frame.set_cursor_position(pos);
        }
        hits
    }

    /// The terminal cursor on the focused control that has no text cursor (as in
    /// mc): inside `[ ]`, on a combo's value, on a button's label, on a list row.
    fn focus_cursor(&self, hits: &[(Stop, Rect)]) -> Option<Position> {
        let mut areas = hits
            .iter()
            .filter(|(s, _)| *s == self.focus)
            .map(|(_, r)| *r);
        let first = areas.clone().next()?;
        let at = |r: Rect, dx: u16| Position::new(r.x + dx.min(r.width.saturating_sub(1)), r.y);
        Some(match self.focus {
            Stop::Field(i) => match &self.fields[i].value {
                FieldValue::List { selected, .. } => {
                    let first_row = selected.saturating_sub(LIST_ROWS as usize - 1);
                    let r = areas.nth(selected - first_row).unwrap_or(first);
                    at(r, 1)
                }
                _ => at(first, 1),
            },
            Stop::ListButton(..) | Stop::Cancel => at(first, 2),
            Stop::Ok => at(first, 3),
            Stop::Preview => at(first, 0),
        })
    }

    /// A click on stop `s`.
    pub fn click(&mut self, s: Stop, y: u16, area: Rect) -> FormEvent {
        self.popup = None;
        self.focus = s;
        match s {
            Stop::Ok => FormEvent::Ok,
            Stop::Cancel => FormEvent::Cancel,
            Stop::ListButton(i, b) => self.list_button(i, b),
            Stop::Field(i) => {
                match &mut self.fields[i].value {
                    FieldValue::Bool(b) => *b = !*b,
                    FieldValue::Choice {
                        state,
                        items,
                        selected,
                        ..
                    } if *state == ChoiceState::Ready && !items.is_empty() => {
                        self.popup = Some(Popup {
                            field: i,
                            list: ComboList::open(*selected),
                        });
                    }
                    FieldValue::List { rows, selected } => {
                        let _ = (y, area);
                        let row_in_list = y.saturating_sub(area.y) as usize;
                        let first = (*selected).saturating_sub(LIST_ROWS as usize - 1);
                        if first + row_in_list < rows.len() {
                            *selected = first + row_in_list;
                        }
                    }
                    _ => {}
                }
                self.refresh();
                FormEvent::None
            }
            Stop::Preview => FormEvent::None,
        }
    }
}

fn initial_value(p: &Param, tab_width: usize) -> FieldValue {
    match p.kind {
        ParamType::Bool => FieldValue::Bool(p.default_bool()),
        ParamType::Choice => {
            let items: Vec<Choice> = p
                .options
                .iter()
                .map(|o| Choice {
                    label: o.clone(),
                    value: ParamValue::Text(o.clone()),
                })
                .collect();
            let fixed = !items.is_empty();
            let default = p.default_text();
            let selected = items.iter().position(|c| c.label == default);
            FieldValue::Choice {
                items,
                selected: if fixed { selected } else { None },
                state: if fixed {
                    ChoiceState::Ready
                } else {
                    ChoiceState::Loading
                },
                wanted: (!fixed && !default.is_empty()).then_some(default),
            }
        }
        ParamType::Sql => {
            FieldValue::Sql(Box::new(Editor::new(&p.default_text(), false, tab_width)))
        }
        ParamType::List => FieldValue::List {
            rows: Vec::new(),
            selected: 0,
        },
        ParamType::Text | ParamType::Identifier | ParamType::Number => {
            let text = p.default_text();
            FieldValue::Text {
                cursor: text.chars().count(),
                text,
            }
        }
    }
}

/// One list row as text: its field values side by side.
pub fn row_text(param: &Param, row: &BTreeMap<String, ParamValue>) -> String {
    param
        .fields
        .iter()
        .map(|f| match row.get(&f.name) {
            Some(ParamValue::Text(s)) => s.clone(),
            Some(ParamValue::Bool(true)) => f.label().to_owned(),
            Some(ParamValue::Bool(false)) | None => String::new(),
            Some(ParamValue::Object(q)) => q.to_string(),
            Some(ParamValue::List(_)) => "…".to_owned(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("  ")
}

/// Choices from lookup rows, columns as in [`Shape`] (D-050).
pub fn choices_from_rows(rows: Vec<Vec<String>>, shape: Shape) -> Vec<Choice> {
    rows.into_iter()
        .filter_map(|r| {
            let col = |i: usize| r.get(i).cloned();
            let q = match shape {
                Shape::Name => {
                    let v = col(0)?;
                    return Some(Choice {
                        label: v.clone(),
                        value: ParamValue::Text(v),
                    });
                }
                Shape::Qualified => QualName::new(col(0)?, col(1)?),
                Shape::WithArgs => QualName {
                    args: Some(col(2).unwrap_or_default()),
                    ..QualName::new(col(0)?, col(1)?)
                },
                Shape::OnTable => QualName {
                    table: Some(col(1)?),
                    ..QualName::new(col(0)?, col(2)?)
                },
            };
            Some(Choice {
                label: q.to_string(),
                value: ParamValue::Object(q),
            })
        })
        .collect()
}

fn put(frame: &mut Frame, x: u16, y: u16, text: &str, width: u16, style: Style) {
    let area = frame.area();
    if y >= area.bottom() || x >= area.right() {
        return;
    }
    let w = width.min(area.right() - x) as usize;
    frame.buffer_mut().set_stringn(x, y, text, w, style);
}

fn byte_index(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map_or(s.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgc_core::ServerVersion;

    fn form() -> TemplateForm {
        let file = devtemplate::parse(
            "+++
name = \"T\"
[[param]]
name = \"schema\"
type = \"choice\"
source = \"schemas\"
default = \"public\"
[[param]]
name = \"name\"
type = \"identifier\"
required = true
[[param]]
name = \"cols\"
type = \"list\"
  [[param.fields]]
  name = \"c\"
  type = \"identifier\"
[[param]]
name = \"body\"
type = \"sql\"
[[param]]
name = \"flag\"
type = \"bool\"
+++
CREATE {{ schema|ident }}.{{ name|ident }}({% for c in cols %}{{ c.c }}{% endfor %}){% if flag %} F{% endif %} {{ body }};
",
        )
        .unwrap();
        TemplateForm::new(
            7,
            "T".into(),
            file,
            TemplateContext {
                version: ServerVersion(160000),
                database: "db".into(),
            },
            4,
        )
    }

    fn key(f: &mut TemplateForm, cmd: Option<Command>, code: KeyCode) -> FormEvent {
        f.handle_key(cmd, None, &KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn choice_rows_by_shape() {
        let row = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let f = choices_from_rows(vec![row(&["s", "f", "a integer"])], Shape::WithArgs);
        assert_eq!(f[0].label, "s.f(a integer)");
        let t = choices_from_rows(vec![row(&["s", "Orders", "trg"])], Shape::OnTable);
        assert_eq!(t[0].label, "trg ON s.Orders");
        let ParamValue::Object(q) = &t[0].value else {
            panic!("object");
        };
        assert_eq!(q.table.as_deref(), Some("Orders"));
        let q = choices_from_rows(vec![row(&["s", "t"]), row(&["short"])], Shape::Qualified);
        assert_eq!(q.len(), 1, "rows with missing columns are skipped");
    }

    #[test]
    fn values_preview_and_choices() {
        let mut f = form();
        assert_eq!(f.focus, Stop::Field(0));
        assert_eq!(f.error.as_deref(), Some("name is required"));
        // choices arrive; the default is selected
        f.set_choices(
            "schema",
            Ok(choices_from_rows(
                vec![vec!["public".into()], vec!["sales".into()]],
                Shape::Name,
            )),
        );
        // type the name
        key(&mut f, Some(Command::NextField), KeyCode::Tab);
        for c in "Orders".chars() {
            key(&mut f, None, KeyCode::Char(c));
        }
        assert_eq!(f.error, None);
        assert_eq!(f.preview.text(), "CREATE public.\"Orders\"() ;\n");
        // pick another schema: Enter opens the list, Down + Enter chooses
        f.focus = Stop::Field(0);
        key(&mut f, Some(Command::Accept), KeyCode::Enter);
        assert!(f.popup.is_some());
        assert_eq!(f.popup_items(), ["public", "sales"]);
        key(&mut f, Some(Command::Down), KeyCode::Down);
        key(&mut f, Some(Command::Accept), KeyCode::Enter);
        assert!(f.popup.is_none());
        assert!(f.preview.text().starts_with("CREATE sales."));
        // Left / Right step through the values of a closed combo
        key(&mut f, Some(Command::Right), KeyCode::Right);
        assert!(f.preview.text().starts_with("CREATE public."));
        // long lists are filtered by typing
        let many = (0..20).map(|n| vec![format!("s{n}")]).collect();
        f.set_choices("schema", Ok(choices_from_rows(many, Shape::Name)));
        key(&mut f, Some(Command::Toggle), KeyCode::Char(' '));
        key(&mut f, None, KeyCode::Char('1'));
        key(&mut f, None, KeyCode::Char('5'));
        assert_eq!(f.popup_items(), ["s15"]);
        key(&mut f, Some(Command::Accept), KeyCode::Enter);
        assert!(f.preview.text().starts_with("CREATE s15."));
        // list rows come from the row dialog
        f.list_rows_mut(2)
            .unwrap()
            .push(BTreeMap::from([("c".into(), ParamValue::Text("a".into()))]));
        f.list_rows_mut(2)
            .unwrap()
            .push(BTreeMap::from([("c".into(), ParamValue::Text("b".into()))]));
        f.refresh();
        assert!(f.preview.text().contains("(ab)"));
        // Down button moves the selected row
        assert_eq!(f.list_button(2, 4), FormEvent::None);
        f.refresh();
        assert!(f.preview.text().contains("(ba)"));
        // SQL box: Enter is a new line, Tab leaves it
        f.focus = Stop::Field(3);
        assert!(f.editing_sql());
        key(&mut f, None, KeyCode::Char('x'));
        key(&mut f, Some(Command::Accept), KeyCode::Enter);
        key(&mut f, None, KeyCode::Char('y'));
        assert!(f.preview.text().contains("x\ny;"));
        key(&mut f, Some(Command::NextField), KeyCode::Tab);
        assert_eq!(f.focus, Stop::Field(4));
        key(&mut f, Some(Command::Toggle), KeyCode::Char(' '));
        assert!(f.preview.text().contains(") F x"));
        // Enter on a field is OK
        assert_eq!(
            key(&mut f, Some(Command::Accept), KeyCode::Enter),
            FormEvent::Ok
        );
    }

    #[test]
    fn stops_include_list_buttons() {
        let f = form();
        let stops = f.stops();
        assert!(stops.contains(&Stop::ListButton(2, 0)));
        assert_eq!(stops.last(), Some(&Stop::Cancel));
    }
}
