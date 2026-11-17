//! psql-style commands on `F4` (D-054): the command window, running a command,
//! `\conninfo` and the answers of the generate commands.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgc_editor::Action;
use pgc_sql::RenderContext;

use super::lookups::LookupTarget;
use super::{App, CONNECTION_BUSY};
use crate::dialog::{Button, Dialog, DialogKind, Widget};
use crate::keymap::Command;
use crate::query_tab::Sent;

impl App {
    /// F4: the window with every command (D-054).
    pub(super) fn open_psql_dialog(&mut self, text: &str) {
        if !self.is_enabled(Command::Psql) {
            self.status = "psql commands need a connected, idle query tab".to_owned();
            return;
        }
        let d = self.psql_dialog(text);
        self.dialogs.push(d);
    }

    /// The F4 window: input, then the list of all commands.
    pub(super) fn psql_dialog(&mut self, text: &str) -> Dialog {
        let rows = crate::psql::list_rows();
        let line = self.theme.hline();
        let items: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Some((c, args, desc)) => format!("{c:<11}{args:<10}{desc}"),
                None => format!("{} pgtc {}", line.repeat(20), line.repeat(44)),
            })
            .collect();
        let marks = rows.iter().map(Option::is_none).collect();
        let mut d = Dialog::new(
            DialogKind::PsqlCommand,
            "psql command",
            80,
            vec![
                Widget::field("cmd", "&Command:", text, 10),
                Widget::label(""),
                Widget::list("list", "", items, 0, rows.len() as u16).highlighted(marks),
                Widget::button(Button::Ok, "&Run", true),
                Widget::button(Button::Cancel, "Ca&ncel", false),
            ],
        );
        d.fit_text(self.frame.screen);
        self.psql_seen = (text.to_owned(), 0);
        d
    }

    /// Choosing a row (keys, mouse) puts its command into the input; typing a command
    /// moves the list to it. The list itself always shows every command.
    pub(super) fn sync_psql_dialog(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if d.kind != DialogKind::PsqlCommand {
            return;
        }
        let rows = crate::psql::list_rows();
        let text = d.text("cmd").unwrap_or("").to_owned();
        let mut sel = d.selected("list").unwrap_or(0);
        let (seen_text, seen_sel) = self.psql_seen.clone();
        if sel != seen_sel {
            // the separator is not a command: go on in the same direction
            if rows.get(sel).is_some_and(Option::is_none) {
                sel = if sel > seen_sel || sel == 0 {
                    sel + 1
                } else {
                    sel - 1
                };
                d.list_select("list", sel);
            }
            if let Some(Some((c, args, _))) = rows.get(sel) {
                let text = if args.is_empty() {
                    (*c).to_owned()
                } else {
                    format!("{c} ")
                };
                d.set_text("cmd", &text);
                self.psql_seen = (text, sel);
            }
        } else if text != seen_text {
            if let Some(first) = crate::psql::matching(&text).first()
                && let Some(row) = rows
                    .iter()
                    .position(|r| r.is_some_and(|(c, _, _)| c == first.0))
            {
                d.list_select("list", row);
                sel = row;
            }
            self.psql_seen = (text, sel);
        }
    }

    /// The argument of the F4 input and its autocomplete list with the names known so
    /// far (D-054); names still missing are not asked for while a result runs or is
    /// paused (D-052). `None`: the command offers nothing.
    fn psql_completion_list(&self) -> Option<(String, crate::completion::Built, bool)> {
        let d = self.dialogs.last()?;
        if d.kind != DialogKind::PsqlCommand {
            return None;
        }
        let (arg, kinds) = crate::psql::completion_target(d.text("cmd")?)?;
        let q = self.query()?;
        let mut built = crate::completion::build_command(&q.names, &kinds, arg);
        let held = q.running.is_some() && !built.needs.is_empty();
        if held {
            built.needs.clear();
            built.loading = false;
        }
        Some((arg.to_owned(), built, held))
    }

    /// `Ctrl+Space` / `Alt+/` in the F4 input: the list for the argument (D-054).
    pub(super) fn open_psql_completion(&mut self) {
        use crate::completion::Popup;
        use pgc_editor::complete::{Analysis, Want};
        let Some((arg, built, held)) = self.psql_completion_list() else {
            self.status = "No suggestions".to_owned();
            return;
        };
        let needs = built.needs.clone();
        if held {
            self.status = super::autocomplete::NAMES_HELD.to_owned();
        } else if built.items.is_empty() && !built.loading {
            self.status = "No suggestions".to_owned();
        }
        if !built.items.is_empty() || built.loading {
            let mut popup = Popup {
                tab: self.tabs[self.active_tab].id,
                in_form: false,
                analysis: Analysis {
                    prefix: arg,
                    start: 0,
                    want: Want::Any,
                    tables: Vec::new(),
                },
                items: Vec::new(),
                loading: false,
                selected: 0,
                offset: 0,
                auto: false,
            };
            popup.set_items(built.items, built.loading);
            self.psql_completion = Some(popup);
        }
        self.request_names(self.active_tab, &needs);
    }

    /// After every message: the open F4 list follows the input and arriving names; it
    /// closes when the F4 window is gone or nothing fits any more.
    pub(super) fn refresh_psql_completion(&mut self) {
        if self.psql_completion.is_none() {
            return;
        }
        let tab = self.tabs[self.active_tab].id;
        match self.psql_completion_list() {
            Some((arg, built, _))
                if (!built.items.is_empty() || built.loading)
                    && self.psql_completion.as_ref().is_some_and(|p| p.tab == tab) =>
            {
                if let Some(p) = self.psql_completion.as_mut() {
                    p.analysis.prefix = arg;
                    p.set_items(built.items, built.loading);
                }
            }
            _ => self.psql_completion = None,
        }
    }

    /// Keys while the F4 list is open; true when the key was used. Typing and
    /// `Backspace` go on into the input (the list follows it afterwards).
    pub(super) fn psql_completion_key(&mut self, ev: &KeyEvent, cmd: Option<Command>) -> bool {
        use crate::completion::LIST_ROWS;
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
            if let Some(p) = self.psql_completion.as_mut() {
                p.move_by(step);
            }
            return true;
        }
        if cmd == Some(Command::Accept) {
            self.accept_psql_completion();
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
        self.psql_completion = None;
        false
    }

    /// Puts the selected entry in place of the argument.
    fn accept_psql_completion(&mut self) {
        let Some(popup) = self.psql_completion.take() else {
            return;
        };
        let Some(item) = popup.items.get(popup.selected) else {
            return;
        };
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        let text = d.text("cmd").unwrap_or("");
        let Some((arg, _)) = crate::psql::completion_target(text) else {
            return;
        };
        let keep = text.len() - arg.len();
        let new = format!("{}{}", &text[..keep], item.insert);
        d.set_text("cmd", &new);
    }

    /// Runs a command of the command line; a wrong one keeps the dialog open.
    pub(super) fn run_psql(&mut self, mut dialog: Dialog, text: &str) {
        use crate::psql::{Cmd, parse, queries};
        if crate::psql::needs_table(text) {
            dialog.focus_on("cmd");
            self.dialogs.push(dialog);
            self.status = "Type the table name after the command".to_owned();
            return;
        }
        let selected = crate::psql::list_rows()
            .get(dialog.selected("list").unwrap_or(0))
            .copied()
            .flatten();
        let text = match selected {
            Some((c, _, _)) if text.trim().is_empty() => c,
            _ => text,
        };
        let cmd = match parse(text) {
            Ok(c) => c,
            Err(e) => {
                self.dialogs.push(dialog);
                self.error(&e);
                return;
            }
        };
        let shown = crate::psql::without_backslash(text).to_owned();
        let Some(version) = self.tab_server_version() else {
            return;
        };
        let ctx = RenderContext {
            version,
            show_system: true,
        };
        match &cmd {
            Cmd::Help => self.send_catalog(vec![crate::psql::help_query()], &shown),
            Cmd::ConnInfo => self.conninfo(),
            Cmd::List { .. } | Cmd::Describe { .. } => {
                match queries(&cmd, &shown, &self.sql, ctx) {
                    Ok(q) => self.send_catalog(q, &shown),
                    Err(e) => self.error(&e),
                }
            }
            Cmd::Generate { what, table } => {
                if *what == crate::psql::Gen::Merge && !version.has(pgc_core::Feature::Merge) {
                    self.error("MERGE needs PostgreSQL 15 or later.");
                    return;
                }
                let tab = self.tabs[self.active_tab].id;
                let shown = match &table.0 {
                    Some(s) => format!("{s}.{}", table.1),
                    None => table.1.clone(),
                };
                let target = LookupTarget::Generate {
                    tab,
                    what: *what,
                    shown,
                };
                let params = vec![table.0.clone(), Some(table.1.clone())];
                self.generate_lookup("generate.sql", ctx, params, target, "Reading the table…");
            }
            Cmd::View { tables } => {
                let tab = self.tabs[self.active_tab].id;
                let shown = tables
                    .iter()
                    .map(|t| match &t.0 {
                        Some(s) => format!("{s}.{}", t.1),
                        None => t.1.clone(),
                    })
                    .collect();
                let params = crate::psql::view_params(tables);
                let target = LookupTarget::View { tab, shown };
                self.generate_lookup("view.sql", ctx, params, target, "Reading the tables…");
            }
            Cmd::Procedure { schema } => {
                // nothing to read: the skeleton goes in at once
                let text = crate::psql::generate_procedure(schema) + "\n";
                self.tabs[self.active_tab]
                    .view
                    .editor
                    .apply(Action::Insert(text));
                self.status = "Generated the procedure — name it at [procedure_name]".to_owned();
            }
            Cmd::Function { schema } => {
                let text = crate::psql::generate_function(schema) + "\n";
                self.tabs[self.active_tab]
                    .view
                    .editor
                    .apply(Action::Insert(text));
                self.status =
                    "Generated the function — name it at [function_name], fill in [return_type]"
                        .to_owned();
            }
        }
    }

    /// Sends the catalog read of a generate command through the tab's connection.
    fn generate_lookup(
        &mut self,
        file: &str,
        ctx: RenderContext,
        params: Vec<Option<String>>,
        target: LookupTarget,
        status: &str,
    ) {
        // reading the table would end a paused result (or the stream an export
        // reads from): refused, as for templates (D-019, D-046)
        if self
            .query()
            .is_some_and(|q| q.running.is_some() || q.export.is_some())
        {
            self.error(CONNECTION_BUSY);
            return;
        }
        let sql = match self.sql.render("psql", file, ctx) {
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
        self.next_key += 1;
        self.template_lookups.insert(self.next_key, target);
        handle.lookup_with(self.next_key, sql, params);
        self.status = status.to_owned();
    }

    /// Answer for `gtv`: the view at the cursor of the tab's editor.
    pub(super) fn view_loaded(
        &mut self,
        tab: u64,
        shown: &[String],
        result: Result<Vec<Vec<String>>, String>,
    ) {
        let rows = match result {
            Ok(r) => r,
            Err(e) => {
                self.error(&e);
                return;
            }
        };
        let (tables, fks) = match crate::psql::view_tables(rows, shown.len()) {
            Ok(t) => t,
            Err(i) => {
                self.error(&format!("Did not find a table named \"{}\".", shown[i]));
                return;
            }
        };
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let text = crate::psql::generate_view(&tables, &fks) + "\n";
        self.tabs[i].view.editor.apply(Action::Insert(text));
        self.status = "Generated the view — name it at [view_name]".to_owned();
    }

    /// Catalog queries as a run of the active query tab (results, Messages, time).
    fn send_catalog(&mut self, queries: Vec<pgc_db::CatalogQuery>, command: &str) {
        self.next_run += 1;
        let run = self.next_run;
        let Some(q) = self.query_mut() else {
            return;
        };
        let Some(conn) = q.conn.as_ref() else {
            return;
        };
        conn.handle.catalog(pgc_db::CatalogRequest { run, queries });
        q.start_run(run, Sent::new(0, ""));
        q.info(command.to_owned());
        self.status = "Executing…".to_owned();
    }

    /// `\conninfo`: the tab's connection, in Messages.
    pub(super) fn conninfo(&mut self) {
        let Some((uid, db)) = self.query().and_then(|q| q.binding.clone()) else {
            return;
        };
        let Some(s) = self.tree.servers.iter().find(|s| s.uid == uid) else {
            return;
        };
        let version = s
            .version
            .map(|v| format!(" PostgreSQL {}.", v.0 / 10000))
            .unwrap_or_default();
        let text = format!(
            "You are connected to database \"{db}\" as user \"{}\" on host \"{}\" at port \"{}\" (SSL mode {}).{version}",
            s.conn.username,
            s.conn.host,
            s.conn.port,
            s.conn.ssl_mode.name(),
        );
        if let Some(q) = self.query_mut() {
            q.info(text);
            q.show_messages = true;
            q.results_visible = true;
        }
    }

    /// Answer for a generate command: the statement at the cursor of the tab's editor.
    pub(super) fn generate_loaded(
        &mut self,
        tab: u64,
        what: crate::psql::Gen,
        shown: &str,
        result: Result<Vec<Vec<String>>, String>,
    ) {
        let rows = match result {
            Ok(r) => r,
            Err(e) => {
                self.error(&e);
                return;
            }
        };
        let Some(table) = crate::psql::gen_table(rows) else {
            self.error(&format!("Did not find a table named \"{shown}\"."));
            return;
        };
        use crate::psql::Gen;
        if what.tables_only() && !table.is_table() {
            self.error(&format!(
                "{} works on tables only; {}.{} is not a table.",
                what.command(),
                table.schema,
                table.name
            ));
            return;
        }
        if what == Gen::ProcUpdate && crate::psql::nothing_to_update(&table) {
            self.error(&format!(
                "gpu: every column of {}.{} is part of the key; there is nothing to update.",
                table.schema, table.name
            ));
            return;
        }
        if what.needs_keys() {
            // then its foreign keys, through the same connection
            let Some(version) = self.tab_server_version() else {
                return;
            };
            let ctx = RenderContext {
                version,
                show_system: true,
            };
            let params = vec![Some(table.schema.clone()), Some(table.name.clone())];
            let target = LookupTarget::Related { tab, what, table };
            self.generate_lookup(
                "foreign_keys.sql",
                ctx,
                params,
                target,
                "Reading the foreign keys…",
            );
            return;
        }
        self.insert_generated(tab, what, &table, crate::psql::generate(what, &table));
    }

    /// Answer for the foreign keys of `gtg` / `gtj` / `gtr`.
    pub(super) fn related_loaded(
        &mut self,
        tab: u64,
        what: crate::psql::Gen,
        table: &crate::psql::GenTable,
        result: Result<Vec<Vec<String>>, String>,
    ) {
        let rows = match result {
            Ok(r) => r,
            Err(e) => {
                self.error(&e);
                return;
            }
        };
        let rel = crate::psql::related(rows);
        match crate::psql::generate_related(what, table, &rel) {
            Ok(text) => self.insert_generated(tab, what, table, text),
            Err(e) => self.error(&e),
        }
    }

    /// The generated text at the cursor of tab `tab`'s editor, and what to do next.
    fn insert_generated(
        &mut self,
        tab: u64,
        what: crate::psql::Gen,
        table: &crate::psql::GenTable,
        text: String,
    ) {
        use crate::psql::Gen;
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        self.tabs[i].view.editor.apply(Action::Insert(text + "\n"));
        let todo = match what {
            Gen::Cursor => "write the loop body at PROCESS THE ROW HERE",
            Gen::Trigger { .. } => "name the function at [function_name]",
            Gen::ProcInsert | Gen::ProcUpdate | Gen::ProcDelete | Gen::ProcUpsert => {
                "name the procedure at [procedure_name]"
            }
            Gen::FuncRow | Gen::FuncSet | Gen::FuncTable | Gen::FuncExists | Gen::FuncJson => {
                "name the function at [function_name]"
            }
            Gen::TestData { .. } => "check the values, then run it",
            Gen::JoinParents => "add the WHERE you need",
            Gen::Audit => "name the function at [function_name]",
            _ => "fill in the [values]",
        };
        self.status = format!("Generated for {}.{} — {todo}", table.schema, table.name);
    }
}
