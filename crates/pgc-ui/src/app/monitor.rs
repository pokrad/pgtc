//! What the dashboard, profiler and database statistics tabs share in the application
//! (D-057, D-058, D-062): the keys and the mouse of their list, and their status row.

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::query::connection_label;
use super::{App, Effect, Focus, Tab};
use crate::dashboard::OwnConn;
use crate::grid::CellArea;
use crate::keymap::Command;
use crate::monitor::MonitorTab;

impl App {
    /// The dashboard, profiler or statistics of the active tab.
    fn monitor_mut(&mut self) -> Option<&mut dyn MonitorTab> {
        self.tabs[self.active_tab].monitor_mut()
    }

    /// The own connection of the dashboard, profiler or database statistics tab `tab`.
    pub(super) fn own_conn(&mut self, tab: u64) -> Option<OwnConn<'_>> {
        self.tab_mut(tab)?.monitor_mut().map(|m| m.own_conn())
    }

    /// How far a key moves the cell cursor of a grid, as (rows, columns): the arrows,
    /// a page, the ends of the row and of the grid. `None` for other commands.
    pub(super) fn grid_step(&self, cmd: Command) -> Option<(isize, isize)> {
        let page = self.frame.grid_size.0.saturating_sub(1).max(1) as isize;
        match cmd {
            Command::Up => Some((-1, 0)),
            Command::Down => Some((1, 0)),
            Command::Left => Some((0, -1)),
            Command::Right => Some((0, 1)),
            Command::PageUp => Some((-page, 0)),
            Command::PageDown => Some((page, 0)),
            Command::Home => Some((0, isize::MIN / 2)),
            Command::End => Some((0, isize::MAX / 2)),
            Command::DocStart => Some((isize::MIN / 2, isize::MIN / 2)),
            Command::DocEnd => Some((isize::MAX / 2, 0)),
            _ => None,
        }
    }

    /// Keys of the list: the cursor, copy, select all. Returns false if the command
    /// is not one of them.
    pub(super) fn monitor_command(&mut self, cmd: Command) -> bool {
        if let Some((rows, columns)) = self.grid_step(cmd) {
            if let Some(m) = self.monitor_mut() {
                m.list_mut().move_cursor(rows, columns);
            }
            self.monitor_scroll();
            return true;
        }
        match cmd {
            Command::Copy => {
                let text = self.tabs[self.active_tab]
                    .monitor()
                    .and_then(|m| m.list().copy_text());
                if let Some(text) = text
                    && self.clipboard.copy(&text)
                {
                    self.effects.push(Effect::Osc52(text));
                }
            }
            Command::SelectAll => {
                if let Some(m) = self.monitor_mut() {
                    m.list_mut().all_selected = true;
                }
            }
            _ => return false,
        }
        true
    }

    /// Keeps the list's cursor visible.
    pub(super) fn monitor_scroll(&mut self) {
        let (height, width) = self.frame.grid_size;
        if let Some(m) = self.monitor_mut() {
            m.list_mut().ensure_visible(height.saturating_sub(1), width);
        }
    }

    /// Mouse in the list drawn at `grid` with the cells `cells`: a click moves the
    /// cursor, the wheel scrolls. `None` if the event was not for the list, else
    /// whether the tab's context menu is asked for (right click).
    pub(super) fn monitor_mouse(
        &mut self,
        kind: MouseEventKind,
        p: Position,
        grid: Rect,
        cells: &[CellArea],
    ) -> Option<bool> {
        if !grid.contains(p) {
            return None;
        }
        match kind {
            MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) => {
                self.focus = Focus::Results;
                let cell = cells.iter().find(|(_, r)| r.contains(p)).map(|(rc, _)| *rc);
                if let Some(m) = self.monitor_mut()
                    && let Some((row, col)) = cell
                {
                    let list = m.list_mut();
                    list.row = row;
                    list.col = col;
                    list.all_selected = false;
                }
                Some(button == MouseButton::Right)
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let rows = if kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if let Some(m) = self.monitor_mut() {
                    m.list_mut().move_cursor(rows, 0);
                }
                self.monitor_scroll();
                Some(false)
            }
            _ => None,
        }
    }

    /// The status row of a dashboard, profiler or statistics tab: `server ·
    /// user@database` with the connection's state, and what the tab is doing (D-043).
    pub(crate) fn monitor_status(&self, tab: &Tab) -> Option<(String, String)> {
        let m = tab.monitor()?;
        let (uid, database, connected) = m.place();
        let left = connection_label(self.place_label(uid, Some(database)), connected);
        Some((left, m.status_text()))
    }
}
