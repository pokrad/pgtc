//! AI assistant (D-073): the `F8` dialog, the structure read through the query tab's
//! connection, the request in the background, the answer in the editor (never run),
//! and Settings → AI….

use std::sync::Arc;

use pgc_core::fsutil::{Access, atomic_write};
use pgc_core::settings::{AiEffort, AiModel, AiProtocol, AiSettings};
use pgc_editor::Action;
use pgc_sql::RenderContext;

use super::lookups::LookupTarget;
use super::state::AiRun;
use super::{App, CONNECTION_BUSY};
use crate::ai::{self, Structure};
use crate::dialog::{Button, Dialog, DialogKind, Widget};

/// Where the answers of AI requests go (set by the event loop).
pub type AiSink = Arc<dyn Fn(AiMsg) + Send + Sync>;

/// What comes back from the background.
pub enum AiMsg {
    Answer(AiEvent),
    Models(AiModels),
}

/// The provider's list of models for base URL `url`, read for the settings dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiModels {
    pub url: String,
    pub result: Result<Vec<AiModel>, String>,
}

/// The answer of AI request `id`: the text, or an error for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiEvent {
    pub id: u64,
    pub result: Result<String, String>,
}

const NOT_SET_UP: &str = "AI is not set up (Settings → AI…)";
const SCOPE_ALL: &str = "Whole database";
const PROMPT_ROWS: u16 = 6;
/// Above this many tokens the dialog warns (models and budgets have limits).
const LARGE_TOKENS: usize = 100_000;
const LARGE_NOTE: &str = "Large request: it may be refused or cost a lot. Try one schema as scope.";
/// What the key field shows for a saved key (masked: eight stars).
const SAVED_KEY: &str = "********";
/// The last entry of the settings' model combo: a name typed by hand.
const MODEL_OTHER: &str = "Other…";
/// The model combo before a list could be read.
const MODEL_HINT: &str = "(fill in the base URL and the API key)";
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
        // reading the structure would end a paused result (or the stream an export
        // reads from): refused, as for the generate commands (D-019, D-054)
        if q.running.is_some() || q.export.is_some() {
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
        self.ai.size = None;
        self.ai.history_pos = None;
        // model and effort: the settings' values, changeable for this request; the
        // models are those the settings dialog read and saved (no network here)
        let a = &self.settings.ai;
        let offered = Self::ai_offered_models(a);
        let model = offered.iter().position(|m| m.id == a.model).unwrap_or(0);
        let models = offered.iter().map(|m| m.label().to_owned()).collect();
        let efforts = AiEffort::ALL.iter().map(|e| e.label().to_owned()).collect();
        let effort = AiEffort::ALL
            .iter()
            .position(|e| *e == a.effort)
            .unwrap_or(0);
        let dialog = Dialog::new(
            DialogKind::AskAi { tab },
            "Ask AI",
            76,
            vec![
                Widget::memo("prompt", "&Prompt", &self.ai.draft, PROMPT_ROWS),
                Widget::combo("scope", "Sc&ope:", vec![SCOPE_ALL.to_owned()], 0, 8),
                Widget::combo("model", "&Model:", models, model, 8),
                Widget::combo("effort", "&Effort:", efforts, effort, 8),
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
                self.ai.structure = Some((tab, Ok(s)));
            }
            Err(e) => {
                d.set_label("info", "The structure could not be read (Send shows why).");
                self.ai.structure = Some((tab, Err(e)));
            }
        }
        self.ai.size = None;
    }

    /// The structure read for the `F8` dialog of tab `tab`: `None` while it is being
    /// read, the error when reading failed.
    fn ai_structure(&self, tab: u64) -> Option<&Result<Structure, String>> {
        self.ai
            .structure
            .as_ref()
            .filter(|(t, _)| *t == tab)
            .map(|(_, s)| s)
    }

    /// Everything a request from dialog `d` of tab `tab` sends, with `prompt`.
    fn ai_ask(&self, d: &Dialog, tab: u64, s: &Structure, prompt: &str) -> Option<ai::Ask> {
        let i = self.tab_index(tab)?;
        // the scope: entry 0 is the whole database, then the schemas in order
        let schemas = s.schemas();
        let schema = d
            .selected("scope")
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| schemas.get(n));
        let (uid, database) = self.tabs[i].query()?.binding.clone()?;
        let server = self
            .tree
            .servers
            .iter()
            .find(|sv| sv.uid == uid)
            .and_then(|sv| sv.version)
            .map_or_else(|| "PostgreSQL".to_owned(), |v| format!("PostgreSQL {v}"));
        let e = &self.tabs[i].view.editor;
        Some(ai::Ask {
            server,
            database,
            structure: s.text(schema.map(String::as_str)),
            editor: ai::editor_text(&e.text(), e.cursor(), e.selection(), ai::EDITOR_LIMIT),
            prompt: prompt.to_owned(),
        })
    }

    /// After each update: the info line follows the chosen scope and the prompt.
    pub(super) fn sync_ai_dialog(&mut self) {
        let Some(d) = self.dialogs.last() else {
            return;
        };
        let DialogKind::AskAi { tab } = d.kind else {
            return;
        };
        let Some(Ok(s)) = self.ai_structure(tab) else {
            return;
        };
        let scope = d.selected("scope").unwrap_or(0);
        // the size without the prompt: once for a scope (the structure may be large)
        if self.ai.size.is_none_or(|(sc, _, _)| sc != scope) {
            let Some(ask) = self.ai_ask(d, tab, s, "") else {
                return;
            };
            let note = match &ask.editor {
                None => "The editor is empty: only the structure and the prompt are sent.",
                Some(e) if e.cut => {
                    "The editor text is long: only the part around the cursor is sent."
                }
                Some(_) => "",
            };
            self.ai.size = Some((scope, ai::request_chars(&ask), note));
        }
        let Some((_, chars, note)) = self.ai.size else {
            return;
        };
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        let prompt = d.memo_text("prompt").map_or(0, |p| p.chars().count());
        let tokens = ai::tokens(chars + prompt);
        let info = format!(
            "Sends about {} tokens to {} (no table data).",
            group(tokens),
            ai::host(&self.settings.ai.base_url)
        );
        d.set_label("info", &info);
        d.set_label(
            "note",
            if tokens > LARGE_TOKENS {
                LARGE_NOTE
            } else {
                note
            },
        );
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
        let ask = match self.ai_structure(tab) {
            None => {
                let mut dialog = dialog;
                dialog.focus_on("prompt");
                self.dialogs.push(dialog);
                self.status = "Still reading the structure…".to_owned();
                return;
            }
            Some(Err(e)) => {
                let e = format!("The structure of the database could not be read:\n{e}");
                let mut dialog = dialog;
                dialog.focus_on("prompt");
                self.dialogs.push(dialog);
                self.error(&e);
                return;
            }
            Some(Ok(s)) => self.ai_ask(&dialog, tab, s, &prompt),
        };
        let Some(ask) = ask else {
            return;
        };
        let key = ai::api_key(&self.paths.ai_key_file());
        // the model and the effort chosen for this request
        let mut chosen = self.settings.ai.clone();
        if let Some(m) = dialog
            .selected("model")
            .and_then(|i| Self::ai_offered_models(&chosen).into_iter().nth(i))
        {
            chosen.model = m.id;
        }
        if let Some(e) = dialog.selected("effort").and_then(|i| AiEffort::ALL.get(i)) {
            chosen.effort = *e;
        }
        let request = ai::request(&chosen, key.as_deref(), &ask);
        if self.ai.history.last() != Some(&prompt) {
            self.ai.history.push(prompt.clone());
        }
        self.ai.structure = None;
        self.ai.draft.clear();
        self.ai.next_id += 1;
        let id = self.ai.next_id;
        let abort = match self.ai.sink.clone() {
            Some(sink) => {
                let task = tokio::spawn(async move {
                    let result = ai::send(request).await;
                    sink(AiMsg::Answer(AiEvent { id, result }));
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
                    self.status = "The AI answer was dropped: its query tab is closed".to_owned();
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
        // (a structure that could not be read offers nothing)
        let items = self.ai_structure(tab).map(|s| match s {
            Ok(s) => ai::prompt_items(s, &qualifier, &prefix),
            Err(_) => Vec::new(),
        });
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
                    // typed into: the list is used, Enter takes its entry
                    if !prefix.is_empty() {
                        p.auto = false;
                    }
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
                p.auto = false;
            }
            return true;
        }
        // Enter in a list that opened by itself after a `.` and was not used yet is
        // a new line (the sentence may just have ended with a name); Tab takes it
        let unused = self.ai.completion.as_ref().is_some_and(|p| p.auto);
        if cmd == Some(Command::NextField) || (cmd == Some(Command::Accept) && !unused) {
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

    /// The `F8` dialog `d` is closed without sending: its prompt is kept for the next
    /// one.
    pub(super) fn keep_ai_draft(&mut self, d: &Dialog) {
        self.ai.draft = d.memo_text("prompt").unwrap_or_default();
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

    // ---- the provider's models ------------------------------------------------------

    /// The models the `F8` dialog offers: those the settings dialog read and saved,
    /// with the settings' model in any case.
    fn ai_offered_models(a: &AiSettings) -> Vec<AiModel> {
        let mut out = a.models.clone();
        if !a.model.is_empty() && !out.iter().any(|m| m.id == a.model) {
            out.insert(
                0,
                AiModel {
                    id: a.model.clone(),
                    name: String::new(),
                },
            );
        }
        out
    }

    /// The entries of the settings dialog's model combo — a model typed by hand first,
    /// the provider's models, [`MODEL_OTHER`] last — each with its model id (none for
    /// the hint and for *Other…*), and the entry of the chosen model.
    fn ai_model_entries(&self) -> (Vec<(String, Option<String>)>, usize) {
        let (list, chosen) = (&self.ai.form_models, &self.ai.form_model);
        let mut out: Vec<(String, Option<String>)> = Vec::new();
        if !chosen.is_empty() && !list.iter().any(|m| m.id == *chosen) {
            out.push((chosen.clone(), Some(chosen.clone())));
        }
        out.extend(
            list.iter()
                .map(|m| (m.label().to_owned(), Some(m.id.clone()))),
        );
        if out.is_empty() {
            out.push((MODEL_HINT.to_owned(), None));
        }
        let selected = out
            .iter()
            .position(|(_, id)| id.as_ref() == Some(chosen))
            .unwrap_or(0);
        out.push((MODEL_OTHER.to_owned(), None));
        (out, selected)
    }

    /// Fills the model combo of the open settings dialog from the models read and
    /// the chosen one.
    pub(super) fn ai_rebuild_models(&mut self) {
        let (entries, selected) = self.ai_model_entries();
        if let Some(d) = self
            .dialogs
            .iter_mut()
            .rev()
            .find(|d| d.kind == DialogKind::AiSettings)
        {
            d.set_combo(
                "model",
                entries.into_iter().map(|(label, _)| label).collect(),
                selected,
            );
        }
        self.ai.model_seen = selected;
    }

    /// Reads the model list of `a`'s provider in the background ([`AiModels`] follows).
    fn fetch_ai_models(&mut self, a: &AiSettings, key: Option<String>) {
        let request = ai::models_request(a, key.as_deref());
        let url = a.base_url.clone();
        match self.ai.sink.clone() {
            Some(sink) => {
                tokio::spawn(async move {
                    let result = ai::fetch_models(request).await;
                    sink(AiMsg::Models(AiModels { url, result }));
                });
            }
            None => self.ai.unsent_models = Some(request),
        }
    }

    /// The provider's models have arrived (or could not be read) for the open
    /// settings dialog: its model combo offers them.
    pub fn on_ai_models(&mut self, m: AiModels) {
        // an answer for a URL that is not in the dialog any more is not wanted
        let Some(d) = self
            .dialogs
            .iter_mut()
            .rev()
            .find(|d| d.kind == DialogKind::AiSettings)
            .filter(|d| d.text("url").map(str::trim) == Some(m.url.as_str()))
        else {
            return;
        };
        match m.result {
            Ok(list) => {
                d.set_label(
                    "minfo",
                    &format!("{} models read from {}.", list.len(), ai::host(&m.url)),
                );
                // nothing chosen yet: the newest
                if self.ai.form_model.is_empty()
                    && let Some(first) = list.first()
                {
                    self.ai.form_model = first.id.clone();
                }
                self.ai.form_models = list;
            }
            Err(e) => {
                let text: String = format!("Models not read: {e}").chars().take(58).collect();
                d.set_label("minfo", &text);
                self.status = format!("AI: the list of models could not be read ({e})");
            }
        }
        self.ai_rebuild_models();
    }

    /// OK in the *Other…* box: the typed name is the model.
    pub(super) fn ai_model_typed(&mut self, d: &Dialog) {
        let name = d.text("name").unwrap_or("").trim();
        if !name.is_empty() {
            self.ai.form_model = name.to_owned();
        }
        self.ai_rebuild_models();
    }

    // ---- Settings → AI… -------------------------------------------------------------

    /// The AI settings dialog (D-073). A saved key shows as [`SAVED_KEY`]; the models
    /// are those read last, and are read again at once.
    pub(super) fn open_ai_settings(&mut self) {
        let a = &self.settings.ai;
        let lw = AI_LABEL_WIDTH;
        let mut presets = vec!["(custom)".to_owned()];
        presets.extend(ai::PRESETS.iter().map(|p| p.name.to_owned()));
        // the preset is not stored: it is the one the saved values belong to
        let preset = ai::preset_of(a).map_or(0, |i| i + 1);
        let protocols = AiProtocol::ALL
            .iter()
            .map(|p| p.label().to_owned())
            .collect();
        let protocol = AiProtocol::ALL
            .iter()
            .position(|p| *p == a.protocol)
            .unwrap_or(0);
        let saved = ai::api_key(&self.paths.ai_key_file()).is_some();
        let efforts = AiEffort::ALL.iter().map(|e| e.label().to_owned()).collect();
        let effort = AiEffort::ALL
            .iter()
            .position(|e| *e == a.effort)
            .unwrap_or(0);
        self.ai.form_models = a.models.clone();
        self.ai.form_model = a.model.clone();
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
                Widget::field("key", "API &key:", if saved { SAVED_KEY } else { "" }, lw).masked(),
                Widget::label(""),
                Widget::combo("model", "&Model:", Vec::new(), 0, lw),
                Widget::label_id("minfo", ""),
                Widget::combo("effort", "E&ffort:", efforts, effort, lw),
                Widget::label(""),
                Widget::label("The key is kept in its own file in pgtc's config folder,"),
                Widget::label("not in settings.toml. Empty: no key (local servers)."),
                Widget::button(Button::Ok, "&OK", true),
                Widget::button(Button::Cancel, "&Cancel", false),
            ],
        );
        self.ai.preset_seen = preset;
        self.ai.key_stars = saved;
        self.ai.fetch_seen = None;
        self.dialogs.push(dialog);
        self.ai_rebuild_models();
    }

    /// After each update of the settings dialog: a newly chosen preset fills in the
    /// fields; typing over the saved key's stars starts a new key; the chosen model
    /// is noted (*Other…* asks for a name); and the provider's models are read when
    /// the provider or the key has changed (after the field is left).
    pub(super) fn sync_ai_settings(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if d.kind != DialogKind::AiSettings {
            return;
        }
        // the first change of the saved key's stars — typing or pasting anywhere in
        // them, Backspace, Delete — takes all the stars away: what is left is new
        if self.ai.key_stars && d.text("key") != Some(SAVED_KEY) {
            let typed = d.text("key").unwrap_or("").replace('*', "");
            d.set_text("key", &typed);
            self.ai.key_stars = false;
        }
        // a preset: its protocol, URL and suggested model; the models of the provider
        // before are not its models
        let preset = d.selected("preset").unwrap_or(0);
        if preset != self.ai.preset_seen {
            self.ai.preset_seen = preset;
            if let Some(p) = preset.checked_sub(1).and_then(|i| ai::PRESETS.get(i)) {
                let protocol = AiProtocol::ALL
                    .iter()
                    .position(|x| *x == p.protocol)
                    .unwrap_or(0);
                d.set_combo_selected("protocol", protocol);
                d.set_text("url", p.base_url);
                self.ai.form_model = p.model.to_owned();
                self.ai.form_models.clear();
                self.ai_rebuild_models();
            }
        }
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        // the model combo: a model is chosen; Other… asks for a name
        let selected = d.selected("model").unwrap_or(0);
        if selected != self.ai.model_seen && !d.combo_open() {
            self.ai.model_seen = selected;
            let (entries, _) = self.ai_model_entries();
            match entries.get(selected) {
                Some((_, Some(id))) => self.ai.form_model = id.clone(),
                Some((label, None)) if label == MODEL_OTHER => {
                    let current = self.ai.form_model.clone();
                    self.dialogs.push(Dialog::new(
                        DialogKind::AiModelOther,
                        "Model",
                        60,
                        vec![
                            Widget::label("The model name as the provider's API takes it:"),
                            Widget::field("name", "", &current, 0),
                            Widget::button(Button::Ok, "&OK", true),
                            Widget::button(Button::Cancel, "&Cancel", false),
                        ],
                    ));
                    return;
                }
                _ => {}
            }
        }
        // the models: read when the dialog opens and when protocol, URL or key have
        // changed — not while the URL or the key is being typed
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        let text = |d: &Dialog, id: &str| d.text(id).unwrap_or("").trim().to_owned();
        let protocol = d.selected("protocol").unwrap_or(0);
        let (url, typed) = (text(d, "url"), text(d, "key"));
        let asked = Some((protocol, url.clone(), typed.clone()));
        if asked == self.ai.fetch_seen || matches!(d.focused_id(), Some("url" | "key")) {
            return;
        }
        self.ai.fetch_seen = asked;
        if url.is_empty() {
            d.set_label("minfo", "");
            return;
        }
        d.set_label("minfo", "Reading the models…");
        let a = AiSettings {
            protocol: AiProtocol::ALL.get(protocol).copied().unwrap_or_default(),
            base_url: url,
            ..AiSettings::default()
        };
        let key = if self.ai.key_stars && typed == SAVED_KEY {
            ai::api_key(&self.paths.ai_key_file())
        } else {
            Some(typed).filter(|k| !k.is_empty())
        };
        self.fetch_ai_models(&a, key);
    }

    /// OK in the AI settings dialog: the settings are saved — with the models that
    /// were read, for the `F8` dialog —, the key goes into the key file (an empty
    /// field removes it; the stars keep it).
    pub(super) fn apply_ai_settings(&mut self, dialog: Dialog) {
        let text = |id: &str| dialog.text(id).unwrap_or("").trim().to_owned();
        let enabled = dialog.checked("enabled");
        let (url, key) = (text("url"), text("key"));
        let model = self.ai.form_model.clone();
        if enabled && (url.is_empty() || model.is_empty()) {
            let mut dialog = dialog;
            dialog.focus_on(if url.is_empty() { "url" } else { "model" });
            self.dialogs.push(dialog);
            self.error("A base URL and a model are needed when AI is enabled.");
            return;
        }
        let file = self.paths.ai_key_file();
        let written = if self.ai.key_stars && key == SAVED_KEY {
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
        let models = std::mem::take(&mut self.ai.form_models);
        let a = &mut self.settings.ai;
        a.enabled = enabled;
        a.protocol = dialog
            .selected("protocol")
            .and_then(|i| AiProtocol::ALL.get(i).copied())
            .unwrap_or_default();
        a.base_url = url;
        a.model = model;
        a.models = models;
        a.effort = dialog
            .selected("effort")
            .and_then(|i| AiEffort::ALL.get(i).copied())
            .unwrap_or_default();
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
