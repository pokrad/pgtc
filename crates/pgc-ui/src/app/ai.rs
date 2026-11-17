//! AI assistant (D-073): the `F8` dialog, the structure read through the query tab's
//! connection, the request in the background, the answer in the editor (never run),
//! and Settings → AI….

use std::sync::Arc;

use pgc_core::fsutil::{Access, atomic_write};
use pgc_core::settings::AiProtocol;
use pgc_editor::Action;
use pgc_sql::RenderContext;

use super::lookups::LookupTarget;
use super::state::AiRun;
use super::{App, CONNECTION_BUSY};
use crate::ai::{self, Structure};
use crate::dialog::{Button, Dialog, DialogKind, Widget};

/// Where the answers of AI requests go (set by the event loop).
pub type AiSink = Arc<dyn Fn(AiEvent) + Send + Sync>;

/// The answer of AI request `id`: the text, or an error for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiEvent {
    pub id: u64,
    pub result: Result<String, String>,
}

const NOT_SET_UP: &str = "AI is not set up (Settings → AI…)";
const SCOPE_ALL: &str = "Whole database";
const PROMPT_ROWS: u16 = 6;
/// Characters of the fixed instructions (for the token estimate).
const INSTRUCTION_CHARS: usize = 700;
/// What the key field shows for a saved key (masked: eight stars).
const SAVED_KEY: &str = "********";
/// Label column of the AI settings dialog.
const AI_LABEL_WIDTH: u16 = 12;

impl App {
    /// Where the AI answers report to (set by the event loop). Without it (tests) the
    /// request is kept in [`AiState::unsent`].
    pub fn set_ai_sink(&mut self, sink: AiSink) {
        self.ai.sink = Some(sink);
    }

    /// Why `F8` does not work now, or `Ok`.
    pub(super) fn ai_ready(&self) -> Result<(), String> {
        if !ai::configured(&self.settings.ai) {
            return Err(NOT_SET_UP.to_owned());
        }
        let Some(q) = self.query() else {
            return Err("Ask AI works in a query tab".to_owned());
        };
        if !q.connected() {
            return Err("Connect the query tab first".to_owned());
        }
        if self.ai_not_allowed() {
            return Err("AI is not allowed for this connection".to_owned());
        }
        if q.busy() || q.export.is_some() {
            return Err(CONNECTION_BUSY.to_owned());
        }
        if self.ai.run.is_some() {
            return Err("An AI request is still running (Esc cancels it)".to_owned());
        }
        Ok(())
    }

    /// The active query tab's server has *AI not allowed* (D-073).
    fn ai_not_allowed(&self) -> bool {
        self.query()
            .and_then(|q| q.binding.as_ref())
            .and_then(|(uid, _)| self.tree.servers.iter().find(|s| s.uid == *uid))
            .is_some_and(|s| s.conn.ai_not_allowed)
    }

    /// `F8` / Tools → Ask AI…: the dialog opens at once; the structure is read meanwhile.
    pub(super) fn open_ai_dialog(&mut self) {
        if let Err(why) = self.ai_ready() {
            self.status = why;
            return;
        }
        let Some(version) = self.tab_server_version() else {
            return;
        };
        let ctx = RenderContext {
            version,
            show_system: false,
        };
        let sql = match self.sql.render("ai", "structure.sql", ctx) {
            Ok(s) => s,
            Err(e) => {
                self.error(&e.to_string());
                return;
            }
        };
        let Some(handle) = self
            .query()
            .and_then(|q| q.conn.as_ref())
            .map(|c| c.handle.clone())
        else {
            return;
        };
        let tab = self.tabs[self.active_tab].id;
        self.next_key += 1;
        self.template_lookups
            .insert(self.next_key, LookupTarget::AiStructure { tab });
        handle.lookup_with(self.next_key, sql, Vec::new());
        self.ai.structure = None;
        self.ai.scope_seen = None;
        self.ai.history_pos = None;
        let dialog = Dialog::new(
            DialogKind::AskAi { tab },
            "Ask AI",
            76,
            vec![
                Widget::memo("prompt", "&Prompt", "", PROMPT_ROWS),
                Widget::combo("scope", "Sc&ope:", vec![SCOPE_ALL.to_owned()], 0, 8),
                Widget::label(""),
                Widget::label_id("info", "Reading the structure…"),
                Widget::label_id("note", ""),
                Widget::button(Button::Ok, "&Send", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        );
        self.dialogs.push(dialog);
        self.status = "The answer goes into the editor; nothing is run".to_owned();
    }

    /// The structure for the `F8` dialog of tab `tab`.
    pub(super) fn ai_structure_loaded(
        &mut self,
        tab: u64,
        result: Result<Vec<Vec<String>>, String>,
    ) {
        let Some(d) = self
            .dialogs
            .iter_mut()
            .find(|d| d.kind == DialogKind::AskAi { tab })
        else {
            return;
        };
        match result {
            Ok(rows) => {
                let s = Structure::from_rows(rows);
                let mut scopes = vec![SCOPE_ALL.to_owned()];
                scopes.extend(s.schemas().into_iter().map(|n| format!("Schema {n}")));
                d.set_combo("scope", scopes, 0);
                self.ai.structure = Some((tab, s));
                self.ai.scope_seen = None;
            }
            Err(e) => d.set_label("info", &format!("Reading the structure failed: {e}")),
        }
    }

    /// The schema the dialog's scope names (`None`: the whole database).
    fn ai_scope(d: &Dialog) -> Option<String> {
        d.selected_combo_item("scope")
            .and_then(|s| s.strip_prefix("Schema "))
            .map(str::to_owned)
    }

    /// The editor text of tab `i` as it is sent.
    fn ai_editor_text(&self, i: usize) -> Option<ai::EditorText> {
        let e = &self.tabs[i].view.editor;
        ai::editor_text(&e.text(), e.cursor(), e.selection(), ai::EDITOR_LIMIT)
    }

    /// After each update: the info line follows the chosen scope.
    pub(super) fn sync_ai_dialog(&mut self) {
        let Some(d) = self.dialogs.last() else {
            return;
        };
        let DialogKind::AskAi { tab } = d.kind else {
            return;
        };
        let scope = d.selected("scope").unwrap_or(0);
        if self.ai.scope_seen == Some(scope) {
            return;
        }
        let Some((_, s)) = self.ai.structure.as_ref().filter(|(t, _)| *t == tab) else {
            return;
        };
        let schema = Self::ai_scope(d);
        let structure = s.text(schema.as_deref()).chars().count();
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let editor = self.ai_editor_text(i);
        let chars = INSTRUCTION_CHARS + structure + editor.as_ref().map_or(0, |e| e.text.len());
        let info = format!(
            "Sends about {} tokens to {} (no table data).",
            group(ai::tokens(chars)),
            ai::host(&self.settings.ai.base_url)
        );
        let note = match &editor {
            None => "The editor is empty: only the structure and the prompt are sent.",
            Some(e) if e.cut => "The editor text is long: only the part around the cursor is sent.",
            Some(_) => "",
        };
        self.ai.scope_seen = Some(scope);
        if let Some(d) = self.dialogs.last_mut() {
            d.set_label("info", &info);
            d.set_label("note", note);
        }
    }

    /// `↑` / `↓` in the prompt box when it is empty or shows an earlier prompt: the
    /// prompts of this session. True when the key was used.
    pub(super) fn ai_history_key(&mut self, up: bool) -> bool {
        let Some(d) = self.dialogs.last_mut() else {
            return false;
        };
        if !matches!(d.kind, DialogKind::AskAi { .. }) || !d.memo_focused() {
            return false;
        }
        let text = d.memo_text("prompt").unwrap_or_default();
        let shown = self.ai.history_pos.and_then(|p| self.ai.history.get(p));
        if !(text.is_empty() || shown == Some(&text)) || self.ai.history.is_empty() {
            return false;
        }
        let last = self.ai.history.len() - 1;
        let pos = match (self.ai.history_pos, up) {
            (None, true) => Some(last),
            (None, false) => return false,
            (Some(p), true) => Some(p.saturating_sub(1)),
            (Some(p), false) if p < last => Some(p + 1),
            (Some(_), false) => None,
        };
        self.ai.history_pos = pos;
        let text = pos.map_or("", |p| self.ai.history[p].as_str());
        d.set_memo_text("prompt", text);
        true
    }

    /// Send in the `F8` dialog.
    pub(super) fn ask_ai(&mut self, dialog: Dialog, tab: u64) {
        let prompt = dialog.memo_text("prompt").unwrap_or_default();
        let prompt = prompt.trim().to_owned();
        if prompt.is_empty() {
            let mut dialog = dialog;
            dialog.focus_on("prompt");
            self.dialogs.push(dialog);
            self.status = "Write what you need first".to_owned();
            return;
        }
        let Some((_, structure)) = self.ai.structure.as_ref().filter(|(t, _)| *t == tab) else {
            let mut dialog = dialog;
            dialog.focus_on("prompt");
            self.dialogs.push(dialog);
            self.status = "Still reading the structure…".to_owned();
            return;
        };
        let key = ai::api_key(&self.paths.ai_key_file());
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let schema = Self::ai_scope(&dialog);
        let structure = structure.text(schema.as_deref());
        let database = self.tabs[i]
            .query()
            .and_then(|q| q.binding.as_ref())
            .map(|b| b.1.clone())
            .unwrap_or_default();
        let server = self.tabs[i]
            .query()
            .and_then(|q| q.binding.as_ref())
            .and_then(|(uid, _)| self.tree.servers.iter().find(|s| s.uid == *uid))
            .and_then(|s| s.version)
            .map_or_else(
                || "PostgreSQL".to_owned(),
                |v| format!("PostgreSQL {}.{}", v.0 / 10000, v.0 % 10000),
            );
        let ask = ai::Ask {
            server,
            database,
            structure,
            editor: self.ai_editor_text(i),
            prompt: prompt.clone(),
        };
        let request = ai::request(&self.settings.ai, key.as_deref(), &ask);
        if self.ai.history.last() != Some(&prompt) {
            self.ai.history.push(prompt.clone());
        }
        self.ai.structure = None;
        self.ai.next_id += 1;
        let id = self.ai.next_id;
        let abort = match self.ai.sink.clone() {
            Some(sink) => {
                let task = tokio::spawn(async move {
                    let result = ai::send(request).await;
                    sink(AiEvent { id, result });
                });
                Some(task.abort_handle())
            }
            None => {
                self.ai.unsent = Some(request);
                None
            }
        };
        self.ai.run = Some(AiRun {
            id,
            tab,
            prompt,
            abort,
        });
        self.status = format!(
            "Asking AI ({}) … Esc cancels",
            ai::host(&self.settings.ai.base_url)
        );
    }

    /// `Esc` while a request runs: it is cancelled. True if there was one.
    pub(super) fn cancel_ai(&mut self) -> bool {
        let Some(run) = self.ai.run.take() else {
            return false;
        };
        if let Some(abort) = run.abort {
            abort.abort();
        }
        self.status = "AI request cancelled".to_owned();
        true
    }

    /// An AI answer: into the editor of its tab, replacing the selection or at the
    /// cursor — never run.
    pub fn on_ai_event(&mut self, event: AiEvent) {
        if self.ai.run.as_ref().is_none_or(|r| r.id != event.id) {
            return;
        }
        let Some(run) = self.ai.run.take() else {
            return;
        };
        match event.result {
            Ok(answer) => {
                let Some(i) = self.tab_index(run.tab) else {
                    return;
                };
                let mut text = ai::editor_insert(&run.prompt, &answer);
                let editor = &mut self.tabs[i].view.editor;
                // the prompt comment starts a line of its own
                if editor.selection().is_none() && editor.cursor_line_col().1 > 0 {
                    text.insert(0, '\n');
                }
                editor.apply(Action::Insert(text));
                self.status = "AI answer inserted — review it before you run it".to_owned();
            }
            Err(e) => {
                self.status = "The AI request failed".to_owned();
                self.error(&e);
            }
        }
    }

    // ---- autocomplete in the prompt ------------------------------------------------

    /// The open list of the prompt box, for drawing.
    pub(crate) fn ai_popup(&self) -> Option<&crate::completion::Popup> {
        self.ai.completion.as_ref().filter(|p| p.shown())
    }

    /// The prompt box has the keys of the `F8` dialog on top.
    pub(super) fn in_ai_prompt(&self) -> bool {
        self.dialogs
            .last()
            .is_some_and(|d| matches!(d.kind, DialogKind::AskAi { .. }) && d.memo_focused())
    }

    /// The name before the prompt's cursor: where it starts, its typed part, and the
    /// entries (`None`: the structure is still being read).
    fn ai_completion_list(&self) -> Option<(usize, String, Option<Vec<crate::completion::Item>>)> {
        if !self.in_ai_prompt() {
            return None;
        }
        let d = self.dialogs.last()?;
        let DialogKind::AskAi { tab } = d.kind else {
            return None;
        };
        let e = d.memo_editor("prompt")?;
        let chars: Vec<char> = e.text().chars().collect();
        let (start, qualifier, prefix) = ai::prompt_word(&chars, e.cursor());
        let items = self
            .ai
            .structure
            .as_ref()
            .filter(|(t, _)| *t == tab)
            .map(|(_, s)| ai::prompt_items(s, &qualifier, &prefix));
        Some((start, prefix, items))
    }

    /// `Ctrl+Space` / `Alt+/` (`auto` false) or a typed `.` (`auto`: shown only when
    /// something fits) in the prompt box: the names of the structure.
    pub(super) fn open_ai_completion(&mut self, auto: bool) {
        use crate::completion::Popup;
        use pgc_editor::complete::{Analysis, Want};
        let Some((start, prefix, items)) = self.ai_completion_list() else {
            return;
        };
        let loading = items.is_none();
        let items = items.unwrap_or_default();
        if items.is_empty() && !loading {
            if !auto {
                self.status = "No suggestions".to_owned();
            }
            return;
        }
        let mut popup = Popup {
            tab: self.tabs[self.active_tab].id,
            in_form: false,
            analysis: Analysis {
                prefix,
                start,
                want: Want::Any,
                tables: Vec::new(),
            },
            items: Vec::new(),
            loading: false,
            selected: 0,
            offset: 0,
            auto,
        };
        popup.set_items(items, loading);
        self.ai.completion = Some(popup);
    }

    /// After every message: the open list follows the typing and the structure that
    /// arrives; it closes when the name is left or nothing fits any more.
    pub(super) fn refresh_ai_completion(&mut self) {
        let Some(start) = self.ai.completion.as_ref().map(|p| p.analysis.start) else {
            return;
        };
        match self.ai_completion_list() {
            Some((at, prefix, items))
                if at == start && items.as_ref().is_none_or(|i| !i.is_empty()) =>
            {
                if let Some(p) = self.ai.completion.as_mut() {
                    let loading = items.is_none();
                    p.analysis.prefix = prefix;
                    p.set_items(items.unwrap_or_default(), loading);
                }
            }
            _ => self.ai.completion = None,
        }
    }

    /// Keys while the prompt's list is open; true when the key was used. Typing and
    /// `Backspace` go on into the prompt (the list follows afterwards); `Enter` and
    /// `Tab` take the entry.
    pub(super) fn ai_completion_key(
        &mut self,
        ev: &crossterm::event::KeyEvent,
        cmd: Option<crate::keymap::Command>,
    ) -> bool {
        use crate::completion::LIST_ROWS;
        use crate::keymap::Command;
        use crossterm::event::{KeyCode, KeyModifiers};
        let typing = matches!(ev.code, KeyCode::Char(_))
            && !ev
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if typing || cmd == Some(Command::Backspace) {
            return false;
        }
        let rows = LIST_ROWS as isize;
        let step = match cmd {
            Some(Command::Up) => Some(-1),
            Some(Command::Down) => Some(1),
            Some(Command::PageUp) => Some(-rows),
            Some(Command::PageDown) => Some(rows),
            _ => None,
        };
        if let Some(step) = step {
            if let Some(p) = self.ai.completion.as_mut() {
                p.move_by(step);
            }
            return true;
        }
        if matches!(cmd, Some(Command::Accept | Command::NextField)) {
            self.accept_ai_completion();
            return true;
        }
        let combo = crate::keys::KeyCombo::from_event(ev);
        if self
            .keymap
            .lookup_only(crate::keymap::Context::Editor, combo)
            == Some(Command::Complete)
        {
            return true;
        }
        self.ai.completion = None;
        false
    }

    /// Puts the selected entry in place of the typed part of the name.
    fn accept_ai_completion(&mut self) {
        let Some(popup) = self.ai.completion.take() else {
            return;
        };
        let Some(item) = popup.items.get(popup.selected) else {
            return;
        };
        let Some(e) = self
            .dialogs
            .last_mut()
            .and_then(|d| d.memo_editor_mut("prompt"))
        else {
            return;
        };
        let cursor = e.cursor();
        e.set_cursor(popup.analysis.start.min(cursor), false);
        e.set_cursor(cursor, true);
        e.apply(Action::Insert(item.insert.clone()));
    }

    // ---- Settings → AI… -------------------------------------------------------------

    /// The AI settings dialog (D-073). A saved key shows as [`SAVED_KEY`].
    pub(super) fn open_ai_settings(&mut self) {
        let a = &self.settings.ai;
        let lw = AI_LABEL_WIDTH;
        let mut presets = vec!["(custom)".to_owned()];
        presets.extend(ai::PRESETS.iter().map(|p| p.name.to_owned()));
        let preset = ai::PRESETS
            .iter()
            .position(|p| p.name == a.preset)
            .map_or(0, |i| i + 1);
        let protocols = AiProtocol::ALL
            .iter()
            .map(|p| p.label().to_owned())
            .collect();
        let protocol = AiProtocol::ALL
            .iter()
            .position(|p| *p == a.protocol)
            .unwrap_or(0);
        let saved = ai::api_key(&self.paths.ai_key_file()).is_some();
        let dialog = Dialog::new(
            DialogKind::AiSettings,
            "AI assistant",
            66,
            vec![
                Widget::checkbox("enabled", "&Enabled", a.enabled),
                Widget::label(""),
                Widget::combo("preset", "&Preset:", presets, preset, lw),
                Widget::combo("protocol", "P&rotocol:", protocols, protocol, lw),
                Widget::field("url", "&Base URL:", &a.base_url, lw),
                Widget::field("model", "&Model:", &a.model, lw),
                Widget::field("key", "API &key:", if saved { SAVED_KEY } else { "" }, lw).masked(),
                Widget::label(""),
                Widget::label("The key is kept in its own file in pgtc's config folder,"),
                Widget::label("not in settings.toml. Empty: no key (local servers)."),
                Widget::button(Button::Ok, "&OK", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        );
        self.ai.preset_seen = preset;
        self.dialogs.push(dialog);
    }

    /// After each update: a newly chosen preset fills in the fields; typing over the
    /// saved key's stars starts a new key.
    pub(super) fn sync_ai_settings(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if d.kind != DialogKind::AiSettings {
            return;
        }
        let key = d.text("key").unwrap_or("").to_owned();
        if key != SAVED_KEY {
            if let Some(typed) = key.strip_prefix(SAVED_KEY) {
                d.set_text("key", typed);
            } else if !key.is_empty() && SAVED_KEY.starts_with(&key) {
                // Backspace on the stars: the whole saved key goes
                d.set_text("key", "");
            }
        }
        let chosen = d.selected("preset").unwrap_or(0);
        if chosen == self.ai.preset_seen {
            return;
        }
        self.ai.preset_seen = chosen;
        let Some(p) = chosen.checked_sub(1).and_then(|i| ai::PRESETS.get(i)) else {
            return;
        };
        let protocol = AiProtocol::ALL
            .iter()
            .position(|x| *x == p.protocol)
            .unwrap_or(0);
        d.set_combo_selected("protocol", protocol);
        d.set_text("url", p.base_url);
        d.set_text("model", p.model);
    }

    /// OK in the AI settings dialog: the settings are saved, the key goes into the key
    /// file (an empty field removes it; the stars keep it).
    pub(super) fn apply_ai_settings(&mut self, dialog: Dialog) {
        let text = |id: &str| dialog.text(id).unwrap_or("").trim().to_owned();
        let enabled = dialog.checked("enabled");
        let (url, model, key) = (text("url"), text("model"), text("key"));
        if enabled && (url.is_empty() || model.is_empty()) {
            let mut dialog = dialog;
            dialog.focus_on(if url.is_empty() { "url" } else { "model" });
            self.dialogs.push(dialog);
            self.error("Base URL and model are needed when AI is enabled.");
            return;
        }
        let file = self.paths.ai_key_file();
        let written = if key == SAVED_KEY {
            Ok(())
        } else if key.is_empty() {
            match std::fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            file.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| atomic_write(&file, key.as_bytes(), Access::Private))
        };
        if let Err(e) = written {
            self.dialogs.push(dialog);
            self.error(&format!("Could not write {}: {e}", file.display()));
            return;
        }
        let preset = dialog
            .selected("preset")
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| ai::PRESETS.get(i))
            .map_or(String::new(), |p| p.name.to_owned());
        let a = &mut self.settings.ai;
        a.enabled = enabled;
        a.preset = preset;
        a.protocol = dialog
            .selected("protocol")
            .and_then(|i| AiProtocol::ALL.get(i).copied())
            .unwrap_or_default();
        a.base_url = url;
        a.model = model;
        self.save_settings();
        self.status = "AI settings saved".to_owned();
    }
}

/// `12345` → `12,345`.
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
