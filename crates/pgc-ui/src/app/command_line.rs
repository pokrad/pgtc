//! psql-style commands on `F4` (D-054): the command window, running a command,
//! `\conninfo` and the answers of the generate commands.

use pgc_editor::Action;
use pgc_sql::RenderContext;

use super::lookups::LookupTarget;
use super::{App, CONNECTION_BUSY};
use crate::dialog::{Dialog, DialogKind, Widget};
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
        let items: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Some((c, args, desc)) => format!("{c:<11}{args:<10}{desc}"),
                None => format!("{:─<20} pgtc {:─<44}", "", ""),
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
                Widget::button("ok", "&Run", true),
                Widget::button("cancel", "Ca&ncel", false),
            ],
        );
        d.fit_text(self.screen);
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
        let text = text.trim().to_owned();
        let shown = if text.starts_with('\\') {
            text.clone()
        } else {
            format!("\\{text}")
        };
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
                // reading the table would end a paused result (or the stream an
                // export reads from): refused, as for templates (D-019, D-046)
                if self
                    .query()
                    .is_some_and(|q| q.running.is_some() || q.export.is_some())
                {
                    self.error(CONNECTION_BUSY);
                    return;
                }
                let sql = match self.sql.render("psql", "generate.sql", ctx) {
                    Ok(s) => s,
                    Err(e) => {
                        self.error(&e.to_string());
                        return;
                    }
                };
                let tab = self.tabs[self.active_tab].id;
                let Some(handle) = self
                    .query()
                    .and_then(|q| q.conn.as_ref())
                    .map(|c| c.handle.clone())
                else {
                    return;
                };
                let name = match &table.0 {
                    Some(s) => format!("{s}.{}", table.1),
                    None => table.1.clone(),
                };
                self.next_key += 1;
                self.template_lookups.insert(
                    self.next_key,
                    LookupTarget::Generate {
                        tab,
                        what: *what,
                        shown: name,
                    },
                );
                handle.lookup_with(
                    self.next_key,
                    sql,
                    vec![table.0.clone(), Some(table.1.clone())],
                );
                self.status = "Reading the table…".to_owned();
            }
        }
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
        q.start_run(
            run,
            Sent {
                offset: 0,
                plpgsql_lines: None,
            },
        );
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
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let text = crate::psql::generate(what, &table) + "\n";
        self.tabs[i].view.editor.apply(Action::Insert(text));
        self.status = format!(
            "Generated for {}.{} — fill in the [values]",
            table.schema, table.name
        );
    }
}
