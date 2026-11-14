//! The editor of the active tab (D-015, D-038): editing actions, clipboard, find and
//! replace.

use pgc_editor::{Action, Editor, Motion, SearchOptions};

use super::{App, Effect};
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::keymap::Command;

impl App {
    pub(super) fn tab_width(&self) -> usize {
        self.settings.editor.tab_width.max(1) as usize
    }

    pub(super) fn editor(&mut self) -> &mut Editor {
        &mut self.tabs[self.active_tab].view.editor
    }

    /// Applies an editing action to the active tab's editor.
    pub(super) fn edit(&mut self, action: Action) {
        if !self.editor().apply(action) {
            self.status = "Read-only view".to_owned();
        }
    }

    /// Copy (or cut) the selection to the clipboard (D-023).
    pub(super) fn copy(&mut self, cut: bool) {
        let text = if cut {
            if self.editor().read_only() {
                self.status = "Read-only view".to_owned();
                return;
            }
            self.editor().cut()
        } else {
            self.editor().selected_text()
        };
        let Some(text) = text else {
            return;
        };
        if self.clipboard.copy(&text) {
            self.effects.push(Effect::Osc52(text));
        }
    }

    /// Ctrl+F: the find / replace dialog (D-038).
    pub(super) fn open_find(&mut self) {
        let (mut opts, replacement) = self.last_search.clone().unwrap_or_default();
        if let Some(sel) = self.editor().selected_text().filter(|s| !s.contains('\n')) {
            opts.text = sel;
        }
        let lw = 15;
        let read_only = self.editor().read_only();
        let mut widgets = vec![
            Widget::field("find", "Search &for:", &opts.text, lw),
            Widget::field("replace", "Replace &with:", &replacement, lw),
            Widget::label(""),
            Widget::checkbox("case", "Case &sensitive", opts.case_sensitive),
            Widget::checkbox("words", "W&hole words", opts.whole_words),
            Widget::checkbox("regex", "Regular e&xpression", opts.regex),
            Widget::button("find", "Fi&nd", true),
        ];
        if !read_only {
            widgets.push(Widget::button("replace", "&Replace", false));
            widgets.push(Widget::button("all", "Replace &all", false));
        }
        widgets.push(Widget::button("cancel", "&Cancel", false));
        self.dialogs
            .push(Dialog::new(DialogKind::Find, "Find", 60, widgets));
    }

    pub(super) fn find_dialog_done(&mut self, dialog: Dialog, button: &str) {
        let opts = SearchOptions {
            text: dialog.text("find").unwrap_or("").to_owned(),
            case_sensitive: dialog.checked("case"),
            whole_words: dialog.checked("words"),
            regex: dialog.checked("regex"),
        };
        let replacement = dialog.text("replace").unwrap_or("").to_owned();
        if let Err(e) = opts.compile() {
            self.dialogs.push(dialog);
            self.error(&format!("Invalid regular expression:\n{e}"));
            return;
        }
        if opts.text.is_empty() {
            return;
        }
        self.last_search = Some((opts.clone(), replacement.clone()));
        match button {
            "find" => self.find_again(true),
            "replace" => {
                if self.editor().selection_matches(&opts) {
                    self.confirm_replace();
                } else {
                    self.replace_next(false);
                }
            }
            "all" => self.replace_all(),
            _ => {}
        }
    }

    /// F3 / Shift+F3: find again with the last options.
    pub(super) fn find_again(&mut self, forward: bool) {
        let Some((opts, _)) = self.last_search.clone() else {
            self.open_find();
            return;
        };
        self.status = match self.editor().find(&opts, forward) {
            Ok(Some(found)) if found.wrapped => format!("\"{}\" — search wrapped", opts.text),
            Ok(Some(_)) => format!("\"{}\"", opts.text),
            Ok(None) => format!("\"{}\" not found", opts.text),
            Err(e) => e,
        };
    }

    /// mc-style replace: find the next match and ask (D-038). `after_replace`: stop
    /// when the search wraps (so replaced text is not visited again).
    fn replace_next(&mut self, after_replace: bool) {
        let Some((opts, _)) = self.last_search.clone() else {
            return;
        };
        match self.editor().find(&opts, true) {
            Ok(Some(found)) if !(after_replace && found.wrapped) => self.confirm_replace(),
            Ok(Some(_)) => {
                let editor = self.editor();
                editor.set_cursor(editor.cursor(), false);
                self.status = "Replace: end of text reached".to_owned();
            }
            Ok(None) => self.status = format!("\"{}\" not found", opts.text),
            Err(e) => self.status = e,
        }
    }

    fn confirm_replace(&mut self) {
        self.dialogs.push(Dialog::confirm_buttons(
            DialogKind::ReplaceConfirm,
            "Replace",
            "Replace this occurrence?",
            &[
                ("replace", "&Replace"),
                ("all", "A&ll"),
                ("skip", "&Skip"),
                ("cancel", "&Cancel"),
            ],
        ));
    }

    pub(super) fn replace_confirm_done(&mut self, button: &str) {
        let Some((opts, replacement)) = self.last_search.clone() else {
            return;
        };
        match button {
            "replace" => {
                self.editor().replace_selected(&opts, &replacement);
                self.replace_next(true);
            }
            "skip" => self.replace_next(true),
            "all" => self.replace_all(),
            _ => {}
        }
    }

    fn replace_all(&mut self) {
        let Some((opts, replacement)) = self.last_search.clone() else {
            return;
        };
        self.status = match self.editor().replace_all(&opts, &replacement) {
            Ok(n) => format!("{n} occurrence(s) replaced"),
            Err(e) => e,
        };
    }
}

/// Keymap command → editor action (D-038).
pub(super) fn editor_action(cmd: Command) -> Option<Action> {
    use Motion as M;
    Some(match cmd {
        Command::Up => Action::Move(M::Up, false),
        Command::Down => Action::Move(M::Down, false),
        Command::Left => Action::Move(M::Left, false),
        Command::Right => Action::Move(M::Right, false),
        Command::Home => Action::Move(M::Home, false),
        Command::End => Action::Move(M::End, false),
        Command::PageUp => Action::Move(M::PageUp, false),
        Command::PageDown => Action::Move(M::PageDown, false),
        Command::DocStart => Action::Move(M::DocStart, false),
        Command::DocEnd => Action::Move(M::DocEnd, false),
        Command::WordLeft => Action::Move(M::WordLeft, false),
        Command::WordRight => Action::Move(M::WordRight, false),
        Command::SelectUp => Action::Move(M::Up, true),
        Command::SelectDown => Action::Move(M::Down, true),
        Command::SelectLeft => Action::Move(M::Left, true),
        Command::SelectRight => Action::Move(M::Right, true),
        Command::SelectHome => Action::Move(M::Home, true),
        Command::SelectEnd => Action::Move(M::End, true),
        Command::SelectPageUp => Action::Move(M::PageUp, true),
        Command::SelectPageDown => Action::Move(M::PageDown, true),
        Command::SelectDocStart => Action::Move(M::DocStart, true),
        Command::SelectDocEnd => Action::Move(M::DocEnd, true),
        Command::SelectWordLeft => Action::Move(M::WordLeft, true),
        Command::SelectWordRight => Action::Move(M::WordRight, true),
        Command::SelectAll => Action::SelectAll,
        Command::Undo => Action::Undo,
        Command::Redo => Action::Redo,
        Command::Indent => Action::Indent,
        Command::Unindent => Action::Unindent,
        Command::Newline => Action::Newline,
        Command::Backspace => Action::Backspace,
        Command::Delete => Action::Delete,
        Command::DeleteWordLeft => Action::DeleteWordLeft,
        Command::DeleteWordRight => Action::DeleteWordRight,
        Command::DeleteLine => Action::DeleteLine,
        _ => return None,
    })
}
