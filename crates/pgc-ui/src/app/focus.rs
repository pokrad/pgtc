//! Direct keys for the frames (D-060): the object tree, the right frame, and the upper /
//! lower part of the right frame.
//!
//! Upper and lower part: a query tab's editor and its results / messages; a dashboard
//! or profiler tab's list (`Focus::Results`) and its statement pane (`Focus::Tabs`,
//! D-059).

use super::{App, Focus};

impl App {
    /// Focus of the upper part of the active tab.
    fn upper_focus(&self) -> Focus {
        // the statistics tab (D-062) has only its list
        if self.list_tab() || self.db_stats().is_some() {
            Focus::Results
        } else {
            Focus::Tabs
        }
    }

    /// Focus of the lower part of the active tab.
    fn lower_focus(&self) -> Focus {
        if self.db_stats().is_some() {
            Focus::Results
        } else if self.list_tab() {
            Focus::Tabs
        } else {
            Focus::Results
        }
    }

    /// The active tab has a lower part that is shown now.
    fn lower_shown(&self) -> bool {
        if self.list_tab() {
            self.pane_shown()
        } else {
            self.query().is_some_and(|q| q.results_visible)
        }
    }

    /// `Alt+2`: the part last focused in the tab, at first the upper part.
    pub(super) fn focus_right(&mut self) {
        let lower = self.tabs[self.active_tab].lower_focused && self.lower_shown();
        self.focus = if lower {
            self.lower_focus()
        } else {
            self.upper_focus()
        };
    }

    /// `Alt+Down`: the lower part, shown first if it is hidden.
    pub(super) fn focus_lower(&mut self) {
        let tab = &mut self.tabs[self.active_tab];
        if let Some(q) = tab.query.as_mut() {
            q.results_visible = true;
        } else if tab.profiler.is_some() || tab.dashboard.as_ref().is_some_and(|d| d.has_pane()) {
            if let Some(p) = tab.profiler.as_mut() {
                p.pane.visible = true;
            }
            if let Some(d) = tab.dashboard.as_mut() {
                d.pane.visible = true;
            }
        } else {
            // no lower part: the upper one
            self.focus = self.upper_focus();
            return;
        }
        self.focus = self.lower_focus();
    }

    /// `Alt+Up`: the upper part.
    pub(super) fn focus_upper(&mut self) {
        self.focus = self.upper_focus();
    }

    /// Remembers which part of the active tab has the focus (for `Alt+2`).
    pub(super) fn remember_part(&mut self) {
        if self.focus == Focus::Tree {
            return;
        }
        let lower = self.focus == self.lower_focus();
        self.tabs[self.active_tab].lower_focused = lower;
    }
}
