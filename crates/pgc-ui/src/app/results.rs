//! The results pane of a query tab (D-019, D-041): keys in the grid and in Messages,
//! mouse clicks, fetching more rows, the context menu.

use ratatui::layout::Position;

use super::{App, Effect, Focus};
use crate::keymap::Command;
use crate::menu::{self, Popup};
use crate::view::Hits;

impl App {
    /// Context menu of the results pane (D-042).
    pub(super) fn open_results_menu(&mut self, anchor: (u16, u16)) {
        let entries = vec![
            menu::item("E&xport result…", Command::ExportResult),
            menu::item("&Fetch all", Command::FetchAll),
        ];
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
        let (h, w) = self.grid_size;
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
        let page = self.grid_size.0.saturating_sub(1).max(1) as isize;
        let Some(q) = self.query_mut() else {
            return false;
        };
        if q.on_messages() {
            let n = q.messages.len();
            let cursor = &mut q.message_cursor;
            match cmd {
                Command::Up => *cursor = cursor.saturating_sub(1),
                Command::Down => *cursor = (*cursor + 1).min(n.saturating_sub(1)),
                Command::Home | Command::DocStart => *cursor = 0,
                Command::End | Command::DocEnd => *cursor = n.saturating_sub(1),
                Command::Copy => {
                    let text = q.messages.get(q.message_cursor).map(|m| m.text.clone());
                    if let Some(text) = text
                        && self.clipboard.copy(&text)
                    {
                        self.effects.push(Effect::Osc52(text));
                    }
                }
                Command::Accept => {
                    // Enter on an error: jump to its line (D-019)
                    if let Some(pos) = q.messages.get(q.message_cursor).and_then(|m| m.jump) {
                        self.focus = Focus::Tabs;
                        self.editor().set_cursor(pos, false);
                    }
                }
                _ => return false,
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
        match cmd {
            Command::Up => rs.move_cursor(-1, 0, 0),
            Command::Down => rs.move_cursor(1, 0, 0),
            Command::Left => rs.move_cursor(0, -1, 0),
            Command::Right => rs.move_cursor(0, 1, 0),
            Command::PageUp => rs.move_cursor(-page, 0, 0),
            Command::PageDown => rs.move_cursor(page, 0, 0),
            Command::Home => rs.move_cursor(0, isize::MIN / 2, 0),
            Command::End => rs.move_cursor(0, isize::MAX / 2, 0),
            Command::DocStart => rs.move_cursor(isize::MIN / 2, isize::MIN / 2, 0),
            Command::DocEnd => rs.move_cursor(isize::MAX / 2, 0, 0),
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

    pub(super) fn click_results(&mut self, p: Position, hits: &Hits) {
        let grid = hits.grid;
        let Some(q) = self.query_mut() else {
            return;
        };
        if q.on_messages() {
            let i = (p.y - grid.y) as usize;
            if i < q.messages.len() {
                q.message_cursor = hits
                    .message_rows
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(m, _)| *m)
                    .unwrap_or(q.message_cursor);
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
