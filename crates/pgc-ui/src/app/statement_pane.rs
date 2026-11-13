//! The statement pane of the dashboard and profiler tabs (D-059): the tab's own
//! read-only editor below the list, following the list cursor; the split; `Ctrl+E`.
//!
//! In these tabs the list has the results focus (`Focus::Results`) and the pane the
//! editor focus (`Focus::Tabs`), so the pane gets every editor key for free.

use super::{App, Focus};
use crate::dashboard::Pane;
use crate::tree::ConnState;

impl App {
    /// The active tab is a dashboard or profiler tab (a list with a statement pane).
    pub(crate) fn list_tab(&self) -> bool {
        let t = &self.tabs[self.active_tab];
        t.dashboard.is_some() || t.profiler.is_some()
    }

    fn pane(&self) -> Option<Pane> {
        let t = &self.tabs[self.active_tab];
        t.dashboard
            .as_ref()
            .map(|d| d.pane)
            .or_else(|| t.profiler.as_ref().map(|p| p.pane))
    }

    fn pane_mut(&mut self) -> Option<&mut Pane> {
        let t = &mut self.tabs[self.active_tab];
        match (t.dashboard.as_mut(), t.profiler.as_mut()) {
            (Some(d), _) => Some(&mut d.pane),
            (None, Some(p)) => Some(&mut p.pane),
            (None, None) => None,
        }
    }

    /// The pane is drawn: visible, and part of the shown list (dashboard: Sessions).
    pub(crate) fn pane_shown(&self) -> bool {
        let t = &self.tabs[self.active_tab];
        let applies = t.profiler.is_some() || t.dashboard.as_ref().is_some_and(|d| d.has_pane());
        applies && self.pane().is_some_and(|p| p.visible)
    }

    /// Height of the pane in percent of the tab.
    pub(crate) fn pane_percent(&self) -> u16 {
        self.pane().map_or(33, |p| p.percent)
    }

    /// Statement (with its comments) and database of the row under the list cursor.
    fn list_statement(&self) -> Option<(String, String)> {
        let t = &self.tabs[self.active_tab];
        t.dashboard
            .as_ref()
            .and_then(|d| d.statement())
            .or_else(|| t.profiler.as_ref().and_then(|p| p.statement()))
    }

    /// Puts the statement of the list cursor's row into the pane (unchanged text keeps
    /// the pane's cursor and selection).
    pub(super) fn sync_pane(&mut self) {
        if !self.list_tab() {
            return;
        }
        let text = self.list_statement().map(|(s, _)| s).unwrap_or_default();
        self.tabs[self.active_tab]
            .view
            .set_text(String::new(), text);
    }

    /// Keeps the focus where it can be: on the list when the pane is hidden.
    pub(super) fn fix_list_focus(&mut self) {
        // the statistics tab has only its list (D-062)
        if self.db_stats().is_some() && self.focus == Focus::Tabs {
            self.focus = Focus::Results;
        }
        if self.list_tab() && self.focus == Focus::Tabs && !self.pane_shown() {
            self.focus = Focus::Results;
        }
    }

    /// `Ctrl+R`: shows / hides the pane.
    pub(super) fn toggle_pane(&mut self) {
        if let Some(p) = self.pane_mut() {
            p.visible = !p.visible;
        }
        self.fix_list_focus();
    }

    /// `Alt+Shift+Up` / `Down`: the pane smaller / larger by 5 % (as D-019).
    pub(super) fn resize_pane(&mut self, delta: i16) {
        if let Some(p) = self.pane_mut() {
            p.percent = (p.percent as i16 + delta).clamp(15, 85) as u16;
            p.visible = true;
        }
    }

    /// The split dragged with the mouse: the pane's height in percent.
    pub(super) fn set_pane_percent(&mut self, percent: u16) {
        if let Some(p) = self.pane_mut() {
            p.percent = percent.clamp(15, 85);
        }
    }

    /// `Ctrl+E` in a list tab: the pane's text in a new query tab on the tab's server
    /// and the row's database, as unsaved text (D-059).
    pub(super) fn edit_statement_in_query_tab(&mut self) {
        let Some((_, database)) = self.list_statement() else {
            self.status = "Select a row with a statement first".to_owned();
            return;
        };
        if database.is_empty() {
            self.status = "This row has no database — no query tab can be bound to it".to_owned();
            return;
        }
        let tab = &self.tabs[self.active_tab];
        let text = tab.view.editor.text();
        if text.trim().is_empty() {
            self.status = "The row has no statement".to_owned();
            return;
        }
        let Some(uid) = tab.server else {
            return;
        };
        let connected = self
            .tree
            .servers
            .iter()
            .any(|s| s.uid == uid && s.state == ConnState::Connected);
        if !connected {
            self.status = "Connect the server first".to_owned();
            return;
        }
        self.new_query_tab(Some((uid, database)));
        let editor = &mut self.tabs[self.active_tab].view.editor;
        editor.set_text(&text);
        editor.set_modified(true);
        self.focus = Focus::Tabs;
    }

    /// `Ctrl+E` can run in a list tab.
    pub(super) fn can_edit_statement(&self) -> bool {
        self.list_statement()
            .is_some_and(|(s, db)| !s.trim().is_empty() && !db.is_empty())
    }
}
