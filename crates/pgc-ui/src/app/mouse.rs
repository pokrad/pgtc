//! Mouse input (D-016): pane borders, editor, results, tree, menus and dialogs.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{App, Focus};
use crate::keymap::Command;
use crate::menu::{self, MenuEntry};
use crate::view::Hits;

impl App {
    pub(super) fn on_mouse(&mut self, m: MouseEvent, hits: &Hits) {
        // any mouse action closes the autocomplete list (D-052)
        if !matches!(m.kind, MouseEventKind::Moved) {
            self.completion = None;
        }
        let p = Position::new(m.column, m.row);
        if self.template.is_some() && self.dialogs.is_empty() {
            // an open choice list takes every click and the wheel
            if let Some(form) = self.template.as_mut()
                && form.popup_mouse(m.kind, p)
            {
                return;
            }
            if m.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some((stop, area)) = hits.template.iter().find(|(_, r)| r.contains(p))
                && let Some(form) = self.template.as_mut()
            {
                let event = form.click(*stop, p.y, *area);
                self.form_event(event);
            }
            return;
        }
        // the wheel scrolls an open combo list of a dialog
        if matches!(
            m.kind,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
        ) && let Some(d) = self.dialogs.last_mut()
        {
            let delta = if m.kind == MouseEventKind::ScrollDown {
                3
            } else {
                -3
            };
            // an open combo list first, else a Help window's text
            if !d.combo_wheel(delta) && !d.text_wheel(delta) {
                d.list_wheel(delta);
            }
            return;
        }
        // the border between the tree and the tabs is dragged to resize them (D-045)
        match m.kind {
            MouseEventKind::Down(MouseButton::Left)
                if !self.modal() && hits.pane_border.contains(p) =>
            {
                self.pane_drag = true;
                return;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.pane_drag => {
                let wanted = (p.x + 1).saturating_sub(hits.main.x);
                self.settings.ui.tree_width =
                    Some(crate::view::tree_width(Some(wanted), hits.main.width));
                return;
            }
            MouseEventKind::Up(_) if self.pane_drag => {
                self.pane_drag = false;
                self.save_settings();
                return;
            }
            _ => {}
        }
        // the results bar is the drag handle of the editor / results split (D-019)
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.split_drag = !self.modal() && hits.split.contains(p);
            }
            MouseEventKind::Drag(MouseButton::Left) if self.split_drag => {
                self.drag_split(p.y, hits.split_area);
                return;
            }
            MouseEventKind::Up(_) => self.split_drag = false,
            _ => {}
        }
        // the dashboard tab's lists and grid (D-057)
        if !self.modal() && self.dashboard_mouse(m.kind, p, hits) {
            return;
        }
        if !self.modal() && self.profiler_mouse(m.kind, p, hits) {
            return;
        }
        if !self.modal() && self.db_stats_mouse(m.kind, p, hits) {
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left)
                if !self.modal() && self.editor_area.contains(p) =>
            {
                self.focus = Focus::Tabs;
                self.click_editor(p, false);
            }
            MouseEventKind::Drag(MouseButton::Left)
                if !self.modal() && self.focus == Focus::Tabs && self.editor_area.contains(p) =>
            {
                self.click_editor(p, true);
            }
            MouseEventKind::ScrollDown if !self.modal() && self.editor_area.contains(p) => {
                self.editor().scroll_lines(3);
            }
            MouseEventKind::ScrollUp if !self.modal() && self.editor_area.contains(p) => {
                self.editor().scroll_lines(-3);
            }
            MouseEventKind::Down(MouseButton::Left)
                if !self.modal() && hits.result_bar.iter().any(|(_, r)| r.contains(p)) =>
            {
                if let Some((item, _)) = hits.result_bar.iter().find(|(_, r)| r.contains(p)) {
                    let item = *item;
                    self.focus = Focus::Results;
                    self.toggle_query(|q| {
                        match item {
                            crate::view::BarItem::Messages => q.show_messages = true,
                            crate::view::BarItem::Set(i) => {
                                q.show_messages = false;
                                q.active_result = i;
                            }
                        }
                        String::new()
                    });
                }
            }
            MouseEventKind::Down(MouseButton::Left) if !self.modal() && hits.grid.contains(p) => {
                self.focus = Focus::Results;
                self.click_results(p, hits);
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if !self.modal() && hits.grid.contains(p) =>
            {
                let d = if m.kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if let Some(rs) = self.active_result_set() {
                    rs.move_cursor(d, 0, 0);
                }
                self.after_grid_move();
            }
            MouseEventKind::Down(MouseButton::Left) => self.left_click(p, hits),
            MouseEventKind::Down(MouseButton::Right) if !self.modal() && hits.grid.contains(p) => {
                self.focus = Focus::Results;
                self.click_results(p, hits);
                self.open_results_menu((p.x, p.y + 1));
            }
            MouseEventKind::Down(MouseButton::Right) if !self.modal() && hits.tree.contains(p) => {
                self.focus = Focus::Tree;
                if let Some(i) = self.tree_row_at(p, hits) {
                    self.tree.cursor = i;
                }
                self.run(Command::ContextMenu);
            }
            _ => {}
        }
    }

    /// Moves the editor / results split so that the results bar is on row `y`.
    fn drag_split(&mut self, y: u16, area: Rect) {
        if area.height < 8 {
            return;
        }
        let results_h = area.bottom().saturating_sub(y).clamp(4, area.height - 3);
        // rounded up: the view rounds down, so the bar lands exactly on `y`
        let percent = (results_h as u32 * 100).div_ceil(area.height as u32) as u16;
        // dashboard / profiler: the statement pane is the lower part (D-059)
        if self.list_tab() {
            self.set_pane_percent(percent);
            return;
        }
        if let Some(q) = self.query_mut() {
            q.results_percent = percent.clamp(15, 85);
        }
    }

    /// Mouse in the editor: place the cursor (`select`: extend the selection).
    fn click_editor(&mut self, p: Position, select: bool) {
        let line_numbers = self.settings.editor.line_numbers;
        let area = self.editor_area;
        let editor = self.editor();
        let gutter = pgc_editor::gutter_width(editor, line_numbers);
        let line = editor.top + (p.y - area.y) as usize;
        if line >= editor.len_lines() {
            let end = editor.line_to_char(editor.len_lines() - 1)
                + editor.line(editor.len_lines() - 1).chars().count();
            editor.set_cursor(end, select);
            return;
        }
        let col = editor.left + p.x.saturating_sub(area.x + gutter) as usize;
        let pos = editor.pos_at_col(line, col);
        editor.set_cursor(pos, select);
    }

    /// Index of the tree row at `p`, if any.
    fn tree_row_at(&self, p: Position, hits: &Hits) -> Option<usize> {
        let first = hits.tree_rows.y;
        let last = hits.tree_rows.bottom();
        if p.y < first || p.y >= last {
            return None;
        }
        let i = self.tree.offset + (p.y - first) as usize;
        (i < self.tree.rows().len()).then_some(i)
    }

    fn left_click(&mut self, p: Position, hits: &Hits) {
        if let Some(dialog) = self.dialogs.last_mut() {
            if let Some((outer, _)) = &hits.dialog
                && dialog.combo_click(*outer, hits.screen, p.x, p.y)
            {
                // the click went to (or closed) an open combo list
            } else if let Some((_, areas)) = &hits.dialog
                && let Some(i) = areas.iter().position(|a| a.contains(p))
            {
                let event = dialog.click(i, areas[i], p.x, p.y);
                self.dialog_event(event);
            }
            return;
        }

        if let Some(popup) = self.popup.take() {
            if let Some((i, _)) = hits.popup_items.iter().find(|(_, r)| r.contains(p))
                && let Some(MenuEntry::Item { cmd, .. }) = popup.entries.get(*i)
                && self.is_enabled(*cmd)
            {
                self.run(*cmd);
            } else if hits.popup.is_some_and(|r: Rect| r.contains(p)) {
                self.popup = Some(popup);
            }
            return;
        }

        let title = hits.menu.titles.iter().position(|r| r.contains(p));
        if let Some(state) = self.menu {
            if let Some(i) = title {
                if i == state.menu {
                    self.menu = None;
                } else {
                    self.open_menu(i);
                }
            } else if let Some((i, _)) = hits.menu.items.iter().find(|(_, r)| r.contains(p)) {
                if let Some(c) = menu::entry_command(&self.menus, state.menu, *i)
                    && self.is_enabled(c)
                {
                    self.menu = None;
                    self.run(c);
                }
            } else if !hits.menu.dropdown.is_some_and(|r: Rect| r.contains(p)) {
                self.menu = None;
            }
            return;
        }

        if let Some(i) = title {
            self.open_menu(i);
        } else if let Some(&(i, _)) = hits.tab_close.iter().find(|(_, r)| r.contains(p)) {
            // the tab's close mark: as Ctrl+W, with its questions (D-043)
            if let Some(id) = self.tabs.get(i).map(|t| t.id) {
                self.active_tab = i;
                self.focus = Focus::Tabs;
                self.request_close_tab(id, true);
            }
        } else if let Some(i) = hits.tabs.iter().position(|r| r.contains(p)) {
            self.active_tab = i;
            // dashboard / profiler: the list (D-059)
            self.focus = if self.list_tab() || self.db_stats().is_some() {
                Focus::Results
            } else {
                Focus::Tabs
            };
        } else if hits.tree.contains(p) {
            self.focus = Focus::Tree;
            if let Some(i) = self.tree_row_at(p, hits) {
                if self.tree.cursor == i {
                    self.toggle();
                } else {
                    self.tree.cursor = i;
                }
            }
        } else if hits.right.contains(p) && !self.list_tab() && self.db_stats().is_none() {
            self.focus = Focus::Tabs;
        }
    }
}
