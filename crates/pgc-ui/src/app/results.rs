//! The results pane of a query tab (D-019, D-041): keys in the grid and in Messages,
//! mouse clicks, fetching more rows, the context menu.

use ratatui::layout::Position;

use super::{App, Effect, Focus};
use crate::keymap::Command;
use crate::menu::{self, Popup};
use crate::view::Hits;

impl App {
    /// Context menu of the results pane (D-042), of Messages (D-019).
    pub(super) fn open_results_menu(&mut self, anchor: (u16, u16)) {
        let entries = if self.query().is_some_and(|q| q.on_messages()) {
            vec![
                menu::item("&Jump to line", Command::Accept),
                menu::item("&Copy message", Command::Copy),
                menu::item("Copy &all messages", Command::CopyAllMessages),
            ]
        } else {
            vec![
                menu::item("E&xport result…", Command::ExportResult),
                menu::item("&Fetch all", Command::FetchAll),
            ]
        };
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }

    pub(super) fn fetch_all(&mut self) {
        if let Some(q) = self.query_mut()
            && q.paused()
            && q.export.is_none()
            && let Some(conn) = q.conn.as_ref()
        {
            conn.handle.fetch_all(q.last_run);
            if let Some(rs) = q.results.last_mut() {
                rs.fetching = true;
            }
        }
    }

    /// Asks for the next batch when the grid cursor nears the end of a paused result.
    pub(super) fn after_grid_move(&mut self) {
        let (h, w) = self.frame.grid_size;
        let Some(q) = self.query_mut() else {
            return;
        };
        let last = q.results.len().saturating_sub(1);
        let active = q.active_result;
        if let Some(rs) = q.results.get_mut(active) {
            rs.ensure_visible(h.saturating_sub(1), w);
            if active == last
                && q.export.is_none()
                && rs.more
                && !rs.fetching
                && rs.row + h.max(1) * 2 >= rs.rows.len()
                && let Some(conn) = q.conn.as_ref()
            {
                rs.fetching = true;
                conn.handle.fetch_more(q.last_run);
            }
        }
    }

    pub(super) fn active_result_set(&mut self) -> Option<&mut crate::grid::ResultSet> {
        let q = self.query_mut()?;
        if q.on_messages() {
            return None;
        }
        let i = q.active_result;
        q.results.get_mut(i)
    }

    /// Keys in the results pane. Returns false if the command is not for it.
    pub(super) fn results_command(&mut self, cmd: Command) -> bool {
        let step = self.grid_step(cmd);
        let Some(q) = self.query_mut() else {
            return false;
        };
        if q.on_messages() {
            let n = q.messages.len();
            let cursor = q.message_cursor;
            let moved = match cmd {
                Command::Up => Some(cursor.saturating_sub(1)),
                Command::Down => Some((cursor + 1).min(n.saturating_sub(1))),
                Command::Home | Command::DocStart => Some(0),
                Command::End | Command::DocEnd => Some(n.saturating_sub(1)),
                _ => None,
            };
            if let Some(to) = moved {
                // moving ends the Ctrl+A selection (D-019)
                q.message_cursor = to;
                q.messages_all_selected = false;
                return true;
            }
            let text = match cmd {
                Command::SelectAll => {
                    q.messages_all_selected = n > 0;
                    return true;
                }
                Command::Copy => q.messages_copy_text(),
                Command::CopyAllMessages => q.all_messages_text(),
                Command::Accept => {
                    self.jump_to_message();
                    return true;
                }
                _ => return false,
            };
            if let Some(text) = text
                && self.clipboard.copy(&text)
            {
                self.effects.push(Effect::Osc52(text));
            }
            return true;
        }
        let i = q.active_result;
        let Some(rs) = q.results.get_mut(i) else {
            return matches!(
                cmd,
                Command::Up | Command::Down | Command::Left | Command::Right
            );
        };
        if let Some((rows, columns)) = step {
            rs.move_cursor(rows, columns);
            self.after_grid_move();
            return true;
        }
        match cmd {
            Command::SelectAll => rs.all_selected = true,
            Command::Copy => {
                if let Some(text) = rs.copy_text()
                    && self.clipboard.copy(&text)
                {
                    self.effects.push(Effect::Osc52(text));
                }
                return true;
            }
            _ => return false,
        }
        self.after_grid_move();
        true
    }

    /// Enter (or a double click) on a message: to its place in the editor; with a
    /// known column the token there is selected (D-019).
    fn jump_to_message(&mut self) {
        let Some((pos, exact)) = self
            .query()
            .and_then(|q| q.messages.get(q.message_cursor))
            .and_then(|m| Some((m.jump?, m.exact)))
        else {
            return;
        };
        self.focus = Focus::Tabs;
        let editor = self.editor();
        editor.set_cursor(pos, false);
        if exact && let Some((start, end)) = token_at(editor, editor.cursor()) {
            editor.set_cursor(start, false);
            editor.set_cursor(end, true);
        }
    }

    /// `double`: the second click of a double click, which jumps like Enter.
    pub(super) fn click_results(&mut self, p: Position, hits: &Hits, double: bool) {
        let grid = hits.grid;
        let Some(q) = self.query_mut() else {
            return;
        };
        if q.on_messages() {
            let i = (p.y - grid.y) as usize;
            if i < q.messages.len() {
                q.messages_all_selected = false;
                q.message_cursor = hits
                    .message_rows
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(m, _)| *m)
                    .unwrap_or(q.message_cursor);
                if double {
                    self.jump_to_message();
                }
            }
            return;
        }
        if let Some(rs) = q.results.get_mut(q.active_result)
            && let Some(&(row, col)) = hits
                .grid_cells
                .iter()
                .find(|(_, r)| r.contains(p))
                .map(|(rc, _)| rc)
        {
            rs.row = row;
            rs.col = col;
            rs.all_selected = false;
        }
        self.after_grid_move();
    }
}

/// Char range of the editor token at char index `pos` (not whitespace).
fn token_at(editor: &pgc_editor::Editor, pos: usize) -> Option<(usize, usize)> {
    let line = editor.line_of_char(pos);
    let start = editor.line_to_char(line);
    let (text, tokens) = editor.highlighted_lines(line, 1).pop()?;
    let byte = text
        .char_indices()
        .nth(pos - start)
        .map_or(text.len(), |(b, _)| b);
    let t = tokens
        .iter()
        .find(|t| t.start <= byte && byte < t.end && !text[t.start..t.end].trim().is_empty())?;
    let chars = |b: usize| start + text[..b].chars().count();
    Some((chars(t.start), chars(t.end)))
}
