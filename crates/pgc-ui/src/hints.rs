//! Key-hint bar at the bottom (D-009, D-016): the most important keys of the focused
//! frame, whatever the key is. Which commands are "important" is fixed here; the keys
//! come from the active keymap, so user bindings show.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::{App, Focus};
use crate::keymap::{Command, Context};
use crate::keys::KeyCombo;
use crate::tree::RowKind;

/// One entry of the bar: the key to show (and press on a click) and its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub key: KeyCombo,
    /// Shown instead of the key's own name (e.g. `←→`).
    pub key_text: Option<&'static str>,
    pub label: &'static str,
}

impl Hint {
    /// Key as shown in the bar: `Ctrl+` is written as `^`.
    pub fn key_text(&self) -> String {
        match self.key_text {
            Some(t) => t.to_owned(),
            None => self.key.to_string().replace("Ctrl+", "^"),
        }
    }
}

use Command as C;

/// (command, label, preferred key). The preferred key is shown when it is bound;
/// otherwise the first bound key.
type Entry = (Command, &'static str, Option<&'static str>);

const TREE: &[Entry] = &[
    (C::Help, "Help", None),
    (C::ContextMenu, "Menu", None),
    (C::ShowInNewTab, "New tab", None),
    (C::NewQuery, "New query", None),
    (C::RefreshTree, "Refresh", None),
    (C::QuickSearch, "Search", Some("Alt+S")),
    (C::DropObject, "Delete", None),
    (C::SwitchPanel, "Right", Some("F6")),
    (C::Menu, "Menu bar", None),
    (C::Quit, "Quit", None),
];

const QUERY_EDITOR: &[Entry] = &[
    (C::Help, "Help", None),
    (C::Execute, "Exec", None),
    (C::CancelQuery, "Cancel", None),
    (C::NewFromTemplate, "Template", None),
    (C::Psql, "psql", None),
    (C::Find, "Find", None),
    (C::ToggleResults, "Results", None),
    (C::CloseTab, "Close tab", None),
    (C::SwitchPanel, "Tree", Some("F6")),
    (C::Menu, "Menu bar", None),
    (C::Quit, "Quit", None),
];

const OBJECT_VIEW: &[Entry] = &[
    (C::Help, "Help", None),
    (C::Find, "Find", None),
    (C::FindNext, "Find next", None),
    (C::Copy, "Copy", Some("Ctrl+C")),
    (C::NewQuery, "New query", None),
    (C::SwitchPanel, "Tree", Some("F6")),
    (C::Menu, "Menu bar", None),
    (C::Quit, "Quit", None),
];

const RESULTS: &[Entry] = &[
    (C::Help, "Help", None),
    (C::Execute, "Exec", None),
    (C::CancelQuery, "Cancel", None),
    (C::ShowMessages, "Messages", None),
    (C::NextResult, "Next result", None),
    (C::FetchAll, "Fetch all", None),
    (C::ContextMenu, "Menu", None),
    (C::Copy, "Copy", Some("Ctrl+C")),
    (C::ToggleResults, "Hide", None),
    (C::SwitchPanel, "Tree", Some("F6")),
    (C::Quit, "Quit", None),
];

const DASHBOARD: &[Entry] = &[
    (C::Help, "Help", None),
    (C::ContextMenu, "Menu", None),
    (C::DashboardRefresh, "Refresh", None),
    (C::DashboardPause, "Pause", None),
    (C::NextList, "Next list", Some("Alt+Right")),
    (C::HideIdleSessions, "Idle", None),
    (C::CancelBackend, "Cancel", None),
    (C::TerminateBackend, "Terminate", None),
    (C::EditInQueryTab, "Edit", None),
    (C::CloseTab, "Close tab", None),
    (C::SwitchPanel, "Tree", Some("F6")),
    (C::Quit, "Quit", None),
];

const PROFILER: &[Entry] = &[
    (C::Help, "Help", None),
    (C::ContextMenu, "Menu", None),
    (C::ProfilerStartStop, "Start/Stop", None),
    (C::ProfilerPause, "Pause", None),
    (C::ProfilerFilter, "Filter", None),
    (C::ProfilerClear, "Clear", None),
    (C::EditInQueryTab, "Edit", None),
    (C::CloseTab, "Close tab", None),
    (C::SwitchPanel, "Tree", Some("F6")),
    (C::Quit, "Quit", None),
];

const DB_STATS: &[Entry] = &[
    (C::Help, "Help", None),
    (C::ContextMenu, "Menu", None),
    (C::DbStatsRefresh, "Refresh", None),
    (C::Copy, "Copy", Some("Ctrl+C")),
    (C::CloseTab, "Close tab", None),
    (C::SwitchPanel, "Tree", Some("F6")),
    (C::Quit, "Quit", None),
];

/// The statement pane of a dashboard / profiler tab (D-059).
const PANE: &[Entry] = &[
    (C::Help, "Help", None),
    (C::Find, "Find", None),
    (C::FindNext, "Find next", None),
    (C::Copy, "Copy", Some("Ctrl+C")),
    (C::EditInQueryTab, "Edit", None),
    (C::ToggleResults, "Hide", None),
    (C::CloseTab, "Close tab", None),
    (C::SwitchPanel, "Tree", Some("F6")),
    (C::Quit, "Quit", None),
];

const DIALOG: &[Entry] = &[
    (C::Accept, "OK", None),
    (C::Cancel, "Cancel", Some("Esc")),
    (C::NextField, "Next field", Some("Tab")),
    (C::Toggle, "Toggle", None),
];

fn fixed(code: KeyCode, key_text: Option<&'static str>, label: &'static str) -> Hint {
    Hint {
        key: KeyCombo::new(code, KeyModifiers::NONE),
        key_text,
        label,
    }
}

impl App {
    /// Entries of the key-hint bar for the current state, in priority order.
    pub fn key_hints(&self) -> Vec<Hint> {
        if self.menu.is_some() {
            return vec![
                fixed(KeyCode::Right, Some("←→"), "Menus"),
                fixed(KeyCode::Enter, None, "Choose"),
                fixed(KeyCode::Esc, None, "Close"),
            ];
        }
        if self.completion.as_ref().is_some_and(|p| p.shown()) {
            return vec![
                fixed(KeyCode::Down, Some("↑↓"), "Choose"),
                fixed(KeyCode::Enter, None, "Insert"),
                fixed(KeyCode::Esc, None, "Close"),
            ];
        }
        if self.popup.is_some() {
            return vec![
                fixed(KeyCode::Enter, None, "Choose"),
                fixed(KeyCode::Esc, None, "Close"),
            ];
        }
        if self
            .dialogs
            .last()
            .is_some_and(|d| d.kind == crate::dialog::DialogKind::Viewer)
        {
            return vec![
                fixed(KeyCode::Down, Some("↑↓"), "Scroll"),
                fixed(KeyCode::PageDown, Some("PgUp/PgDn"), "Page"),
                fixed(KeyCode::Esc, None, "Close"),
            ];
        }
        if self
            .dialogs
            .last()
            .is_some_and(|d| d.kind == crate::dialog::DialogKind::PsqlCommand)
        {
            return vec![
                fixed(KeyCode::Down, Some("↑↓"), "Choose"),
                fixed(KeyCode::Enter, None, "Run"),
                fixed(KeyCode::Esc, None, "Close"),
            ];
        }
        let (ctx, entries) = if !self.dialogs.is_empty() || self.template.is_some() {
            (Context::Dialog, DIALOG)
        } else {
            match self.focus {
                Focus::Tree => (Context::Tree, TREE),
                // dashboard / profiler: the list has the results focus (D-059)
                Focus::Results if self.tabs[self.active_tab].dashboard().is_some() => {
                    (Context::Dashboard, DASHBOARD)
                }
                Focus::Results if self.tabs[self.active_tab].profiler().is_some() => {
                    (Context::Profiler, PROFILER)
                }
                Focus::Results if self.tabs[self.active_tab].db_stats().is_some() => {
                    (Context::DbStats, DB_STATS)
                }
                Focus::Results => (Context::Results, RESULTS),
                Focus::Tabs if self.tabs[self.active_tab].query().is_some() => {
                    (Context::Editor, QUERY_EDITOR)
                }
                Focus::Tabs if self.list_tab() => (Context::Editor, PANE),
                Focus::Tabs => (Context::Editor, OBJECT_VIEW),
            }
        };
        entries
            .iter()
            .filter(|(cmd, _, _)| self.hint_available(*cmd))
            .filter_map(|&(cmd, label, prefer)| {
                let key = self.hint_key(ctx, cmd, prefer)?;
                Some(Hint {
                    key,
                    key_text: None,
                    label,
                })
            })
            .collect()
    }

    /// The key that runs `cmd` in `ctx` (a global key only if `ctx` does not
    /// override it).
    fn hint_key(&self, ctx: Context, cmd: Command, prefer: Option<&str>) -> Option<KeyCombo> {
        let km = &self.keymap;
        let mut keys = km.keys_for(ctx, cmd);
        if ctx != Context::Dialog {
            keys.extend(km.keys_for(Context::Global, cmd));
        }
        keys.retain(|k| {
            let found = if ctx == Context::Dialog {
                km.lookup_only(ctx, *k)
            } else {
                km.lookup(ctx, *k)
            };
            found == Some(cmd)
        });
        prefer
            .and_then(|p| keys.iter().find(|k| k.to_string() == p).copied())
            .or_else(|| keys.first().copied())
    }

    /// Hints for commands that cannot run now are left out (D-016).
    fn hint_available(&self, cmd: Command) -> bool {
        match cmd {
            C::Execute
            | C::CancelQuery
            | C::FetchAll
            | C::NewQuery
            | C::RefreshTree
            | C::ShowInNewTab
            | C::CloseTab
            | C::NewFromTemplate
            | C::Psql
            | C::CancelBackend
            | C::TerminateBackend
            | C::EditInQueryTab => self.is_enabled(cmd),
            // F8 only deletes saved connections (server rows)
            C::DropObject => self
                .tree
                .selected()
                .is_some_and(|r| r.kind == RowKind::Server),
            C::NextResult => self
                .tabs
                .get(self.active_tab)
                .and_then(|t| t.query())
                .is_some_and(|q| !q.show_messages && q.results.len() > 1),
            _ => true,
        }
    }
}
