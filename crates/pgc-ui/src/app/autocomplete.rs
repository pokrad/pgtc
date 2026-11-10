//! Autocomplete in query tabs (D-052): opening, filtering and accepting the list,
//! reading names through the tab's connection.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgc_editor::Action;
use pgc_sql::RenderContext;

use super::App;
use super::lookups::LookupTarget;
use crate::keymap::Command;
use crate::query_tab::QueryState;

/// Status line: autocomplete could not read names it misses (D-052).
pub(super) const NAMES_HELD: &str =
    "Autocomplete: new names are read after the running or paused result ends";

impl App {
    /// Keys while the list is open; true when the key was used. Typed characters go
    /// on into the editor (and refilter the list afterwards).
    pub(super) fn completion_key(&mut self, ev: &KeyEvent, cmd: Option<Command>) -> bool {
        use crate::completion::LIST_ROWS;
        // a list still waiting for names is not shown: keys work as usual
        if self.completion.as_ref().is_some_and(|p| !p.shown()) {
            return false;
        }
        let typing = matches!(ev.code, KeyCode::Char(_))
            && !ev
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if typing {
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
            if let Some(p) = self.completion.as_mut() {
                p.move_by(step);
            }
            return true;
        }
        match cmd {
            // Enter / Tab insert while the list is open
            Some(Command::Newline | Command::Indent | Command::Accept) => {
                self.accept_completion();
                true
            }
            Some(Command::Backspace) => {
                self.edit(Action::Backspace);
                self.refresh_completion();
                true
            }
            Some(Command::Complete) => true,
            _ => {
                self.completion = None;
                false
            }
        }
    }

    /// After a typed character: refilter the open list, or open it after `.` and after
    /// the 3rd letter of a name (D-052).
    pub(super) fn after_typing(&mut self, c: char) {
        if self.completion.is_some() {
            self.refresh_completion();
        } else if c == '.' || c == '_' || c.is_alphanumeric() {
            self.open_completion(false);
        }
    }

    /// `Ctrl+Space` / `Alt+/` (`manual`) or automatic: opens the list at the cursor.
    pub(super) fn open_completion(&mut self, manual: bool) {
        use crate::completion::{AUTO_LETTERS, Popup};
        use pgc_editor::complete::Want;
        let tab = self.tabs[self.active_tab].id;
        let Some(q) = self.query() else {
            return;
        };
        if !q.connected() {
            if manual {
                self.status = "Autocomplete needs a connected query tab".to_owned();
            }
            return;
        }
        let Some(a) = pgc_editor::complete::analyze(&self.tabs[self.active_tab].view.editor) else {
            if manual {
                self.status = "No suggestions".to_owned();
            }
            return;
        };
        if !manual {
            let qualified_start = matches!(a.want, Want::Qualified(_)) && a.prefix.is_empty();
            if !qualified_start && a.prefix.chars().count() != AUTO_LETTERS {
                return;
            }
        }
        let (built, held) = completion_list(&a, q);
        let needs = built.needs.clone();
        if manual && held {
            self.status = NAMES_HELD.to_owned();
        }
        if built.items.is_empty() && !built.loading {
            if manual && !held {
                self.status = "No suggestions".to_owned();
            }
            self.request_names(&needs);
            return;
        }
        let mut popup = Popup {
            tab,
            analysis: a,
            items: Vec::new(),
            loading: false,
            selected: 0,
            offset: 0,
            auto: !manual,
        };
        popup.set_items(built.items, built.loading);
        self.completion = Some(popup);
        self.request_names(&needs);
    }

    /// The text changed or names arrived: new entries for the open list; it closes
    /// when the cursor left the name or nothing fits any more.
    fn refresh_completion(&mut self) {
        use pgc_editor::complete::Want;
        let Some(popup) = self.completion.as_ref() else {
            return;
        };
        let Some(i) = self.tab_index(popup.tab) else {
            self.completion = None;
            return;
        };
        let tab = &self.tabs[i];
        let Some(q) = tab.query.as_ref() else {
            self.completion = None;
            return;
        };
        let a = pgc_editor::complete::analyze(&tab.view.editor);
        let Some(a) = a.filter(|a| {
            !a.prefix.is_empty() || matches!(a.want, Want::Qualified(_)) || !popup.auto
        }) else {
            self.completion = None;
            return;
        };
        // a different word than the one the list was opened for
        if a.start != popup.analysis.start && !a.prefix.is_empty() {
            self.completion = None;
            return;
        }
        let (built, _) = completion_list(&a, q);
        let needs = built.needs.clone();
        if built.items.is_empty() && !built.loading {
            self.completion = None;
            return;
        }
        if let Some(p) = self.completion.as_mut() {
            p.analysis = a;
            p.set_items(built.items, built.loading);
        }
        self.request_names(&needs);
    }

    /// Inserts the selected entry in place of the typed prefix.
    fn accept_completion(&mut self) {
        let Some(popup) = self.completion.take() else {
            return;
        };
        let Some(item) = popup.items.get(popup.selected).cloned() else {
            return;
        };
        let editor = self.editor();
        let Some(a) = pgc_editor::complete::analyze(editor) else {
            return;
        };
        let cursor = editor.cursor();
        editor.set_cursor(a.start, false);
        editor.set_cursor(cursor, true);
        editor.apply(Action::Insert(item.insert));
    }

    /// Asks the active query tab's connection for missing names.
    fn request_names(&mut self, needs: &[crate::completion::Need]) {
        use crate::completion::Need;
        if needs.is_empty() {
            return;
        }
        let tab = self.tabs[self.active_tab].id;
        let Some(version) = self.tab_server_version() else {
            return;
        };
        let render = RenderContext {
            version,
            show_system: true,
        };
        let objects_sql = self.sql.render("completion", "objects.sql", render);
        let columns_sql = self.sql.render("completion", "columns.sql", render);
        let Some(q) = self.tabs[self.active_tab].query.as_mut() else {
            return;
        };
        let Some(handle) = q.conn.as_ref().map(|c| c.handle.clone()) else {
            return;
        };
        // a request now would end the running or paused result (see `completion_list`)
        if q.running.is_some() {
            return;
        }
        let generation = q.names.generation;
        for need in needs {
            let (sql, params) = match need {
                Need::Objects if !q.names.objects_pending => {
                    q.names.objects_pending = true;
                    (&objects_sql, Vec::new())
                }
                Need::Columns(key) if !q.names.columns_pending.contains(key) => {
                    q.names.columns_pending.insert(key.clone());
                    (&columns_sql, vec![key.0.clone(), Some(key.1.clone())])
                }
                _ => continue,
            };
            let sql = match sql {
                Ok(sql) => sql.clone(),
                Err(e) => {
                    tracing::error!("autocomplete SQL: {e}");
                    continue;
                }
            };
            self.next_key += 1;
            self.template_lookups.insert(
                self.next_key,
                LookupTarget::Names {
                    tab,
                    generation,
                    need: need.clone(),
                },
            );
            handle.lookup_with(self.next_key, sql, params);
        }
    }

    /// Answer to [`Self::request_names`].
    pub(super) fn names_loaded(
        &mut self,
        tab: u64,
        generation: u64,
        need: crate::completion::Need,
        result: Result<Vec<Vec<String>>, String>,
    ) {
        use crate::completion::Need;
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(q) = self.tabs[i].query.as_mut() else {
            return;
        };
        if q.names.generation != generation {
            return;
        }
        if let Err(e) = &result {
            tracing::warn!("autocomplete names: {e}");
        }
        let rows = result.unwrap_or_default();
        match need {
            Need::Objects => q.names.set_objects(rows),
            Need::Columns(key) => {
                q.names.columns_pending.remove(&key);
                let cols = rows
                    .into_iter()
                    .filter_map(|r| r.into_iter().next())
                    .collect();
                q.names.columns.insert(key, cols);
            }
        }
        if self.completion.as_ref().is_some_and(|p| p.tab == tab) {
            self.refresh_completion();
        }
    }
}

/// The autocomplete entries for `a` in a query tab. While the tab's connection is held
/// by a running or paused result, another request on it would end that result (D-019):
/// names that are not read yet are then not asked for, and the list offers what is
/// known. Returns the list and whether names were held back.
fn completion_list(
    a: &pgc_editor::complete::Analysis,
    q: &QueryState,
) -> (crate::completion::Built, bool) {
    let mut built = crate::completion::build(a, &q.names, pgc_editor::lexer::keywords());
    let held = q.running.is_some() && !built.needs.is_empty();
    if held {
        built.needs.clear();
        built.loading = false;
    }
    (built, held)
}
