//! `F1` (D-063): a window with only the keys that do something now — the keys of the
//! focused frame (or of the dialog on top) and the global ones, without the commands
//! that cannot run in the current state. Help → Keyboard shortcuts lists every key.

use super::{App, Focus};
use crate::help::{self, KeyRows};
use crate::keymap::{Command, Context};
use crate::tree::RowKind;

impl App {
    /// `F1`: the keys that work now.
    pub(super) fn open_key_help(&mut self) {
        if self.viewer_open() {
            return;
        }
        let mut d = help::viewer("Keys available now", self.key_help_lines());
        d.fit_text(self.screen);
        self.dialogs.push(d);
    }

    /// A dialog (or the template form) has the keys.
    fn dialog_has_keys(&self) -> bool {
        !self.dialogs.is_empty() || self.template.is_some()
    }

    /// Text of the `F1` window: the focused frame, *Everywhere*, *Other keys*.
    pub(super) fn key_help_lines(&self) -> help::Lines {
        let dialog = self.dialog_has_keys();
        let ctx = if dialog {
            Context::Dialog
        } else {
            self.context().unwrap_or(Context::Global)
        };
        // the keys of `section` that reach their command now (a context key hides
        // the global one) and whose command can run
        let rows = |section: Context| -> KeyRows {
            help::command_rows(|cmd| {
                if !self.key_works(cmd) {
                    return Vec::new();
                }
                let mut keys = self.keymap.keys_for(section, cmd);
                keys.retain(|k| self.command_for(*k) == Some(cmd));
                keys
            })
        };
        let global = if ctx == Context::Global {
            KeyRows::new()
        } else {
            rows(Context::Global)
        };
        let letter = if dialog {
            help::FIELD_LETTER
        } else {
            help::MENU_LETTER
        };
        let other = [help::ESC_DIGIT, letter]
            .iter()
            .map(|&(k, d)| (k.to_owned(), d, crate::keymap::Group::General))
            .collect();
        let mut lines = help::key_sections(&[
            (self.frame_name(ctx), rows(ctx)),
            ("Everywhere", global),
            ("Other keys", other),
        ]);
        lines.push(String::new(), false);
        lines.push("All keys: Help → Keyboard shortcuts".to_owned(), false);
        lines
    }

    /// Name of the focused frame, as the heading of its keys.
    fn frame_name(&self, ctx: Context) -> &'static str {
        match ctx {
            Context::Editor if self.query().is_some() => "Query editor",
            Context::Editor if self.list_tab() => "Statement pane",
            Context::Editor => "Object view",
            Context::Results if self.query().is_some_and(|q| q.on_messages()) => "Messages",
            _ => help::SECTIONS
                .iter()
                .find(|(c, _)| *c == ctx)
                .map_or("", |(_, title)| title),
        }
    }

    /// Whether the key of `cmd` does something in the current state. Commands with a
    /// menu item follow [`App::is_enabled`]; the keys without one are decided here.
    fn key_works(&self, cmd: Command) -> bool {
        use Command as C;
        if self.dialog_has_keys() {
            // a dialog has its own keys; of the global ones only Help reaches it
            return !matches!(cmd, C::Menu | C::Quit);
        }
        let in_tree = self.focus == Focus::Tree;
        let in_view = self.focus == Focus::Tabs;
        let editable = in_view && !self.tabs[self.active_tab].view.editor.read_only();
        let tree_rows = !self.tree.servers.is_empty();
        match cmd {
            C::Help | C::Menu | C::SwitchPanel => true,
            C::FocusTree => !in_tree,
            C::FocusRight => in_tree,
            C::Up
            | C::Down
            | C::Left
            | C::Right
            | C::PageUp
            | C::PageDown
            | C::Home
            | C::End
            | C::DocStart
            | C::DocEnd => !in_tree || tree_rows,
            C::Accept if in_tree => self.tree.selected().is_some(),
            // Enter on an error message jumps to its line (D-019)
            C::Accept => self.query().is_some_and(|q| {
                q.on_messages()
                    && q.messages
                        .get(q.message_cursor)
                        .is_some_and(|m| m.jump.is_some())
            }),
            C::ContextMenu | C::QuickSearch if in_tree => tree_rows,
            C::ContextMenu => true,
            // F8 only deletes saved connections (server rows)
            C::DropObject => self
                .tree
                .selected()
                .is_some_and(|r| r.kind == RowKind::Server),
            C::WordLeft
            | C::WordRight
            | C::SelectLeft
            | C::SelectRight
            | C::SelectUp
            | C::SelectDown
            | C::SelectWordLeft
            | C::SelectWordRight
            | C::SelectHome
            | C::SelectEnd
            | C::SelectPageUp
            | C::SelectPageDown
            | C::SelectDocStart
            | C::SelectDocEnd
            | C::Find
            | C::FindNext
            | C::FindPrev => in_view,
            C::Copy | C::SelectAll if self.db_stats().is_none() => true,
            // not in a read-only view
            C::Cut
            | C::Paste
            | C::Undo
            | C::Redo
            | C::Indent
            | C::Unindent
            | C::Newline
            | C::Backspace
            | C::Delete
            | C::DeleteWordLeft
            | C::DeleteWordRight
            | C::DeleteLine => editable,
            C::Complete => in_view && self.query().is_some(),
            C::ResultsSmaller | C::ResultsLarger if !self.list_tab() => {
                self.query().is_some_and(|q| q.results_visible)
            }
            _ => self.is_enabled(cmd),
        }
    }
}
