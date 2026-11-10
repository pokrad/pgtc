//! The open list of a combo box (drop-down), shared by the dialogs and the developer
//! template form so both look and behave the same (D-016, D-046).
//!
//! Closed combo: Enter / Space / click opens the list, Left / Right steps through the
//! values. Open list: Up / Down / PgUp / PgDn / Home / End move, Enter / Space / click
//! choose, Esc / a click outside closes, Tab / Shift+Tab close and move on, the wheel
//! scrolls. Long lists (more items than rows) are filtered by typing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::keymap::Command;
use crate::theme::{Role, Theme};

/// Visible rows of an open list.
pub const ROWS: usize = 8;
/// Narrowest closed combo (value text).
const MIN_TEXT: usize = 10;

/// What a key or mouse event on the open list did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListEvent {
    /// Still open.
    None,
    /// Item (index into all items) chosen; the list closes.
    Chosen(usize),
    /// Closed without a choice.
    Closed,
    /// Closed; the focus moves to the next / previous field.
    Next,
    Prev,
}

/// An open combo list: highlighted row and first visible row (both in the filtered
/// list) and the type-ahead filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComboList {
    pub hi: usize,
    pub offset: usize,
    pub filter: String,
}

impl ComboList {
    /// Opened on the selected item.
    pub fn open(selected: Option<usize>) -> Self {
        let hi = selected.unwrap_or(0);
        Self {
            hi,
            offset: hi.saturating_sub(ROWS - 1),
            filter: String::new(),
        }
    }

    /// Indices of the items shown (after the filter).
    pub fn visible<S: AsRef<str>>(&self, items: &[S]) -> Vec<usize> {
        let filter = self.filter.to_lowercase();
        (0..items.len())
            .filter(|&i| items[i].as_ref().to_lowercase().contains(&filter))
            .collect()
    }

    pub fn key<S: AsRef<str>>(
        &mut self,
        cmd: Option<Command>,
        ev: &KeyEvent,
        items: &[S],
    ) -> ListEvent {
        let filterable = items.len() > ROWS;
        if let KeyCode::Char(c) = ev.code
            && c != ' '
            && !ev
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            if filterable {
                self.filter.push(c);
                self.hi = 0;
                self.offset = 0;
            }
            return ListEvent::None;
        }
        let shown = self.visible(items);
        let max = shown.len().saturating_sub(1);
        match cmd {
            Some(Command::Up) => self.hi = self.hi.saturating_sub(1),
            Some(Command::Down) => self.hi = (self.hi + 1).min(max),
            Some(Command::PageUp) => self.hi = self.hi.saturating_sub(ROWS),
            Some(Command::PageDown) => self.hi = (self.hi + ROWS).min(max),
            Some(Command::Home) => self.hi = 0,
            Some(Command::End) => self.hi = max,
            Some(Command::Backspace) if filterable => {
                self.filter.pop();
                self.hi = 0;
                self.offset = 0;
            }
            Some(Command::Accept | Command::Toggle) => {
                return shown
                    .get(self.hi)
                    .map_or(ListEvent::Closed, |&i| ListEvent::Chosen(i));
            }
            Some(Command::Cancel) => return ListEvent::Closed,
            Some(Command::NextField) => return ListEvent::Next,
            Some(Command::PrevField) => return ListEvent::Prev,
            _ => {}
        }
        self.keep_visible();
        ListEvent::None
    }

    /// Mouse wheel over the list: `delta` rows.
    pub fn wheel<S: AsRef<str>>(&mut self, delta: isize, items: &[S]) {
        let max = self.visible(items).len().saturating_sub(1) as isize;
        self.hi = (self.hi as isize + delta).clamp(0, max.max(0)) as usize;
        self.keep_visible();
    }

    /// A click at `p`; `area` is the list box from [`ComboList::area`].
    pub fn click<S: AsRef<str>>(&self, area: Rect, p: Position, items: &[S]) -> ListEvent {
        let rows = Self::rows_rect(area);
        if !rows.contains(p) {
            // on the frame: nothing; outside: close
            return if area.contains(p) {
                ListEvent::None
            } else {
                ListEvent::Closed
            };
        }
        let n = self.offset + (p.y - rows.y) as usize;
        self.visible(items)
            .get(n)
            .map_or(ListEvent::None, |&i| ListEvent::Chosen(i))
    }

    /// Where the terminal cursor goes: on the highlighted row of the list at `area`.
    pub fn cursor(&self, area: Rect) -> Position {
        let rows = Self::rows_rect(area);
        let row = (self.hi.saturating_sub(self.offset) as u16).min(rows.height.saturating_sub(1));
        Position::new(rows.x + 1, rows.y + row)
    }

    fn keep_visible(&mut self) {
        if self.hi < self.offset {
            self.offset = self.hi;
        } else if self.hi >= self.offset + ROWS {
            self.offset = self.hi + 1 - ROWS;
        }
    }

    /// The framed list box for a closed combo at `field`: below it, or above it when
    /// there is no room below; at least as wide as the field, wider for long items.
    pub fn area<S: AsRef<str>>(&self, field: Rect, items: &[S], screen: Rect) -> Rect {
        let shown = self.visible(items).len();
        let rows = shown.clamp(1, ROWS) as u16;
        let height = rows + 2;
        let longest = items.iter().map(|s| s.as_ref().width()).max().unwrap_or(0) as u16;
        let x = field.x.saturating_sub(1).max(screen.x);
        let width = (longest + 4)
            .max(field.width + 2)
            .min(screen.right().saturating_sub(x));
        let y = if field.y + 1 + height <= screen.bottom() {
            field.y + 1
        } else {
            field.y.saturating_sub(height).max(screen.y)
        };
        Rect::new(x, y, width, height.min(screen.height))
    }

    fn rows_rect(area: Rect) -> Rect {
        Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        )
    }

    pub fn render<S: AsRef<str>>(&self, frame: &mut Frame, area: Rect, items: &[S], theme: &Theme) {
        let normal = theme.get(Role::DialogNormal);
        let input = theme.get(Role::DialogInput);
        let focus = theme.get(Role::DialogFocus);
        let shown = self.visible(items);
        let mut block = Block::bordered()
            .border_set(theme.plain_border())
            .border_style(normal)
            .style(normal);
        if !self.filter.is_empty() {
            block = block.title_top(Line::from(Span::styled(
                format!(" {} ", self.filter),
                normal,
            )));
        } else if items.len() > ROWS {
            block = block.title_bottom(Line::from(Span::styled(
                " type to filter ",
                theme.get(Role::DialogDisabled),
            )));
        }
        if shown.len() > ROWS {
            block = block.title_bottom(
                Line::from(Span::styled(
                    format!(" {}/{} ", self.hi + 1, shown.len()),
                    normal,
                ))
                .right_aligned(),
            );
        }
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        let rows = Self::rows_rect(area);
        let width = rows.width as usize;
        let lines: Vec<Line> = if shown.is_empty() {
            vec![Line::from(Span::styled(
                format!("{:<width$}", " (no match)"),
                input,
            ))]
        } else {
            shown
                .iter()
                .enumerate()
                .skip(self.offset)
                .take(rows.height as usize)
                .map(|(row, &i)| {
                    let style = if row == self.hi { focus } else { input };
                    let text: String = format!(" {}", items[i].as_ref())
                        .chars()
                        .take(width)
                        .collect();
                    Line::from(Span::styled(format!("{text:<width$}"), style))
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), rows);
    }
}

/// Width of a closed combo: the longest item plus the ` ▼` marker, at most `avail`.
pub fn field_width<S: AsRef<str>>(items: &[S], avail: u16) -> u16 {
    let longest = items
        .iter()
        .map(|s| s.as_ref().width())
        .max()
        .unwrap_or(0)
        .max(MIN_TEXT);
    ((longest + 3) as u16).min(avail)
}

/// Text of a closed combo of `width` cells: ` value… ▼`.
pub fn closed_text(value: &str, width: u16) -> String {
    let inner = (width as usize).saturating_sub(3);
    let mut text = String::new();
    for c in value.chars() {
        if (text.as_str().width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)) > inner
        {
            break;
        }
        text.push(c);
    }
    let pad = inner.saturating_sub(text.as_str().width());
    format!(" {text}{} ▼", " ".repeat(pad))
}

/// Next / previous value of a closed combo (wraps around).
pub fn step(selected: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return selected;
    }
    let from = selected.map_or(if delta > 0 { -1 } else { 0 }, |s| s as isize);
    Some((from + delta).rem_euclid(len as isize) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn keys_filter_and_choose() {
        let items: Vec<String> = (0..20).map(|i| format!("item{i}")).collect();
        let mut l = ComboList::open(Some(15));
        assert_eq!(l.offset, 15 + 1 - ROWS);
        l.key(Some(Command::Down), &ev(KeyCode::Down), &items);
        assert_eq!(
            l.key(Some(Command::Accept), &ev(KeyCode::Enter), &items),
            ListEvent::Chosen(16)
        );
        // typing filters long lists; the choice is an index into all items
        let mut l = ComboList::open(None);
        for c in "m1".chars() {
            l.key(None, &ev(KeyCode::Char(c)), &items);
        }
        assert_eq!(l.visible(&items).len(), 11, "item1, item10..19");
        l.key(Some(Command::End), &ev(KeyCode::End), &items);
        assert_eq!(
            l.key(Some(Command::Toggle), &ev(KeyCode::Char(' ')), &items),
            ListEvent::Chosen(19)
        );
        // short lists are not filtered
        let short = ["a", "b"];
        let mut l = ComboList::open(None);
        l.key(None, &ev(KeyCode::Char('b')), &short);
        assert!(l.filter.is_empty());
        assert_eq!(
            l.key(Some(Command::Cancel), &ev(KeyCode::Esc), &short),
            ListEvent::Closed
        );
        assert_eq!(
            l.key(Some(Command::NextField), &ev(KeyCode::Tab), &short),
            ListEvent::Next
        );
    }

    #[test]
    fn area_click_and_wheel() {
        let items: Vec<String> = (0..20).map(|i| format!("item{i}")).collect();
        let screen = Rect::new(0, 0, 80, 24);
        let mut l = ComboList::open(None);
        // below the field, framed, at least as wide as the field
        let a = l.area(Rect::new(10, 3, 20, 1), &items, screen);
        assert_eq!(a, Rect::new(9, 4, 22, ROWS as u16 + 2));
        // no room below: above the field
        let up = l.area(Rect::new(10, 20, 20, 1), &items, screen);
        assert_eq!(up.bottom(), 20);
        // a row chooses, the frame does nothing, outside closes
        assert_eq!(
            l.click(a, Position::new(12, 6), &items),
            ListEvent::Chosen(1)
        );
        assert_eq!(l.click(a, Position::new(9, 4), &items), ListEvent::None);
        assert_eq!(l.click(a, Position::new(0, 0), &items), ListEvent::Closed);
        l.wheel(12, &items);
        assert_eq!((l.hi, l.offset), (12, 12 + 1 - ROWS));
        assert_eq!(
            l.click(a, Position::new(12, 5), &items),
            ListEvent::Chosen(5)
        );
    }

    #[test]
    fn closed_field() {
        assert_eq!(field_width(&["RESTRICT", "CASCADE"], 50), 13);
        assert_eq!(
            field_width(&["a very long function name(integer, text)"], 20),
            20
        );
        assert_eq!(closed_text("abc", 8), " abc   ▼");
        assert_eq!(closed_text("abcdefgh", 8), " abcde ▼");
        assert_eq!(step(None, 3, 1), Some(0));
        assert_eq!(step(None, 3, -1), Some(2));
        assert_eq!(step(Some(2), 3, 1), Some(0));
        assert_eq!(step(None, 0, 1), None);
    }
}
