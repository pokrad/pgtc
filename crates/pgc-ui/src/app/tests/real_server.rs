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
            a.dialogs.last().map(|d| &d.kind),
            Some(&DialogKind::PsqlCommand)
        );
        typing(a, text);
        key(a, "Enter");
    };
    // a list with a pattern (system objects included because of the pattern)
    run(&mut a, "dt pg_catalog.pg_cl*");
    pump(&mut a, &mut rx, &hits, idle).await;
    let t = titles(&a);
    assert_eq!(t.len(), 1, "{t:?}");
    assert!(t[0].starts_with("1: dt pg_catalog.pg_cl* · "), "{t:?}");
    let rs = &a.query().unwrap().results[0];
    assert_eq!(rs.columns[0].name, "Schema");
    assert!(
        rs.rows.iter().any(|r| r[1].as_deref() == Some("pg_class")),
        "{:?}",
        rs.rows
    );
    // describe: sections without rows are left out
    run(&mut a, "d pgtc_psql");
    pump(&mut a, &mut rx, &hits, idle).await;
    let t = titles(&a);
    assert!(t[0].starts_with("1: Relation"), "{t:?}");
    assert!(t[1].starts_with("2: Columns · 3 rows"), "{t:?}");
    assert!(t.iter().any(|x| x.contains("Indexes")), "{t:?}");
    assert!(!t.iter().any(|x| x.contains("Triggers")), "{t:?}");
    // not found: a message, no result set
    run(&mut a, "d no_such_table_here");
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
    // ? and conninfo
    run(&mut a, "?");
    pump(&mut a, &mut rx, &hits, idle).await;
    assert!(titles(&a)[0].starts_with("1: ? · "));
    run(&mut a, "conninfo");
    assert!(a.query().unwrap().messages.iter().any(|m| {
        m.text
            .starts_with("You are connected to database \"postgres\"")
    }),);
    // generate: the statement at the cursor, the key in WHERE, [placeholders]
    run(&mut a, "gtu pgtc_psql");
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
    run(&mut a, "gti pgtc_psql");
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
    // gtv: a second table with a nullable foreign key to the first; typed first,
    // so the first table is joined with LEFT JOIN on that key
    a.editor().set_text(
        "CREATE TEMP TABLE pgtc_psql_items (item_id int PRIMARY KEY, \
         psql_id int REFERENCES pgtc_psql, note text)",
    );
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, idle).await;
    a.editor().set_text("");
    a.status.clear();
    run(&mut a, "gtv pgtc_psql_items pgtc_psql");
    pump(&mut a, &mut rx, &hits, |a| {
        a.status.starts_with("Generated")
    })
    .await;
    let text = a.editor().text();
    assert!(text.contains(".[view_name] AS\nSELECT\n"), "{text}");
    assert!(
        text.contains("    T1.note,\n    T2.id,\n    T2.name,\n    T2.note AS pgtc_psql_note\n"),
        "{text}"
    );
    assert!(
        text.contains(".pgtc_psql AS T2 ON T2.id = T1.psql_id;"),
        "{text}"
    );
    assert!(text.contains("LEFT JOIN "), "{text}");
    // gtv: a name that does not exist
    a.editor().set_text("");
    run(&mut a, "gtv pgtc_psql no_such_table_here");
    pump(&mut a, &mut rx, &hits, |a| {
        a.dialogs.last().is_some_and(|d| d.error)
    })
    .await;
    assert!(a.editor().text().is_empty());
    a.dialogs.clear();
    // gtrb / gtra / gtsb / gtsa: the trigger function and the trigger, the function
    // name a placeholder
    for (command, each) in [
        ("gtrb", "ROW"),
        ("gtra", "ROW"),
        ("gtsb", "STATEMENT"),
        ("gtsa", "STATEMENT"),
    ] {
        a.editor().set_text("");
        a.status.clear();
        run(&mut a, &format!("{command} pgtc_psql"));
        pump(&mut a, &mut rx, &hits, |a| {
            a.status.starts_with("Generated")
        })
        .await;
        let text = a.editor().text();
        assert!(
            text.contains(".[function_name]()\nRETURNS trigger"),
            "{text}"
        );
        let suffix = &command[2..];
        assert!(
            text.contains(&format!("CREATE TRIGGER pgtc_psql_{suffix}_trg")),
            "{text}"
        );
        assert!(text.contains(&format!("FOR EACH {each} ")), "{text}");
    }
    // gp / gpi / gpu / gpo / gpd: the procedures are created (in the temp schema of
    // the table) and called; a second delete of the same key raises "no row"
    let errors = |a: &App| -> Vec<String> {
        a.query()
            .unwrap()
            .messages
            .iter()
            .filter(|m| m.kind == crate::query_tab::MessageKind::Error)
            .map(|m| m.text.clone())
            .collect()
    };
    for (command, call) in [
        ("gp pg_temp", "CALL pg_temp.pgtc_psql_gp();"),
        (
            "gpi pgtc_psql",
            "CALL pg_temp.pgtc_psql_gpi('a', 'b', NULL);",
        ),
        ("gpu pgtc_psql", "CALL pg_temp.pgtc_psql_gpu(1, 'c', NULL);"),
        ("gpo pgtc_psql", "CALL pg_temp.pgtc_psql_gpo(2, 'd', 'e');"),
        ("gpd pgtc_psql", "CALL pg_temp.pgtc_psql_gpd(1);"),
    ] {
        a.editor().set_text("");
        a.status.clear();
        run(&mut a, command);
        pump(&mut a, &mut rx, &hits, |a| {
            a.status.starts_with("Generated")
        })
        .await;
        let name = format!("pgtc_psql_{}", &command[..command.find(' ').unwrap()]);
        let text = a.editor().text().replace("[procedure_name]", &name);
        assert!(text.contains("LANGUAGE plpgsql"), "{text}");
        a.editor().set_text(&format!("{text}\n{call}"));
        key(&mut a, "F5");
        pump(&mut a, &mut rx, &hits, idle).await;
        assert!(errors(&a).is_empty(), "{command}: {:?}\n{text}", errors(&a));
    }
    a.editor().set_text("CALL pg_temp.pgtc_psql_gpd(1);");
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, idle).await;
    assert!(
        errors(&a).iter().any(|e| e.contains("no row with id = 1")),
        "{:?}",
        errors(&a)
    );
    a.editor()
        .set_text("SELECT string_agg(id || ':' || name || ':' || coalesce(note, '-'), ',' ORDER BY id) FROM pgtc_psql");
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, idle).await;
    assert_eq!(
        a.query().unwrap().results[0].rows[0][0].as_deref(),
        Some("2:d:e")
    );
    // gf / gfr / gfs / gft / gfe / gfj: created with the placeholders filled in, then
    // called; the table now holds the one row 2:d:e
    for (command, fill, call, expected) in [
        (
            "gf pg_temp",
            &[("[return_type]", "integer"), ("[value]", "7")][..],
            "SELECT pg_temp.pgtc_psql_gf()",
            "7",
        ),
        (
            "gfr pgtc_psql",
            &[],
            "SELECT (pg_temp.pgtc_psql_gfr(2)).name",
            "d",
        ),
        (
            "gfs pgtc_psql",
            &[("[condition]", "id = 2")],
            "SELECT count(*) FROM pg_temp.pgtc_psql_gfs()",
            "1",
        ),
        (
            "gft pgtc_psql",
            &[("[condition]", "T1.id = 2")],
            "SELECT note FROM pg_temp.pgtc_psql_gft()",
            "e",
        ),
        (
            "gfe pgtc_psql",
            &[],
            "SELECT pg_temp.pgtc_psql_gfe(2)::text",
            "true",
        ),
        (
            "gfj pgtc_psql",
            &[],
            "SELECT pg_temp.pgtc_psql_gfj(2) ->> 'name'",
            "d",
        ),
    ] {
        a.editor().set_text("");
        a.status.clear();
        run(&mut a, command);
        pump(&mut a, &mut rx, &hits, |a| {
            a.status.starts_with("Generated")
        })
        .await;
        let name = format!("pgtc_psql_{}", &command[..command.find(' ').unwrap()]);
        let mut text = a.editor().text().replace("[function_name]", &name);
        for (from, to) in fill {
            text = text.replace(from, to);
        }
        a.editor().set_text(&format!("{text}\n{call}"));
        key(&mut a, "F5");
        pump(&mut a, &mut rx, &hits, idle).await;
        assert!(errors(&a).is_empty(), "{command}: {:?}\n{text}", errors(&a));
        assert_eq!(
            a.query().unwrap().results[0].rows[0][0].as_deref(),
            Some(expected),
            "{command}"
        );
    }
    // gfr / gfj: NULL for a key that is not there
    a.editor().set_text(
        "SELECT (pg_temp.pgtc_psql_gfr(99) IS NULL AND pg_temp.pgtc_psql_gfj(99) IS NULL)::text",
    );
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, idle).await;
    assert_eq!(
        a.query().unwrap().results[0].rows[0][0].as_deref(),
        Some("true")
    );
    // gtg / gtj / gtr / gtau on pgtc_psql_items (item_id key, psql_id → pgtc_psql,
    // which holds the one row 2); each script runs with its placeholders filled in
    for (command, fill, then, expected) in [
        (
            "gtg pgtc_psql_items 5",
            &[][..],
            "SELECT count(*) || ':' || min(psql_id) || ':' || max(item_id) FROM pgtc_psql_items",
            "5:2:5",
        ),
        (
            "gtj pgtc_psql_items",
            &[],
            "SELECT count(*) FROM pgtc_psql_items",
            "5",
        ),
        ("gtr pgtc_psql", &[("[id]", "2")], "SELECT 'ok'", "ok"),
        (
            "gtau pgtc_psql_items",
            &[("[function_name]", "pgtc_psql_audit_fn")],
            "UPDATE pgtc_psql_items SET note = 'x' WHERE item_id = 1; \
             SELECT operation || ':' || (new_row ->> 'note') FROM pgtc_psql_items_audit",
            "UPDATE:x",
        ),
    ] {
        a.editor().set_text("");
        a.status.clear();
        run(&mut a, command);
        pump(&mut a, &mut rx, &hits, |a| {
            a.status.starts_with("Generated")
        })
        .await;
        let mut text = a.editor().text();
        for (from, to) in fill {
            text = text.replace(from, to);
        }
        a.editor().set_text(&format!("{text}\n{then}"));
        key(&mut a, "F5");
        pump(&mut a, &mut rx, &hits, idle).await;
        assert!(errors(&a).is_empty(), "{command}: {:?}\n{text}", errors(&a));
        let q = a.query().unwrap();
        let last = q.results.last().expect("a result");
        assert_eq!(
            last.rows[0][0].as_deref(),
            Some(expected),
            "{command}\n{text}"
        );
        if command.starts_with("gtj") {
            // the join: every row with its parent
            assert_eq!(q.results[0].rows.len(), 5, "{text}");
        }
        if command.starts_with("gtr") {
            assert_eq!(q.results[0].rows.len(), 5, "{text}");
        }
    }
    a.editor().set_text("");
    // unknown command: an error, the command line stays open
    run(&mut a, "xyz");
    assert_eq!(a.dialogs.len(), 2);
    assert!(a.dialogs[1].error);
    assert_eq!(a.dialogs[0].kind, DialogKind::PsqlCommand);
    a.dialogs.clear();
    // the list: Down chooses the next command; Enter with an empty input runs
    // the selected one
    key(&mut a, "F4");
    key(&mut a, "Down");
    assert_eq!(a.dialogs[0].text("cmd"), Some("l "));
    key(&mut a, "Enter");
    pump(&mut a, &mut rx, &hits, idle).await;
    assert!(titles(&a)[0].starts_with("1: l · "), "{:?}", titles(&a));

    // F8 (D-073): what the AI would get, read inside a transaction that is rolled
    // back (nothing stays on the server; no request is sent in tests)
    a.editor().set_text(
        "BEGIN; CREATE SCHEMA pgtc_ai; \
         CREATE TYPE pgtc_ai.k AS ENUM ('a', 'b'); \
         CREATE TABLE pgtc_ai.t (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, kind pgtc_ai.k NOT NULL DEFAULT 'a'); \
         COMMENT ON TABLE pgtc_ai.t IS 'The t   table'; \
         COMMENT ON COLUMN pgtc_ai.t.kind IS 'What kind'; \
         CREATE VIEW pgtc_ai.v AS SELECT id FROM pgtc_ai.t; \
         CREATE FUNCTION pgtc_ai.f(p int) RETURNS int LANGUAGE sql AS 'SELECT p'; \
         CREATE PROCEDURE pgtc_ai.pr(x int) LANGUAGE sql AS 'SELECT 1';",
    );
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, idle).await;
    assert!(errors(&a).is_empty(), "{:?}", errors(&a));
    a.settings.ai = pgc_core::settings::AiSettings {
        enabled: true,
        base_url: "http://localhost:1/v1".into(),
        model: "m".into(),
        ..Default::default()
    };
    key(&mut a, "F8");
    assert!(
        matches!(
            a.dialogs.last().map(|d| &d.kind),
            Some(DialogKind::AskAi { .. })
        ),
        "{}",
        a.status
    );
    pump(&mut a, &mut rx, &hits, |a| a.ai.structure.is_some()).await;
    let text = match &a.ai.structure.as_ref().unwrap().1 {
        Ok(s) => s.text(Some("pgtc_ai")),
        Err(e) => panic!("{e}"),
    };
    assert!(
        text.starts_with(
            "TYPE pgtc_ai.k AS ENUM ('a', 'b');\n\
             -- The t table\n\
             TABLE pgtc_ai.t (\n  id integer NOT NULL GENERATED ALWAYS AS IDENTITY,\n  kind pgtc_ai.k NOT NULL DEFAULT 'a'::pgtc_ai.k, -- What kind\n  PRIMARY KEY (id)\n);\n\
             VIEW pgtc_ai.v (id integer);\n\
             FUNCTION pgtc_ai.f(p integer) RETURNS integer;\n\
             PROCEDURE pgtc_ai.pr("
        ),
        "{text}"
    );
    // (the argument modes of a procedure differ between versions)
    assert!(text.ends_with("x integer);\n"), "{text}");
    a.dialogs.clear();
    a.editor().set_text("ROLLBACK");
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, idle).await;
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

/// D-019: closing a paused result must not lose work. A script's earlier statements
/// and an open transaction survive (the remaining rows are read and dropped); a lone
/// SELECT is cancelled, and the cancel does not hit the next statement. Runs only when
/// `PGTC_TEST_PASSWORD` is set (local server, user postgres, database postgres).
#[tokio::test]
#[ignore]
async fn closing_a_paused_result_against_a_real_server() {
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
    // F5 on `sql`, until the run is paused or has finished
    async fn run(
        a: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
        hits: &Hits,
        sql: &str,
    ) {
        a.focus = Focus::Tabs;
        key(a, "Ctrl+A");
        typing(a, sql);
        key(a, "F5");
        pump(a, rx, hits, |a| !a.query().unwrap().busy()).await;
    }
    fn errors(a: &App) -> Vec<String> {
        let q = a.query().unwrap();
        q.messages
            .iter()
            .filter(|m| m.kind == crate::query_tab::MessageKind::Error)
            .map(|m| m.text.clone())
            .collect()
    }
    fn count(a: &App) -> Option<String> {
        let q = a.query().unwrap();
        q.results[0].rows[0][0].as_deref().map(str::to_owned)
    }
    const BIG: &str = "select g from generate_series(1, 300000) g";

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

    // auto commit on: the script's statements before the paused result are kept when
    // the next query closes it (one implicit transaction: a cancel would undo them)
    let script = format!("create temp table pgtc_p(x int); insert into pgtc_p values (1); {BIG};");
    run(&mut a, &mut rx, &hits, &script).await;
    assert!(a.query().unwrap().paused());
    run(&mut a, &mut rx, &hits, "select count(*) from pgtc_p").await;
    assert_eq!(errors(&a), Vec::<String>::new());
    assert_eq!(count(&a).as_deref(), Some("1"));

    // auto commit off: Commit on a paused result commits
    a.run(Command::ToggleAutoCommit);
    run(&mut a, &mut rx, &hits, "insert into pgtc_p values (2)").await;
    assert_eq!(a.query().unwrap().tx, TxStatus::InTransaction);
    run(&mut a, &mut rx, &hits, BIG).await;
    assert!(a.query().unwrap().paused());
    a.run(Command::Commit);
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().unwrap().running.is_none()
    })
    .await;
    let q = a.query().unwrap();
    assert!(
        q.messages.iter().any(|m| m.text == "COMMIT"),
        "{:?}",
        q.messages
    );
    assert_eq!(q.tx, TxStatus::Idle);
    a.run(Command::ToggleAutoCommit);
    run(&mut a, &mut rx, &hits, "select count(*) from pgtc_p").await;
    assert_eq!(count(&a).as_deref(), Some("2"));

    // the cancel key on a paused result inside a transaction: read to the end, the
    // transaction stays usable
    run(&mut a, &mut rx, &hits, &format!("begin; {BIG};")).await;
    assert!(a.query().unwrap().paused());
    a.run(Command::CancelQuery);
    assert!(a.query().unwrap().busy(), "closing counts as running");
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().unwrap().running.is_none()
    })
    .await;
    assert_eq!(errors(&a), Vec::<String>::new());
    assert_eq!(a.query().unwrap().tx, TxStatus::InTransaction);
    a.run(Command::Rollback);
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;

    // a lone SELECT is cancelled: reading its 100,000 slow rows would take minutes
    // (the pump gives up after 10 s), and the cancel does not hit the next statement.
    // The first rows are not slow — the first batch must be there within those 10 s,
    // and on Windows pg_sleep(0.002) takes about 16 ms (the timer's resolution) —,
    // with some to spare, as the server sends rows when its buffer is full.
    run(
        &mut a,
        &mut rx,
        &hits,
        "select g, case when g > 3000 then pg_sleep(0.002) end from generate_series(1, 100000) g",
    )
    .await;
    assert!(a.query().unwrap().paused());
    a.run(Command::CancelQuery);
    pump(&mut a, &mut rx, &hits, |a| {
        a.query().unwrap().running.is_none()
    })
    .await;
    assert_eq!(a.query().unwrap().tx, TxStatus::Idle);
    run(&mut a, &mut rx, &hits, "select 7 from pg_sleep(0.3)").await;
    assert_eq!(errors(&a), Vec::<String>::new());
    assert_eq!(count(&a).as_deref(), Some("7"));
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
    assert_eq!(a.editor().selected_text().as_deref(), Some("nosuchcol"));
    let q = a.query().unwrap();
    assert!(
        q.messages
            .iter()
            .any(|m| m.text.contains("\nLINE 2: select nosuchcol;\n")),
        "{:?}",
        q.messages
    );

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
    let notice = q.messages.iter().find(|m| m.text.contains("NOTICE:  hi 2"));
    assert_eq!(
        notice.and_then(|m| m.jump),
        Some(0),
        "PL/pgSQL mode: user line 1"
    );

    // SQL mode, DO after another statement (D-019): the notice jumps to its line, the
    // error in a statement of the body to its column (internal position)
    a.run(Command::TogglePlpgsql);
    a.editor().set_text(
        "select 1;\nDO $$\nBEGIN\n  RAISE NOTICE 'n';\n  INSERT INTO pgtc_nosuch VALUES (1);\nEND $$;",
    );
    a.focus = Focus::Tabs;
    key(&mut a, "F5");
    pump(&mut a, &mut rx, &hits, |a| !a.query().unwrap().busy()).await;
    let q = a.query().unwrap();
    let editor = &a.tabs[a.active_tab].view.editor;
    let at = |m: &crate::query_tab::Message| m.jump.map(|j| editor.line_of_char(j));
    let notice = q.messages.iter().find(|m| m.text.starts_with("NOTICE:  n"));
    assert_eq!(notice.and_then(&at), Some(3), "{:?}", q.messages);
    let error = q
        .messages
        .iter()
        .position(|m| m.text.contains("pgtc_nosuch"));
    let error = error.expect("the error");
    assert_eq!(at(&q.messages[error]), Some(4), "{:?}", q.messages);
    assert!(q.messages[error].exact, "{:?}", q.messages[error]);
    a.focus = Focus::Results;
    a.query_mut().unwrap().message_cursor = error;
    key(&mut a, "Enter");
    assert_eq!(a.editor().selected_text().as_deref(), Some("pgtc_nosuch"));
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
    // the templates folder is the only source (D-013): the built-in ones go there
    pgc_sql::devtemplate::install(&paths.user_templates_dir()).unwrap();
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
    assert_eq!(
        a.templates.form.as_ref().unwrap().title,
        "New: Create index"
    );
    // the table list comes through the tab's connection
    pump(&mut a, &mut rx, &hits, |a| a.template_lookups.is_empty()).await;
    let form = a.templates.form.as_ref().unwrap();
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
    let form = a.templates.form.as_mut().unwrap();
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
    let preview = a.templates.form.as_ref().unwrap().preview.text();
    assert_eq!(
        preview,
        "CREATE INDEX ON public.pgtc_tmpl_test (\"Name\");\n",
        "{:?}",
        a.templates.form.as_ref().unwrap().error
    );
    // OK inserts at the cursor; run it, then roll back
    a.templates.form.as_mut().unwrap().focus = crate::template_dialog::Stop::Ok;
    key(&mut a, "Enter");
    assert!(a.templates.form.is_none());
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
    a.files.dir = dir.path().to_path_buf();
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
    assert!(matches!(d.kind, DialogKind::SaveDefinition { .. }));
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
        assert!(matches!(d.kind, DialogKind::OpenDefinition { .. }));
        d.set_text("path", "pgtc_test_ui_src.pgdef");
        d.focus_on("path");
        a.file_dialog_done(d, crate::dialog::Button::Ok);
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
    a.files.dir = dir.path().to_path_buf();
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
