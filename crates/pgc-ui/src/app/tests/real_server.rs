use super::*;

use pgc_core::paths::Paths;
use pgc_core::settings::Settings;
use pgc_db::{DbEvent, EventSink, TxStatus};
use pgc_editor::Action;
use ratatui::layout::Rect;

use crate::dialog::{Dialog, DialogKind};
use crate::keymap::Command;
use crate::menu::MenuEntry;
use crate::tree::{ConnState, RowKind};
use crate::view::Hits;

/// psql-style commands against a real server (D-054): lists, describe, conninfo,
/// `\?`, the generate commands. Runs only when `PGTC_TEST_PASSWORD` is set.
#[tokio::test]
#[ignore]
async fn psql_commands_against_a_real_server() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let mut settings = Settings::default();
    settings.clipboard.system = false;
    let startup = pgc_db::conninfo::resolve_with(
        &pgc_db::conninfo::ConnArgs {
            host: Some("localhost".into()),
            username: Some("postgres".into()),
            dbname: Some("postgres".into()),
            ..Default::default()
        },
        &|_| None,
    )
    .unwrap();
    let startup = pgc_db::conninfo::Resolved {
        password: Some(password),
        ..startup
    };
    let mut a = App::new(paths, settings, sink, Some(startup));
    let hits = Hits::default();
    async fn pump(
        a: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
        hits: &Hits,
        done: impl Fn(&App) -> bool,
    ) {
        while !done(a) {
            let Ok(e) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
            else {
                panic!(
                    "timeout; status {:?}; messages {:?}",
                    a.status,
                    a.query().map(|q| &q.messages)
                );
            };
            a.update(Msg::Db(e.expect("event")), hits);
        }
    }
    let idle = |a: &App| a.query().is_some_and(|q| !q.busy());
    pump(&mut a, &mut rx, &hits, |a| {
        a.tree.servers[0].state == ConnState::Connected
    })
    .await;
    key(&mut a, "Home");
    a.new_query_tab(Some((a.tree.servers[0].uid, "postgres".to_owned())));
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().is_some_and(|q| q.connected())
    })
    .await;
    a.focus = Focus::Tabs;
    // a temp table for describe and the generate commands
    a.editor().set_text(
        "CREATE TEMP TABLE pgtc_psql (id serial PRIMARY KEY, name text NOT NULL, note text)",
    );
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, idle).await;
    a.editor().set_text("");
    let titles = |a: &App| -> Vec<String> {
        a.query()
            .unwrap()
            .results
            .iter()
            .enumerate()
            .map(|(i, r)| r.label(i + 1))
            .collect()
    };
    let run = |a: &mut App, text: &str| {
        key(a, "F4");
        assert_eq!(
            a.dialogs.last().map(|d| d.kind),
            Some(DialogKind::PsqlCommand)
        );
        typing(a, text);
        key(a, "Enter");
    };
    // a list with a pattern (system objects included because of the pattern)
    run(&mut a, "dt pg_catalog.pg_cl*");
    pump(&mut a, &mut rx, &hits, idle).await;
    let t = titles(&a);
    assert_eq!(t.len(), 1, "{t:?}");
    assert!(t[0].starts_with("1: \\dt pg_catalog.pg_cl* · "), "{t:?}");
    let rs = &a.query().unwrap().results[0];
    assert_eq!(rs.columns[0].name, "Schema");
    assert!(
        rs.rows.iter().any(|r| r[1].as_deref() == Some("pg_class")),
        "{:?}",
        rs.rows
    );
    // describe: sections without rows are left out
    run(&mut a, "\\d pgtc_psql");
    pump(&mut a, &mut rx, &hits, idle).await;
    let t = titles(&a);
    assert!(t[0].starts_with("1: Relation"), "{t:?}");
    assert!(t[1].starts_with("2: Columns · 3 rows"), "{t:?}");
    assert!(t.iter().any(|x| x.contains("Indexes")), "{t:?}");
    assert!(!t.iter().any(|x| x.contains("Triggers")), "{t:?}");
    // not found: a message, no result set
    run(&mut a, "\\d no_such_table_here");
    pump(&mut a, &mut rx, &hits, idle).await;
    let q = a.query().unwrap();
    assert!(q.results.is_empty());
    assert!(
        q.messages.iter().any(|m| m
            .text
            .contains("Did not find any relation named \"no_such_table_here\"")),
        "{:?}",
        q.messages
    );
    // \? and \conninfo
    run(&mut a, "\\?");
    pump(&mut a, &mut rx, &hits, idle).await;
    assert!(titles(&a)[0].starts_with("1: \\? · "));
    run(&mut a, "conninfo");
    assert!(a.query().unwrap().messages.iter().any(|m| {
        m.text
            .starts_with("You are connected to database \"postgres\"")
    }),);
    // generate: the statement at the cursor, the key in WHERE, [placeholders]
    run(&mut a, "\\gtu pgtc_psql");
    pump(&mut a, &mut rx, &hits, |a| {
        a.status.starts_with("Generated")
    })
    .await;
    let text = a.editor().text();
    assert!(
        text.contains(".pgtc_psql SET\n    name = [name],"),
        "{text}"
    );
    assert!(text.contains("WHERE id = [id];"), "{text}");
    assert!(text.contains("-- primary key: integer"), "{text}");
    // INSERT: the serial key (default nextval) is left out, the other columns listed
    a.status.clear();
    run(&mut a, "\\gti pgtc_psql");
    pump(&mut a, &mut rx, &hits, |a| {
        a.status.starts_with("Generated")
    })
    .await;
    let text = a.editor().text();
    assert!(
        text.contains(
            ".pgtc_psql (\n    name,  -- text, NOT NULL\n    note   -- text\n) VALUES (\n    [name],\n    [note]\n);"
        ),
        "{text}"
    );
    // unknown command: an error, the command line stays open
    run(&mut a, "\\xyz");
    assert_eq!(a.dialogs.len(), 2);
    assert!(a.dialogs[1].error);
    assert_eq!(a.dialogs[0].kind, DialogKind::PsqlCommand);
    a.dialogs.clear();
    // the list: Down chooses the next command; Enter with an empty input runs
    // the selected one
    key(&mut a, "F4");
    key(&mut a, "Down");
    assert_eq!(a.dialogs[0].text("cmd"), Some("\\l "));
    key(&mut a, "Enter");
    pump(&mut a, &mut rx, &hits, idle).await;
    assert!(titles(&a)[0].starts_with("1: \\l · "), "{:?}", titles(&a));
}

/// Autocomplete against a real server (D-052): relations after FROM, columns after
/// an alias, automatic opening after `.`. Runs only when `PGTC_TEST_PASSWORD` is set.
#[tokio::test]
#[ignore]
async fn autocomplete_against_a_real_server() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let mut settings = Settings::default();
    settings.clipboard.system = false;
    let startup = pgc_db::conninfo::resolve_with(
        &pgc_db::conninfo::ConnArgs {
            host: Some("localhost".into()),
            username: Some("postgres".into()),
            dbname: Some("postgres".into()),
            ..Default::default()
        },
        &|_| None,
    )
    .unwrap();
    let startup = pgc_db::conninfo::Resolved {
        password: Some(password),
        ..startup
    };
    let mut a = App::new(paths, settings, sink, Some(startup));
    let hits = Hits::default();
    async fn pump(
        a: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
        hits: &Hits,
        done: impl Fn(&App) -> bool,
    ) {
        while !done(a) {
            let Ok(e) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
            else {
                panic!("timeout; status {:?}", a.status);
            };
            a.update(Msg::Db(e.expect("event")), hits);
        }
    }
    pump(&mut a, &mut rx, &hits, |a| {
        a.tree.servers[0].state == ConnState::Connected
    })
    .await;
    key(&mut a, "Home");
    a.new_query_tab(Some((a.tree.servers[0].uid, "postgres".to_owned())));
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().is_some_and(|q| q.connected())
    })
    .await;
    a.focus = Focus::Tabs;
    let labels = |a: &App| -> Vec<String> {
        a.completion
            .as_ref()
            .map(|p| {
                p.items
                    .iter()
                    .map(|i| format!("{} {}", i.label, i.kind))
                    .collect()
            })
            .unwrap_or_default()
    };
    let set = |a: &mut App, text: &str| {
        let e = a.editor();
        e.set_text(text);
        let end = text.chars().count();
        e.set_cursor(end, false);
    };
    // relations after FROM: the names are read first ("loading…")
    set(&mut a, "select * from pg_cla");
    key(&mut a, "Ctrl+Space");
    assert!(a.completion.as_ref().is_some_and(|p| p.loading));
    pump(&mut a, &mut rx, &hits, |a| {
        a.completion.as_ref().is_some_and(|p| !p.loading)
    })
    .await;
    assert!(
        labels(&a).contains(&"pg_class table".to_owned()),
        "{:?}",
        labels(&a)
    );
    key(&mut a, "Enter");
    assert_eq!(a.editor().text(), "select * from pg_class");
    assert!(a.completion.is_none());
    // columns of an alias, read on demand
    set(&mut a, "select c.reln");
    a.editor().apply(Action::Insert(" from pg_class c".into()));
    let at = "select c.reln".chars().count();
    a.editor().set_cursor(at, false);
    key(&mut a, "Alt+/");
    pump(&mut a, &mut rx, &hits, |a| {
        a.completion.as_ref().is_some_and(|p| !p.loading)
    })
    .await;
    let l = labels(&a);
    assert_eq!(
        l.first().map(String::as_str),
        Some("relname column"),
        "{l:?}"
    );
    key(&mut a, "Tab");
    assert_eq!(a.editor().text(), "select c.relname from pg_class c");
    // typing `.` opens the list by itself; the names are cached now
    set(&mut a, "select x");
    a.editor().apply(Action::Insert(" from pg_class x".into()));
    a.editor().set_cursor("select x".chars().count(), false);
    typing(&mut a, ".");
    assert!(
        labels(&a).contains(&"relkind column".to_owned()),
        "{:?}",
        labels(&a)
    );
    // typing filters, Esc closes at once
    typing(&mut a, "relki");
    assert_eq!(labels(&a), ["relkind column"]);
    key(&mut a, "Esc");
    assert!(a.completion.is_none());
    // `schema.` opens the schema's tables by itself
    set(&mut a, "SELECT * FROM pg_catalog");
    typing(&mut a, ".");
    assert!(
        labels(&a).contains(&"pg_class table".to_owned()),
        "{:?}",
        labels(&a)
    );
    key(&mut a, "Esc");
    // keywords in upper case
    set(&mut a, "sel");
    key(&mut a, "Ctrl+Space");
    assert!(
        labels(&a).contains(&"SELECT keyword".to_owned()),
        "{:?}",
        labels(&a)
    );
}

/// End-to-end against a real server: connect, open a query tab, execute, page
/// through a paused result, error with line jump, transactions. Runs only when
/// `PGTC_TEST_PASSWORD` is set (local server, user postgres, database postgres).
#[tokio::test]
#[ignore]
async fn query_tab_against_a_real_server() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let mut settings = Settings::default();
    settings.clipboard.system = false;
    let startup = pgc_db::conninfo::resolve_with(
        &pgc_db::conninfo::ConnArgs {
            host: Some("localhost".into()),
            username: Some("postgres".into()),
            dbname: Some("postgres".into()),
            ..Default::default()
        },
        &|_| None,
    )
    .unwrap();
    let startup = pgc_db::conninfo::Resolved {
        password: Some(password),
        ..startup
    };
    let mut a = App::new(paths, settings, sink, Some(startup));
    let hits = Hits {
        grid: Rect::new(0, 0, 80, 10),
        ..Hits::default()
    };
    // pump database events until `done` holds
    async fn pump(
        a: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
        hits: &Hits,
        done: impl Fn(&App) -> bool,
    ) {
        while !done(a) {
            let Ok(e) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
            else {
                panic!(
                    "timeout; status {:?}; messages {:?}",
                    a.status,
                    a.query().map(|q| &q.messages)
                );
            };
            let e = e.expect("event");
            a.update(Msg::Db(e), hits);
        }
    }
    pump(&mut a, &mut rx, &hits, |a| {
        a.tree.servers[0].state == ConnState::Connected
    })
    .await;
    key(&mut a, "Home");
    // a query tab on the postgres database
    a.new_query_tab(Some((a.tree.servers[0].uid, "postgres".to_owned())));
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().is_some_and(|q| q.connected())
    })
    .await;
    assert_eq!(a.query().unwrap().database(), Some("postgres"));

    // 2,500 rows: first batch, then auto-fetch while moving down
    typing(
        &mut a,
        "select g, null::text n from generate_series(1, 2500) g;",
    );
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    let q = a.query().unwrap();
    assert_eq!(q.results.len(), 1);
    assert_eq!(q.results[0].rows.len(), 1000);
    assert!(q.paused());
    a.focus = Focus::Results;
    key(&mut a, "Ctrl+End");
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().unwrap().results[0].rows.len() == 2000
    })
    .await;
    key(&mut a, "Alt+End"); // fetch all
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().unwrap().running.is_none()
    })
    .await;
    assert_eq!(a.query().unwrap().results[0].rows.len(), 2500);
    assert_eq!(a.query().unwrap().results[0].rows[0][1], None);

    // error on line 2: Messages tab, Enter jumps to the error
    a.focus = Focus::Tabs;
    key(&mut a, "Ctrl+A");
    typing(&mut a, "select 1;");
    key(&mut a, "Enter");
    typing(&mut a, "select nosuchcol;");
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    let q = a.query().unwrap();
    assert!(q.show_messages);
    assert!(q.messages.iter().any(|m| m.text.contains("nosuchcol")));
    // the window status line says how the run ended, not empty (D-043)
    assert!(
        a.status.starts_with("Execution failed after ") && a.status.ends_with("see Messages"),
        "{}",
        a.status
    );
    a.focus = Focus::Results;
    key(&mut a, "Enter");
    assert_eq!(a.focus, Focus::Tabs);
    assert_eq!(a.editor().cursor_line_col().0, 1, "cursor on line 2");

    // auto commit off: an open transaction shows in the label; Rollback ends it
    a.run(Command::ToggleAutoCommit);
    key(&mut a, "Ctrl+A");
    typing(&mut a, "create temp table pgtc_t(x int)");
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    assert_eq!(a.query().unwrap().tx, TxStatus::InTransaction);
    assert!(
        a.status.starts_with("Execution finished in "),
        "{}",
        a.status
    );
    assert!(a.tabs[a.active_tab].label().contains("(tx)"));
    a.run(Command::Rollback);
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    assert_eq!(a.query().unwrap().tx, TxStatus::Idle);

    // PL/pgSQL mode: RAISE NOTICE appears in Messages
    a.run(Command::TogglePlpgsql);
    a.run(Command::ToggleAutoCommit);
    key(&mut a, "Ctrl+A");
    typing(&mut a, "raise notice 'hi %', 1+1;");
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    let q = a.query().unwrap();
    assert!(
        q.messages.iter().any(|m| m.text.contains("NOTICE:  hi 2")),
        "{:?}",
        q.messages
    );
    assert!(q.messages.iter().any(|m| m.text == "DO"));
}

/// Developer template end to end against a real server (D-046); runs only when
/// `PGTC_TEST_PASSWORD` is set. Everything happens in a rolled-back transaction.
#[tokio::test]
#[ignore]
async fn template_against_a_real_server() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let mut settings = Settings::default();
    settings.clipboard.system = false;
    let startup = pgc_db::conninfo::resolve_with(
        &pgc_db::conninfo::ConnArgs {
            host: Some("localhost".into()),
            username: Some("postgres".into()),
            dbname: Some("postgres".into()),
            ..Default::default()
        },
        &|_| None,
    )
    .unwrap();
    let startup = pgc_db::conninfo::Resolved {
        password: Some(password),
        ..startup
    };
    let mut a = App::new(paths, settings, sink, Some(startup));
    let hits = Hits {
        screen: Rect::new(0, 0, 120, 40),
        ..Hits::default()
    };
    async fn pump(
        a: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
        hits: &Hits,
        done: impl Fn(&App) -> bool,
    ) {
        while !done(a) {
            let Ok(e) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
            else {
                panic!("timeout; status {:?}", a.status);
            };
            a.update(Msg::Db(e.expect("event")), hits);
        }
    }
    pump(&mut a, &mut rx, &hits, |a| {
        a.tree.servers[0].state == ConnState::Connected
    })
    .await;
    key(&mut a, "Home");
    // a query tab on the postgres database
    a.new_query_tab(Some((a.tree.servers[0].uid, "postgres".to_owned())));
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().is_some_and(|q| q.connected())
    })
    .await;
    // a table that exists only inside this tab's transaction
    typing(
        &mut a,
        "begin; create table public.pgtc_tmpl_test(id int, \"Name\" text);",
    );
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    assert_eq!(a.query().unwrap().tx, TxStatus::InTransaction);
    key(&mut a, "Ctrl+A");
    key(&mut a, "Delete");

    // F7 in the editor: the picker; search for "create index"
    key(&mut a, "F7");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::TemplatePicker);
    typing(&mut a, "create index");
    key(&mut a, "Enter");
    assert!(
        a.dialogs.is_empty(),
        "{:?}",
        a.dialogs.last().map(|d| &d.title)
    );
    assert_eq!(a.template.as_ref().unwrap().title, "New: Create index");
    // the table list comes through the tab's connection
    pump(&mut a, &mut rx, &hits, |a| a.template_lookups.is_empty()).await;
    let form = a.template.as_ref().unwrap();
    let crate::template_dialog::FieldValue::Choice { items, .. } = &form.fields[0].value else {
        panic!("table is a choice");
    };
    assert!(
        items.iter().any(|c| c.label == "public.pgtc_tmpl_test"),
        "{items:?}"
    );
    // choose the table: open the list, filter, Enter
    key(&mut a, "Space");
    typing(&mut a, "pgtc_tmpl");
    key(&mut a, "Enter");
    // the columns list (field 5): Add opens the row dialog
    let form = a.template.as_mut().unwrap();
    form.focus = crate::template_dialog::Stop::Field(5);
    key(&mut a, "Enter");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::TemplateRow);
    typing(&mut a, "\"Name\"");
    key(&mut a, "Alt+O");
    assert!(
        a.dialogs.is_empty(),
        "{:?}",
        a.dialogs.last().map(|d| &d.title)
    );
    let preview = a.template.as_ref().unwrap().preview.text();
    assert_eq!(
        preview,
        "CREATE INDEX ON public.pgtc_tmpl_test (\"Name\");\n",
        "{:?}",
        a.template.as_ref().unwrap().error
    );
    // OK inserts at the cursor; run it, then roll back
    a.template.as_mut().unwrap().focus = crate::template_dialog::Stop::Ok;
    key(&mut a, "Enter");
    assert!(a.template.is_none());
    assert_eq!(a.editor().text(), preview);
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    let q = a.query().unwrap();
    assert!(
        q.messages.iter().any(|m| m.text == "CREATE INDEX"),
        "{:?}",
        q.messages
    );
    a.run(Command::Rollback);
    pump(&mut a, &mut rx, &hits, |a| {
        let q = a.query().unwrap();
        !q.busy() && q.tx == TxStatus::Idle
    })
    .await;
}

/// Export and load of a database definition through the menu commands (D-048):
/// export one database, load the file into an empty one, run the script, load
/// again. Runs only when `PGTC_TEST_PASSWORD` is set; creates and drops
/// `pgtc_test_ui_src` / `pgtc_test_ui_dst`.
#[tokio::test]
#[ignore]
async fn definition_export_and_load_against_a_real_server() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let (admin, conn) = tokio_postgres::connect(
        &format!("host=localhost user=postgres dbname=postgres password={password}"),
        tokio_postgres::NoTls,
    )
    .await
    .unwrap();
    tokio::spawn(conn);
    for db in ["pgtc_test_ui_src", "pgtc_test_ui_dst"] {
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"))
            .await
            .unwrap();
        admin
            .batch_execute(&format!("CREATE DATABASE {db}"))
            .await
            .unwrap();
    }
    let (src, conn) = tokio_postgres::connect(
        &format!("host=localhost user=postgres dbname=pgtc_test_ui_src password={password}"),
        tokio_postgres::NoTls,
    )
    .await
    .unwrap();
    tokio::spawn(conn);
    src.batch_execute(
        "CREATE SCHEMA app;
             CREATE TABLE app.t (id int PRIMARY KEY, name text NOT NULL);
             CREATE VIEW app.v AS SELECT id FROM app.t;",
    )
    .await
    .unwrap();
    drop(src);

    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let mut settings = Settings::default();
    settings.clipboard.system = false;
    let startup = pgc_db::conninfo::resolve_with(
        &pgc_db::conninfo::ConnArgs {
            host: Some("localhost".into()),
            username: Some("postgres".into()),
            dbname: Some("postgres".into()),
            ..Default::default()
        },
        &|_| None,
    )
    .unwrap();
    let startup = pgc_db::conninfo::Resolved {
        password: Some(password),
        ..startup
    };
    let mut a = App::new(paths, settings, sink, Some(startup));
    a.file_dir = dir.path().to_path_buf();
    let hits = Hits::default();
    async fn pump(
        a: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
        hits: &Hits,
        done: impl Fn(&App) -> bool,
    ) {
        while !done(a) {
            let Ok(e) = tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv()).await
            else {
                panic!(
                    "timeout; status {:?}; dialogs {:?}",
                    a.status,
                    a.dialogs.last().map(labels)
                );
            };
            a.update(Msg::Db(e.expect("event")), hits);
        }
    }
    fn labels(d: &Dialog) -> String {
        d.widgets
            .iter()
            .filter(|w| matches!(w.kind, crate::dialog::WidgetKind::Label))
            .map(|w| w.label.text.clone())
            .collect::<Vec<_>>()
            .join(" ")
    }
    /// Row of the node with `name` (`label` prefix for groups).
    fn find(a: &App, name: &str) -> Option<crate::tree::Row> {
        a.tree.rows().into_iter().find(|r| match r.kind {
            RowKind::Node(nid) => a
                .tree
                .node(nid)
                .is_some_and(|n| n.name == name || n.label.starts_with(name)),
            _ => false,
        })
    }
    pump(&mut a, &mut rx, &hits, |a| {
        a.tree.servers[0].state == ConnState::Connected
    })
    .await;
    key(&mut a, "Home");
    assert!(
        !a.is_enabled(Command::ExportDefinition),
        "a server is selected"
    );
    if find(&a, "Databases").is_none() {
        a.expand();
    }
    a.tree.select(find(&a, "Databases").unwrap());
    a.expand();
    pump(&mut a, &mut rx, &hits, |a| {
        find(a, "pgtc_test_ui_src").is_some()
    })
    .await;

    // export
    a.tree.select(find(&a, "pgtc_test_ui_src").unwrap());
    assert!(a.is_enabled(Command::ExportDefinition));
    // the database's context menu offers both commands; `e` exports
    a.run(Command::ContextMenu);
    let cmds: Vec<Command> = a
        .popup
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .filter_map(|e| match e {
            MenuEntry::Item { cmd, .. } => Some(*cmd),
            _ => None,
        })
        .collect();
    assert!(cmds.contains(&Command::ExportDefinition), "{cmds:?}");
    assert!(cmds.contains(&Command::LoadDefinition), "{cmds:?}");
    assert!(cmds.contains(&Command::NewQuery), "{cmds:?}");
    assert!(cmds.contains(&Command::OpenFile), "{cmds:?}");
    assert!(!cmds.contains(&Command::ShowInNewTab), "{cmds:?}");
    assert!(a.is_enabled(Command::NewQuery) && a.is_enabled(Command::OpenFile));
    key(&mut a, "e");
    assert!(a.popup.is_none());
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.kind, DialogKind::SaveDefinition);
    assert_eq!(d.text("path"), Some("pgtc_test_ui_src.pgdef"));
    key(&mut a, "Alt+O");
    assert!(a.dialogs.is_empty());
    pump(&mut a, &mut rx, &hits, |a| a.jobs.is_empty()).await;
    let file = dir.path().join("pgtc_test_ui_src.pgdef");
    let text = std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{}", a.status));
    assert!(text.contains("\"name\": \"app\""), "{text}");
    assert!(
        a.status
            .starts_with("Definition of pgtc_test_ui_src exported")
    );

    // load into the empty database: a script in a query tab of that database
    let load = |a: &mut App| {
        a.tree.select(find(a, "pgtc_test_ui_dst").unwrap());
        a.run(Command::LoadDefinition);
        let mut d = a.dialogs.pop().unwrap();
        assert_eq!(d.kind, DialogKind::OpenDefinition);
        d.set_text("path", "pgtc_test_ui_src.pgdef");
        d.focus_on("path");
        a.definition_dialog_done(d);
    };
    let tabs = a.tabs.len();
    load(&mut a);
    pump(&mut a, &mut rx, &hits, |a| a.jobs.is_empty()).await;
    assert_eq!(a.tabs.len(), tabs + 1, "{:?}", a.dialogs.last().map(labels));
    let script = a.tabs[a.active_tab].view.editor.text();
    assert!(script.contains("CREATE TABLE app.t ("), "{script}");
    assert!(script.contains("CREATE VIEW app.v AS"), "{script}");
    assert_eq!(
        a.query().unwrap().binding,
        Some((a.tree.servers[0].uid, "pgtc_test_ui_dst".to_owned()))
    );
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().is_some_and(|q| q.connected())
    })
    .await;
    a.focus = Focus::Tabs;
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    let q = a.query().unwrap();
    assert!(
        !q.messages
            .iter()
            .any(|m| m.kind == crate::query_tab::MessageKind::Error),
        "{:?}",
        q.messages
    );

    // loaded again: nothing to do
    a.focus = Focus::Tree;
    load(&mut a);
    pump(&mut a, &mut rx, &hits, |a| a.jobs.is_empty()).await;
    assert_eq!(a.tabs.len(), tabs + 1, "no new tab");
    let d = a.dialogs.last().unwrap();
    assert!(labels(d).contains("already matches"), "{}", labels(d));

    drop(a);
    for db in ["pgtc_test_ui_src", "pgtc_test_ui_dst"] {
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"))
            .await
            .unwrap();
    }
}

/// Export of a paused result from a real server (D-042); runs only when
/// `PGTC_TEST_PASSWORD` is set.
#[tokio::test]
#[ignore]
async fn export_against_a_real_server() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Msg>();
    let db_tx = tx.clone();
    let sink: EventSink = Arc::new(move |e| {
        let _ = db_tx.send(Msg::Db(e));
    });
    let mut settings = Settings::default();
    settings.clipboard.system = false;
    let startup = pgc_db::conninfo::resolve_with(
        &pgc_db::conninfo::ConnArgs {
            host: Some("localhost".into()),
            username: Some("postgres".into()),
            dbname: Some("postgres".into()),
            ..Default::default()
        },
        &|_| None,
    )
    .unwrap();
    let startup = pgc_db::conninfo::Resolved {
        password: Some(password),
        ..startup
    };
    let mut a = App::new(paths, settings, sink, Some(startup));
    a.set_export_sink(Arc::new(move |e| {
        let _ = tx.send(Msg::Export(e));
    }));
    a.file_dir = dir.path().to_path_buf();
    let hits = Hits {
        grid: Rect::new(0, 0, 80, 10),
        ..Hits::default()
    };
    async fn pump(
        a: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Msg>,
        hits: &Hits,
        done: impl Fn(&App) -> bool,
    ) {
        while !done(a) {
            let Ok(m) = tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv()).await
            else {
                panic!(
                    "timeout; status {:?}; messages {:?}",
                    a.status,
                    a.query().map(|q| &q.messages)
                );
            };
            a.update(m.expect("message"), hits);
        }
    }
    pump(&mut a, &mut rx, &hits, |a| {
        a.tree.servers[0].state == ConnState::Connected
    })
    .await;
    key(&mut a, "Home");
    // a query tab on the postgres database
    a.new_query_tab(Some((a.tree.servers[0].uid, "postgres".to_owned())));
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().is_some_and(|q| q.connected())
    })
    .await;

    // a paused result of 2,500 rows, then a second statement
    typing(
        &mut a,
        "select g, 'x;' || g t, g % 3 = 0 b from generate_series(1, 2500) g; select 1 as after;",
    );
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    assert!(a.query().unwrap().paused());
    a.run(Command::ExportResult);
    key(&mut a, "Alt+O"); // result_1.csv, comma, no BOM
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().unwrap().export.is_none() && a.query().unwrap().running.is_none()
    })
    .await;
    let out = std::fs::read_to_string(dir.path().join("result_1.csv")).unwrap();
    let lines: Vec<&str> = out.split("\r\n").collect();
    assert_eq!(lines.len(), 2502, "header + rows + empty tail");
    assert_eq!(lines[0], "g,t,b");
    assert_eq!(lines[1], "1,x;1,f");
    assert_eq!(lines[2500], "2500,x;2500,f");
    let q = a.query().unwrap();
    assert_eq!(q.results.len(), 2, "{:?}", q.messages);
    assert_eq!(q.results[0].rows.len(), 1000);
    assert_eq!(
        q.results[0].label(1),
        "1: 1,000 of 2,500 rows (rest exported)"
    );
    assert!(
        q.messages
            .iter()
            .any(|m| m.text.starts_with("Exported 2,500 rows")),
        "{:?}",
        q.messages
    );

    // cancelled while streaming: the file is deleted, the result is closed
    a.focus = Focus::Tabs;
    key(&mut a, "Ctrl+A");
    typing(&mut a, "select g from generate_series(1, 3000000) g;");
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    a.run(Command::ExportResult);
    key(&mut a, "Alt+O");
    key(&mut a, "y"); // overwrite result_1.csv
    pump(&mut a, &mut rx, &hits, |a| {
        a.query()
            .unwrap()
            .export
            .as_ref()
            .is_some_and(|e| e.written >= 5000)
    })
    .await;
    key(&mut a, "Alt+Shift+C");
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().unwrap().export.is_none() && a.query().unwrap().running.is_none()
    })
    .await;
    assert!(!dir.path().join("result_1.csv").exists());
    let q = a.query().unwrap();
    assert!(!q.paused());
    assert!(
        q.messages
            .iter()
            .any(|m| m.text.starts_with("Export cancelled")),
        "{:?}",
        q.messages
    );

    // disconnecting the server closes the query tabs and every connection (D-043)
    // a second, unchanged tab of the same server
    a.new_query_tab(Some((a.tree.servers[0].uid, "postgres".to_owned())));
    assert!(a.connections.len() >= 3, "{:?}", a.connections);
    a.focus = Focus::Tree;
    key(&mut a, "Home");
    a.run(Command::Disconnect);
    key(&mut a, "n"); // the first tab has unsaved text
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
    assert!(a.connections.is_empty(), "{:?}", a.connections);
    assert_eq!(a.tree.servers[0].state, ConnState::Disconnected);
}
