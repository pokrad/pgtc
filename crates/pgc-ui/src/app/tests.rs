mod real_server;
mod screenshots;

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use pgc_core::paths::Paths;
use pgc_core::settings::{ExportFormat, SavedConnection, Settings, SslMode};
use pgc_db::{ConnId, DbError, DbEvent, EventSink, TxStatus};
use pgc_sql::devtemplate;
use ratatui::layout::Rect;

use crate::app::autocomplete::NAMES_HELD;
use crate::app::lookups::LookupTarget;
use crate::dialog::DialogKind;
use crate::export::ExportEvent;
use crate::keymap::Command;
use crate::keys::KeyCombo;
use crate::menu::{self, MenuEntry};
use crate::query_tab::{LineMap, QueryConn, QueryConnState, QueryState, Sent};
use crate::setup;
use crate::template_dialog::{FormEvent, TemplateForm};
use crate::tree::ConnState;
use crate::view::Hits;

use super::*;

fn app() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_dirs(dir.path().join("cfg"), dir.path().join("data"));
    let sink: EventSink = Arc::new(|_| {});
    // Tests must not touch the real system clipboard.
    let mut settings = Settings::default();
    settings.clipboard.system = false;
    (App::new(paths, settings, sink, None), dir)
}

fn key(app: &mut App, k: &str) -> Vec<Effect> {
    let c = KeyCombo::parse(k).unwrap();
    app.update(
        Msg::Input(Event::Key(KeyEvent::new(c.code, c.mods))),
        &Hits::default(),
    )
}

/// A lone Esc: the key plus its timeout (Esc+digit wait, D-016).
fn esc(app: &mut App) {
    for e in key(app, "Esc") {
        if let Effect::StartEscTimer(n) = e {
            app.update(Msg::EscTimeout(n), &Hits::default());
        }
    }
}

fn typing(app: &mut App, text: &str) {
    for c in text.chars() {
        app.update(
            Msg::Input(Event::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            ))),
            &Hits::default(),
        );
    }
}

#[test]
fn setup_menu_items_are_enabled() {
    let (a, _d) = app();
    for c in [
        Command::RunSetup,
        Command::InstallTemplates,
        Command::ShowTemplatesFolder,
    ] {
        assert!(a.is_enabled(c), "{c:?}");
    }
    let settings = menu::menu_bar()
        .into_iter()
        .find(|m| m.title.text == "Settings")
        .unwrap();
    assert!(settings.entries.iter().any(|e| matches!(
        e,
        MenuEntry::Item {
            cmd: Command::RunSetup,
            ..
        }
    )));
}

/// Install templates (D-013): a missing folder is filled without asking; an existing
/// one only after Yes, and No keeps it as it is.
#[test]
fn install_templates_asks_before_overwriting() {
    let (mut a, _d) = app();
    let dir = a.paths.user_templates_dir();
    a.run(Command::InstallTemplates);
    let d = a.dialogs.pop().unwrap();
    assert_eq!(d.kind, DialogKind::Message, "no question for a new folder");
    let (first, _) = &devtemplate::embedded_files()[0];
    let file = dir.join(first);
    assert!(file.is_file());
    std::fs::write(&file, "changed").unwrap();
    // the folder exists: asked; No (the focused default of the question) keeps it
    a.run(Command::InstallTemplates);
    assert_eq!(
        a.dialogs.last().unwrap().kind,
        DialogKind::ConfirmTemplatesOverwrite
    );
    key(&mut a, "n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "changed");
    assert!(
        a.dialogs.pop().unwrap().widgets[0]
            .label
            .text
            .contains("not installed")
    );
    // Yes: overwritten
    a.run(Command::RunSetup);
    key(&mut a, "y");
    assert_ne!(std::fs::read_to_string(&file).unwrap(), "changed");
    assert!(
        a.dialogs.pop().unwrap().widgets[0]
            .label
            .text
            .contains("overwritten")
    );
}

#[test]
fn steps_are_shown_in_one_message() {
    let (mut a, _d) = app();
    let ok = |t: &str| setup::StepOutcome {
        ok: true,
        text: t.into(),
    };
    a.show_steps("Run setup", vec![ok("Templates: done")]);
    let d = a.dialogs.pop().unwrap();
    assert!(!d.error);
    a.show_steps(
        "Run setup",
        vec![setup::StepOutcome {
            ok: false,
            text: "Templates: failed".into(),
        }],
    );
    assert!(a.dialogs.pop().unwrap().error, "a failed step is an error");
}

#[test]
fn esc_digit_is_fkey() {
    let (mut a, _d) = app();
    assert_eq!(key(&mut a, "Esc"), vec![Effect::StartEscTimer(1)]);
    key(&mut a, "9"); // F9 opens the menu
    assert!(a.menu.is_some());
    key(&mut a, "Esc");
    a.update(Msg::EscTimeout(1), &Hits::default()); // stale timer: ignored
    assert!(a.menu.is_some());
    a.update(Msg::EscTimeout(2), &Hits::default());
    assert!(a.menu.is_none());
    key(&mut a, "Esc");
    key(&mut a, "0"); // F10 quits
    assert!(a.should_quit());
}

#[test]
fn tabs_open_switch_close() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    a.new_query_tab(None);
    assert_eq!(a.tabs.len(), 3);
    assert_eq!(a.active_tab, 2);
    key(&mut a, "Alt+Right");
    assert_eq!(a.active_tab, 0);
    key(&mut a, "Ctrl+W");
    assert_eq!(a.tabs.len(), 3, "default tab is not closed");
    key(&mut a, "Alt+Left");
    key(&mut a, "Ctrl+W");
    let kinds: Vec<TabKind> = a.tabs.iter().map(|t| t.kind.clone()).collect();
    assert_eq!(kinds, vec![TabKind::Default, TabKind::Query(1)]);
}

#[test]
fn ctrl_tab_switches_panel_tab_does_not() {
    let (mut a, _d) = app();
    key(&mut a, "Tab");
    assert_eq!(a.focus, Focus::Tree, "Tab does nothing in the tree");
    key(&mut a, "Ctrl+Tab");
    assert_eq!(a.focus, Focus::Tabs);
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Tree);
    a.run(Command::Settings);
    key(&mut a, "Ctrl+Tab");
    assert_eq!(a.focus, Focus::Tree, "not behind a dialog");
    key(&mut a, "Tab");
    assert_eq!(a.dialogs.len(), 1, "dialog owns Tab");
}

#[test]
fn settings_dialog_applies_and_saves() {
    let (mut a, _d) = app();
    a.run(Command::Settings);
    key(&mut a, "Space"); // icons on
    key(&mut a, "Alt+S"); // focus the colour scheme combo
    key(&mut a, "Space"); // open it
    key(&mut a, "Down"); // Dark
    key(&mut a, "Enter"); // pick it
    key(&mut a, "Alt+T");
    key(&mut a, "Backspace");
    key(&mut a, "Backspace");
    typing(&mut a, "30");
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
    assert!(a.settings.ui.icons);
    assert_eq!(a.theme.name, "Dark");
    let saved = Settings::load(&a.paths.settings_file()).unwrap();
    assert_eq!(saved.ui.color_scheme, "Dark");
    assert_eq!(saved.connection.connect_timeout_secs, 30);
}

#[test]
fn settings_dialog_query_clipboard_and_frames() {
    let (mut a, _d) = app();
    a.run(Command::Settings);
    // fits a 24-row terminal (D-051)
    let screen = Rect::new(0, 0, 80, 24);
    assert!(a.dialogs[0].area(screen).height <= 24);
    key(&mut a, "Alt+R");
    for _ in 0..4 {
        key(&mut a, "Backspace");
    }
    typing(&mut a, "250");
    key(&mut a, "Alt+W");
    key(&mut a, "Backspace");
    typing(&mut a, "9");
    key(&mut a, "Alt+M"); // auto commit off for new tabs
    key(&mut a, "Alt+B"); // auto rollback on
    // (tests keep the real system clipboard off)
    key(&mut a, "Alt+5"); // OSC 52 off
    key(&mut a, "Alt+F"); // ASCII frames on
    key(&mut a, "Enter");
    assert!(
        a.dialogs.is_empty(),
        "{:?}",
        a.dialogs.last().map(|d| &d.title)
    );
    let saved = Settings::load(&a.paths.settings_file()).unwrap();
    assert_eq!(saved.query.batch_rows, 250);
    assert_eq!(saved.query.cancel_wait_secs, 9);
    assert!(!saved.query.auto_commit);
    assert!(saved.query.auto_rollback);
    assert!(!saved.clipboard.system);
    assert!(!saved.clipboard.osc52);
    assert!(saved.ui.ascii_frames);
    // new query tabs take the defaults
    let q = QueryState::new(a.settings.query.auto_commit, a.settings.query.auto_rollback);
    assert!(!q.auto_commit && q.auto_rollback);
    // ASCII frames: no box-drawing characters anywhere on screen
    let text = screen_text(&a);
    assert!(
        text.contains("+=== Connections") || text.contains("= Connections ="),
        "{text}"
    );
    assert!(text.contains("|+-----"), "inner separator: {text}");
    assert!(
        !text.chars().any(|c| ('\u{2500}'..='\u{257F}').contains(&c)),
        "{text}"
    );
    // a wrong batch size keeps the dialog open
    a.run(Command::Settings);
    key(&mut a, "Alt+R");
    typing(&mut a, "x");
    key(&mut a, "Enter");
    assert_eq!(a.dialogs.len(), 2);
    assert_eq!(a.dialogs[0].focused_id(), Some("batch"));
}

#[test]
fn notify_shows_in_messages() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    let tab = a.tabs[a.active_tab].id;
    let id = 77;
    a.connections.insert(id, Target::Query(tab));
    a.update(
        Msg::Db(DbEvent::Notice {
            id,
            notice: pgc_db::ServerMessage::notification("jobs", 42, "done"),
        }),
        &Hits::default(),
    );
    let q = a.tabs[a.active_tab].query.as_ref().unwrap();
    assert!(
        q.messages
            .iter()
            .any(|m| m.text == "NOTIFY:  channel \"jobs\" (pid 42): done"),
        "{:?}",
        q.messages.iter().map(|m| &m.text).collect::<Vec<_>>()
    );
}

#[test]
fn invalid_timeout_keeps_dialog_open() {
    let (mut a, _d) = app();
    a.run(Command::Settings);
    key(&mut a, "Alt+T");
    typing(&mut a, "x");
    key(&mut a, "Enter");
    assert_eq!(a.dialogs.len(), 2);
    assert_eq!(a.dialogs[0].kind, DialogKind::Settings);
    assert!(a.dialogs[1].error);
}

#[test]
fn menu_runs_enabled_items_only() {
    let (mut a, _d) = app();
    key(&mut a, "F9");
    key(&mut a, "c"); // File → Connect is disabled (no server)
    assert!(a.menu.is_some());
    key(&mut a, "q"); // File → New query is disabled (no database, D-049)
    assert!(a.menu.is_some());
    key(&mut a, "n"); // File → New connection…
    assert!(a.menu.is_none());
    assert_eq!(a.dialogs[0].kind, DialogKind::Connection);
}

#[test]
fn query_and_file_commands_need_a_database_or_a_query_tab() {
    let (mut a, _d) = app();
    add_server(&mut a, "a");
    // a server row: no database, so no new query or file open; no query tab to save
    for c in [Command::NewQuery, Command::OpenFile, Command::SaveFile] {
        assert!(!a.is_enabled(c), "{c:?}");
    }
    a.run(Command::OpenFile);
    assert!(a.dialogs.is_empty());
    a.run(Command::SaveFile);
    assert!(a.dialogs.is_empty());
    assert!(a.status.contains("query tab"), "{}", a.status);
    // the server's context menu has neither
    a.run(Command::ContextMenu);
    let cmds: Vec<Command> = a
        .popup
        .take()
        .unwrap()
        .entries
        .iter()
        .filter_map(|e| match e {
            MenuEntry::Item { cmd, .. } => Some(*cmd),
            _ => None,
        })
        .collect();
    assert!(!cmds.contains(&Command::NewQuery), "{cmds:?}");
    assert!(!cmds.contains(&Command::ShowInNewTab), "{cmds:?}");
    // a query tab can be saved
    a.new_query_tab(None);
    assert!(a.is_enabled(Command::SaveFile));
}

fn add_server(a: &mut App, name: &str) -> u64 {
    let uid = a.tree.add(SavedConnection {
        name: name.into(),
        ..SavedConnection::default()
    });
    a.persist_servers();
    uid
}

/// Pretends that connection `id` belongs to server `uid` and is connecting.
fn fake_connecting(a: &mut App, uid: u64) -> ConnId {
    a.next_conn += 1;
    let id = a.next_conn;
    a.connections.insert(id, Target::Server(uid));
    a.tree.by_uid(uid).unwrap().state = ConnState::Connecting;
    id
}

#[test]
fn connect_events_drive_the_tree() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "srv");
    let id = fake_connecting(&mut a, uid);
    // `DbEvent::Connected` needs a real CancelToken; call its handler directly.
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    assert_eq!(a.tree.servers[0].state, ConnState::Connected);
    assert!(a.status.contains("PostgreSQL 17.2"), "{}", a.status);
    let databases = a.tree.servers[0].children[0].nid;
    assert!(a.tree.node(databases).unwrap().expanded);
    // No real connection in the test: answer the group's query by hand.
    a.pending.insert(99, databases);
    let row = |oid, name: &str| pgc_db::NodeRow {
        oid,
        name: name.into(),
        label: None,
        partitioned: false,
    };
    a.update(
        Msg::Db(DbEvent::Nodes {
            id,
            key: 99,
            result: Ok(vec![row(1, "app"), row(5, "postgres")]),
        }),
        &Hits::default(),
    );
    let labels: Vec<String> = a
        .tree
        .rows()
        .iter()
        .map(|r| a.tree.row_text(*r).label)
        .collect();
    assert_eq!(
        labels,
        vec![
            "srv (connected)",
            "Databases (2)",
            "app",
            "postgres",
            "Login/Group Roles",
            "Tablespaces"
        ]
    );
    // Quick search: "po" jumps to postgres.
    key(&mut a, "Ctrl+S");
    typing(&mut a, "po");
    assert_eq!(a.tree.row_name(a.tree.selected().unwrap()), "postgres");
    key(&mut a, "Enter");
    assert!(a.search.is_none());
    a.update(
        Msg::Db(DbEvent::Closed {
            id,
            error: Some(DbError::Other("boom".into())),
        }),
        &Hits::default(),
    );
    assert_eq!(a.tree.servers[0].state, ConnState::Disconnected);
    assert_eq!(a.tree.rows().len(), 1);
    assert!(a.status.contains("lost: boom"), "{}", a.status);
}

/// The terminal cursor follows the focused control of the template form and of
/// dialogs, not the editor underneath.
#[test]
fn cursor_follows_the_focused_control() {
    use crate::template_dialog::Stop;
    let (mut a, _d) = app();
    a.new_query_tab(None);
    typing(&mut a, "select 1;\nselect 2;");
    let file = pgc_sql::devtemplate::parse(
        "+++\nname = \"T\"\n[[param]]\nname = \"mode\"\ntype = \"choice\"\noptions = [\"a\", \"b\"]\n[[param]]\nname = \"flag\"\ntype = \"bool\"\n+++\nx\n",
    )
    .unwrap();
    a.template = Some(TemplateForm::new(
        1,
        "T".into(),
        file,
        pgc_sql::devtemplate::TemplateContext {
            version: pgc_core::ServerVersion(160000),
            database: "db".into(),
        },
        4,
    ));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    let mut draw = |a: &App| {
        let mut hits = Hits::default();
        terminal
            .draw(|f| crate::view::render(f, a, &mut hits))
            .unwrap();
        (terminal.get_cursor_position().unwrap(), hits)
    };
    for stop in [Stop::Field(0), Stop::Field(1), Stop::Ok] {
        a.template.as_mut().unwrap().focus = stop;
        let (pos, hits) = draw(&a);
        let r = hits.template.iter().find(|(s, _)| *s == stop).unwrap().1;
        assert!(r.contains(pos), "{stop:?}: cursor {pos:?} not in {r:?}");
    }
    // an open list: on its highlighted row
    a.template.as_mut().unwrap().focus = Stop::Field(0);
    key(&mut a, "Enter");
    let (pos, _) = draw(&a);
    let area = a.template.as_ref().unwrap().popup_area.get().unwrap();
    assert_eq!((pos.x, pos.y), (area.x + 2, area.y + 1));

    // a dialog: on the focused combo's value
    a.template = None;
    a.run(Command::Settings);
    let d = a.dialogs.last_mut().unwrap();
    let combo = d
        .widgets
        .iter()
        .position(|w| matches!(w.kind, crate::dialog::WidgetKind::Combo { .. }))
        .unwrap();
    d.focus = combo;
    let (pos, hits) = draw(&a);
    let (_, areas) = hits.dialog.unwrap();
    assert!(
        areas[combo].contains(pos),
        "cursor {pos:?} not in {:?}",
        areas[combo]
    );
}

/// Draws the app on a test screen and returns its text.
fn screen_text(a: &App) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    let mut hits = Hits::default();
    terminal
        .draw(|f| crate::view::render(f, a, &mut hits))
        .unwrap();
    let buf = terminal.backend().buffer();
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

#[test]
fn ctrl_n_and_tab_status_bar_at_the_bottom() {
    let (mut a, _d) = app();
    // the View menu no longer has the system objects item (D-044)
    key(&mut a, "F9");
    key(&mut a, "Right");
    assert!(!screen_text(&a).contains("system objects"));
    key(&mut a, "F10");
    // Ctrl+N needs a database selected in the tree (D-049)
    a.focus = Focus::Tree;
    key(&mut a, "Ctrl+N");
    assert_eq!(a.tabs.len(), 1);
    assert!(a.status.contains("Select a database"), "{}", a.status);
    a.new_query_tab(None);
    a.new_query_tab(None);
    assert_eq!(a.tabs.len(), 3);
    // the tab's status row is the last row inside its frame, below the results
    // (D-043); single separator lines inside the double frame (D-014, D-019)
    a.query_mut().unwrap().results_visible = true;
    let screen = screen_text(&a);
    let rows: Vec<&str> = screen.lines().collect();
    // rows from the bottom: F-key bar, window status line, frame, tab status
    let bar = rows[rows.len() - 4];
    assert!(bar.contains("║ No database"), "{screen}");
    assert!(bar.contains("Ln 1, Col 1 ║"), "{screen}");
    assert!(rows[rows.len() - 5].contains("║╟───"), "{screen}");
    assert!(rows[rows.len() - 3].contains("╝╚═══"), "{screen}");
    // the tab strip is the first row inside the right frame, a separator below it
    assert!(rows[1].contains("╗╔═══"), "{screen}");
    assert!(rows[2].contains("Query 2 ×"), "{screen}");
    assert!(rows[3].contains("║╟───"), "{screen}");
    assert!(screen.contains(" Results   Messages (1) "), "{screen}");
}

#[test]
fn key_hint_bar_follows_the_focused_frame() {
    let (mut a, _d) = app();
    let texts = |a: &App| -> Vec<String> {
        a.key_hints()
            .iter()
            .map(|h| format!("{} {}", h.key_text(), h.label))
            .collect()
    };
    // tree, nothing selected: no refresh / new query / delete
    a.focus = Focus::Tree;
    let t = texts(&a);
    assert_eq!(t.first().map(String::as_str), Some("F1 Help"), "{t:?}");
    assert!(t.contains(&"F6 Panel".to_owned()), "F6 preferred: {t:?}");
    assert!(t.contains(&"Alt+S Search".to_owned()), "{t:?}");
    assert!(
        !t.iter()
            .any(|x| x.contains("Refresh") || x.contains("Delete")),
        "{t:?}"
    );
    // a server row: F8 deletes the saved connection
    add_server(&mut a, "srv");
    key(&mut a, "Home");
    assert!(texts(&a).contains(&"F8 Delete".to_owned()));
    // query editor: Exec, no Cancel while idle; ^ for Ctrl
    a.new_query_tab(None);
    a.focus = Focus::Tabs;
    let t = texts(&a);
    assert!(t.contains(&"F5 Exec".to_owned()), "{t:?}");
    assert!(t.contains(&"^R Results".to_owned()), "{t:?}");
    assert!(t.contains(&"^W Close tab".to_owned()), "{t:?}");
    assert!(!t.iter().any(|x| x.contains("Cancel")), "{t:?}");
    // results
    a.focus = Focus::Results;
    let t = texts(&a);
    assert!(t.contains(&"Alt+M Messages".to_owned()), "{t:?}");
    assert!(!t.iter().any(|x| x.contains("Fetch all")), "{t:?}");
    // read-only object view
    a.active_tab = 0;
    a.focus = Focus::Tabs;
    let t = texts(&a);
    assert!(t.contains(&"^F Find".to_owned()), "{t:?}");
    assert!(!t.iter().any(|x| x.contains("Exec")), "{t:?}");
    // the bar on screen
    let screen = screen_text(&a);
    let bar = screen.lines().last().unwrap();
    assert!(
        bar.starts_with("F1 Help ^F Find F3 Find next ^C Copy"),
        "{bar}"
    );
    // menu open
    key(&mut a, "F9");
    assert_eq!(texts(&a), ["←→ Menus", "Enter Choose", "Esc Close"]);
    a.menu = None;
    // a dialog: OK / Cancel / Next field / Toggle
    a.open_connection_dialog(None);
    assert_eq!(
        texts(&a),
        ["Enter OK", "Esc Cancel", "Tab Next field", "Space Toggle"]
    );
}

#[test]
fn help_menu_shortcuts_licences_about() {
    let (mut a, _d) = app();
    a.screen = Rect::new(0, 0, 100, 30);
    for cmd in [
        Command::Help,
        Command::KeyboardShortcuts,
        Command::ThirdPartyLicences,
        Command::About,
    ] {
        assert!(a.is_enabled(cmd), "{cmd:?}");
    }
    // F1 in the tree: the shortcuts window at the Object tree section
    a.focus = Focus::Tree;
    key(&mut a, "F1");
    let d = a.dialogs.last().expect("shortcuts window");
    assert_eq!(d.kind, DialogKind::Viewer);
    assert_eq!(d.title, "Keyboard shortcuts");
    let top = d.text_offset("text").unwrap();
    assert!(top > 0, "scrolled to the tree section");
    let screen = screen_text(&a);
    assert!(screen.contains("Object tree"), "{screen}");
    assert!(screen.contains("Refresh the object tree"), "{screen}");
    assert!(a.key_hints().iter().any(|h| h.label == "Scroll"));
    // F1 again does not stack a second window; Down scrolls; Esc closes
    key(&mut a, "F1");
    assert_eq!(a.dialogs.len(), 1);
    key(&mut a, "Down");
    assert_eq!(a.dialogs[0].text_offset("text"), Some(top + 1));
    // (Esc waits for an Esc+digit F-key; F10 closes a dialog at once)
    key(&mut a, "F10");
    assert!(a.dialogs.is_empty());
    // the menu item opens it at the top
    a.run(Command::KeyboardShortcuts);
    assert_eq!(a.dialogs[0].text_offset("text"), Some(0));
    a.dialogs.clear();
    // licences: pgAdmin first, then the crates
    a.run(Command::ThirdPartyLicences);
    let screen = screen_text(&a);
    assert!(screen.contains("Third-party licences"), "{screen}");
    assert!(screen.contains("pgAdmin 4 SQL templates"), "{screen}");
    a.dialogs.clear();
    // about
    a.run(Command::About);
    let screen = screen_text(&a);
    assert!(screen.contains("Postgres terminal commander"), "{screen}");
    assert!(screen.contains(env!("CARGO_PKG_VERSION")), "{screen}");
    assert!(screen.contains("PostgreSQL Licence"), "{screen}");
}

#[test]
fn default_tab_follows_the_tree() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "srv");
    key(&mut a, "Home");
    assert!(a.tabs[0].view.editor.text().contains("-- Server: srv"));
    assert!(a.tabs[0].view.editor.text().contains("disconnected"));
    let id = fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(160004));
    let databases = a.tree.servers[0].children[0].nid;
    a.pending.insert(7, databases);
    a.update(
        Msg::Db(DbEvent::Nodes {
            id,
            key: 7,
            result: Ok(vec![pgc_db::NodeRow {
                oid: 5,
                name: "app".into(),
                label: None,
                partitioned: false,
            }]),
        }),
        &Hits::default(),
    );
    key(&mut a, "Down"); // "Databases (1)" group: info text
    assert!(a.tabs[0].view.editor.text().contains("-- Databases (1)"));
    assert!(a.tabs[0].view.editor.text().contains("--   app"));
    // An object starts the 200 ms timer instead of loading at once.
    let effects = key(&mut a, "Down");
    assert!(matches!(
        effects.as_slice(),
        [Effect::StartDefinitionTimer(_)]
    ));
    assert!(a.tabs[0].view.loading);
    assert_eq!(a.tabs[0].view.title, "app");
    // the tab status says where the object comes from, then read only (D-043)
    let label = a.tab_connection_label(&a.tabs[0]);
    assert!(label.starts_with("srv · "), "{label}");
    assert!(label.ends_with("app · read only"), "{label}");
    // no title in the top frame line
    let screen = screen_text(&a);
    assert!(!screen.contains("═ app ═"), "{screen}");
    assert!(screen.contains("app · read only"), "{screen}");
    // F3 on the group opens it in a new tab: server level, no database
    key(&mut a, "Up");
    key(&mut a, "F3");
    assert_eq!(a.tabs.len(), 2);
    assert_eq!(a.tabs[1].kind, TabKind::Object);
    assert_eq!(a.focus, Focus::Tabs);
    let label = a.tab_connection_label(&a.tabs[1]);
    assert!(
        label.starts_with("srv") && label.ends_with(" · read only"),
        "{label}"
    );
    assert!(!label.contains("app"), "{label}");
}

#[test]
fn stale_definitions_are_dropped_and_views_scroll() {
    let (mut a, _d) = app();
    a.pending_defs.insert(1, 0);
    a.pending_defs.insert(2, 0);
    a.default_key = 2;
    a.definition_loaded(1, Ok("old".into()));
    assert!(a.tabs[0].view.is_empty());
    let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
    a.definition_loaded(2, Ok(text));
    assert!(a.tabs[0].view.editor.text().starts_with("line 0"));
    a.tabs[0].view.editor.set_view(10, 20);
    a.focus = Focus::Tabs;
    a.run(Command::PageDown);
    assert_eq!(a.tabs[0].view.editor.cursor_line_col().0, 9);
    a.run(Command::DocEnd);
    assert_eq!(a.tabs[0].view.editor.top, 41);
    // the object view is read-only
    typing(&mut a, "x");
    assert!(a.status.contains("Read-only"));
}

#[test]
fn query_tab_editing_clipboard_and_find() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    typing(&mut a, "select 1;");
    key(&mut a, "Enter");
    typing(&mut a, "select 2;");
    assert_eq!(a.editor().text(), "select 1;\nselect 2;");
    // Tab indents in the editor; F6 leaves it
    key(&mut a, "Ctrl+A");
    key(&mut a, "Tab");
    assert_eq!(a.editor().text(), "\tselect 1;\n\tselect 2;");
    key(&mut a, "Shift+Tab");
    assert_eq!(a.editor().text(), "select 1;\nselect 2;");
    // copy / paste through the internal clipboard (system clipboard off)
    key(&mut a, "Ctrl+A");
    let effects = key(&mut a, "Ctrl+C");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Osc52(t) if t == "select 1;\nselect 2;")),
        "{effects:?}"
    );
    key(&mut a, "Ctrl+End");
    key(&mut a, "Enter");
    key(&mut a, "Shift+Insert");
    assert_eq!(a.editor().len_lines(), 4);
    key(&mut a, "Ctrl+Z");
    assert_eq!(a.editor().len_lines(), 3);
    // find dialog, then F3 again
    key(&mut a, "Ctrl+Home");
    key(&mut a, "Ctrl+F");
    assert_eq!(a.dialogs[0].kind, DialogKind::Find);
    typing(&mut a, "select");
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.editor().selection(), Some((0, 6)));
    key(&mut a, "F3");
    assert_eq!(a.editor().selection(), Some((10, 16)));
    // replace all from the dialog
    key(&mut a, "Ctrl+F");
    key(&mut a, "Alt+W");
    typing(&mut a, "SELECT");
    key(&mut a, "Alt+A");
    assert_eq!(a.editor().line(0), "SELECT 1;");
    assert!(a.status.contains("occurrence"));
    // D-041: F6 cycles editor → results → tree → editor
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Results);
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Tree);
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Tabs);
}

#[test]
fn psql_window_lists_every_command() {
    let (mut a, _d) = app();
    a.screen = Rect::new(0, 0, 100, 40);
    let d = a.psql_dialog("");
    a.dialogs.push(d);
    let rows = crate::psql::list_rows();
    let screen = screen_text(&a);
    assert!(
        screen.contains("dt         [pattern] list tables"),
        "{screen}"
    );
    assert!(!screen.contains("\\dt"), "no psql backslash: {screen}");
    assert!(screen.contains(" pgtc "), "{screen}");
    assert!(screen.contains("gtd"), "the whole list fits: {screen}");
    // Down: the next command goes into the input (with a space for its argument)
    key(&mut a, "Down");
    assert_eq!(a.dialogs[0].text("cmd"), Some("l "));
    // typing moves the list to the command; the list stays complete
    a.dialogs[0].set_text("cmd", "");
    a.update(Msg::Tick, &Hits::default());
    typing(&mut a, "gtu");
    let gtu = rows
        .iter()
        .position(|r| r.is_some_and(|(c, _, _)| c == "gtu"))
        .unwrap();
    assert_eq!(a.dialogs[0].selected("list"), Some(gtu));
    // a generate command without a table waits for the name
    key(&mut a, "Enter");
    assert_eq!(a.dialogs.len(), 1, "no error dialog");
    assert_eq!(a.dialogs[0].focused_id(), Some("cmd"));
    // the separator before pgtc's commands is skipped
    let sep = rows.iter().position(Option::is_none).unwrap();
    a.dialogs[0].list_select("list", sep - 1);
    a.update(Msg::Tick, &Hits::default());
    key(&mut a, "Down");
    assert_eq!(a.dialogs[0].selected("list"), Some(sep + 1));
    assert_eq!(a.dialogs[0].text("cmd"), Some("gts "));
}

#[test]
fn replace_asks_per_match() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    typing(&mut a, "a a a");
    key(&mut a, "Ctrl+Home");
    key(&mut a, "Ctrl+F");
    typing(&mut a, "a");
    key(&mut a, "Alt+W");
    typing(&mut a, "bb");
    key(&mut a, "Alt+R");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::ReplaceConfirm);
    key(&mut a, "r"); // replace the first
    key(&mut a, "s"); // skip the second
    key(&mut a, "r"); // replace the third; then the search wraps and stops
    assert!(a.dialogs.is_empty());
    assert_eq!(a.editor().text(), "bb a bb");
}

#[test]
fn closing_an_unsaved_query_tab_asks_to_save() {
    let (mut a, dir) = app();
    a.file_dir = dir.path().to_path_buf();
    // unchanged: closes without asking
    a.new_query_tab(None);
    key(&mut a, "Ctrl+W");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
    // Cancel keeps the tab, No closes it
    a.new_query_tab(None);
    typing(&mut a, "select 1;");
    key(&mut a, "Ctrl+W");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::SaveChanges);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 2);
    key(&mut a, "Ctrl+W");
    key(&mut a, "n");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
    // Yes without a file: Save dialog; cancelling it keeps the tab
    a.new_query_tab(None);
    typing(&mut a, "select 2;");
    key(&mut a, "Ctrl+W");
    key(&mut a, "y");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::SaveFile);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 2);
    assert_eq!(a.after_save, None);
    // ... saving through it closes the tab
    key(&mut a, "Ctrl+W");
    key(&mut a, "y");
    key(&mut a, "Enter");
    let saved = dir.path().join("query.sql");
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "select 2;");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
    // Yes with a file: saved there at once, then closed
    a.open_path(saved.clone());
    typing(&mut a, "-- x\n");
    key(&mut a, "Ctrl+W");
    key(&mut a, "y");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "-- x\nselect 2;");
}

#[test]
fn quitting_asks_about_each_unsaved_query_tab() {
    let (mut a, dir) = app();
    a.file_dir = dir.path().to_path_buf();
    for text in ["select 1;", "select 2;"] {
        a.new_query_tab(None);
        typing(&mut a, text);
    }
    a.new_query_tab(None); // unchanged: never asked about
    // Cancel stops quitting
    a.request_quit();
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::QuitSaveChanges);
    assert_eq!(a.active_tab, 1);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert!(!a.should_quit());
    assert_eq!(a.bulk, None);
    // No for the first tab, Yes (save as query.sql) for the second: then quits
    a.request_quit();
    key(&mut a, "n");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::QuitSaveChanges);
    assert_eq!(a.active_tab, 2);
    assert!(!a.should_quit());
    key(&mut a, "y");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::SaveFile);
    key(&mut a, "Enter");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("query.sql")).unwrap(),
        "select 2;"
    );
    assert!(a.should_quit());
}

/// A query tab bound to server `uid` / database `db` (no real connection).
fn bound_tab(a: &mut App, uid: u64, db: &str, text: &str) -> u64 {
    a.tree.select_server(0);
    a.new_query_tab(None);
    let id = a.tabs[a.active_tab].id;
    a.query_mut().unwrap().binding = Some((uid, db.to_owned()));
    if !text.is_empty() {
        typing(a, text);
    }
    id
}

#[test]
fn each_tab_shows_its_own_connection() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "local16");
    a.tree.by_uid(uid).unwrap().conn.username = "postgres".into();
    a.new_query_tab(None);
    let screen = screen_text(&a);
    assert!(
        screen.contains("No database – select one in the tree"),
        "{screen}"
    );
    bound_tab(&mut a, uid, "shop", "select 1;");
    let screen = screen_text(&a);
    assert!(
        screen.contains("local16 · postgres@shop – not connected"),
        "{screen}"
    );
    assert!(screen.contains("Ln 1, Col 10"), "{screen}");
    // the window status line only has messages
    a.status = "Saved x".into();
    let last_rows: Vec<String> = screen_text(&a)
        .lines()
        .rev()
        .take(2)
        .map(str::to_owned)
        .collect();
    assert!(last_rows[1].trim() == "Saved x", "{last_rows:?}");
    // object tabs: where the object comes from, then read only
    a.active_tab = 0;
    assert!(screen_text(&a).contains(" read only"));
}

#[test]
fn disconnect_closes_the_servers_tabs() {
    let (mut a, _d) = app();
    let a_uid = add_server(&mut a, "a");
    let b_uid = add_server(&mut a, "b");
    let unsaved = bound_tab(&mut a, a_uid, "shop", "select 1;");
    let clean = bound_tab(&mut a, a_uid, "crm", "");
    let other = bound_tab(&mut a, b_uid, "shop", "select 2;");
    a.tabs[0].server = Some(a_uid); // the default tab is never closed
    a.tree.select_server(a_uid);
    a.run(Command::ShowInNewTab);
    let object = a.tabs[a.active_tab].id;
    assert_eq!(a.tabs[a.active_tab].server, Some(a_uid));
    assert_eq!(a.tabs.len(), 5);

    // Cancel: nothing is closed
    a.tree.select_server(a_uid);
    a.run(Command::Disconnect);
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.kind, DialogKind::QuitSaveChanges);
    assert_eq!(d.title, "Disconnect");
    assert_eq!(a.tabs[a.active_tab].id, unsaved);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 5);
    assert_eq!(a.bulk, None);

    // No: all tabs of server a close, server b's tab stays
    a.tree.select_server(a_uid);
    a.run(Command::Disconnect);
    key(&mut a, "n");
    assert!(a.dialogs.is_empty());
    let ids: Vec<u64> = a.tabs.iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![0, other]);
    for gone in [unsaved, clean, object] {
        assert!(!ids.contains(&gone));
    }
    assert_eq!(a.status, "Disconnected");

    // without the question (setting off): closes at once
    a.settings.query.ask_save = false;
    a.tree.select_server(b_uid);
    a.run(Command::Disconnect);
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
}

#[test]
fn tab_close_mark() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    a.new_query_tab(None);
    typing(&mut a, "select 1;");
    let hits = |a: &App| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        let mut hits = Hits::default();
        terminal
            .draw(|f| crate::view::render(f, a, &mut hits))
            .unwrap();
        (hits, terminal.backend().buffer().clone())
    };
    let (h, buf) = hits(&a);
    // the default tab has none, query tabs have one
    assert_eq!(
        h.tab_close.iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![1, 2]
    );
    let r = h.tab_close[0].1;
    assert_eq!(buf[(r.x, r.y)].symbol(), "×");
    let click = |a: &mut App, r: Rect, h: &Hits| {
        a.update(
            Msg::Input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: r.x,
                row: r.y,
                modifiers: KeyModifiers::NONE,
            })),
            h,
        );
    };
    // unchanged tab: closes at once
    click(&mut a, r, &h);
    assert_eq!(a.tabs.len(), 2);
    // changed tab: the same question as Ctrl+W
    let (h, _) = hits(&a);
    click(&mut a, h.tab_close[0].1, &h);
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::SaveChanges);
}

#[test]
fn tree_pane_border_is_dragged() {
    let (mut a, _d) = app();
    let draw = |a: &App| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        let mut hits = Hits::default();
        terminal
            .draw(|f| crate::view::render(f, a, &mut hits))
            .unwrap();
        hits
    };
    let h = draw(&a);
    assert_eq!(h.tree.width, 40); // a third
    let mouse = |kind, x| MouseEvent {
        kind,
        column: x,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };
    a.on_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), h.pane_border.x),
        &h,
    );
    a.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 59), &h);
    a.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 59), &h);
    assert_eq!(draw(&a).tree.width, 60);
    let saved = Settings::load(&a.paths.settings_file()).unwrap();
    assert_eq!(saved.ui.tree_width, Some(60));
    // limits: both panes stay usable
    assert_eq!(crate::view::tree_width(Some(5), 120), 20);
    assert_eq!(crate::view::tree_width(Some(115), 120), 90);
    assert_eq!(crate::view::tree_width(None, 30), 20);
}

#[test]
fn ask_save_setting() {
    let (mut a, _d) = app();
    assert!(a.settings.query.ask_save);
    a.run(Command::Settings);
    key(&mut a, "Alt+A"); // toggles the checkbox
    key(&mut a, "Alt+O");
    assert!(
        a.dialogs.is_empty(),
        "{:?}",
        a.dialogs.last().map(|d| d.kind)
    );
    assert!(!a.settings.query.ask_save);
    a.new_query_tab(None);
    typing(&mut a, "select 1;");
    key(&mut a, "Ctrl+W");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
}

/// An app whose export events arrive on `rx`.
fn export_app() -> (
    App,
    tempfile::TempDir,
    std::sync::mpsc::Receiver<ExportEvent>,
) {
    let (mut a, dir) = app();
    let (tx, rx) = std::sync::mpsc::channel();
    a.set_export_sink(Arc::new(move |e| {
        let _ = tx.send(e);
    }));
    a.file_dir = dir.path().to_path_buf();
    (a, dir, rx)
}

/// Feeds export events to the app until no export runs.
fn finish_export(a: &mut App, rx: &std::sync::mpsc::Receiver<ExportEvent>) {
    while a.query().is_some_and(|q| q.export.is_some()) {
        let e = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("export event");
        a.update(Msg::Export(e), &Hits::default());
    }
}

#[test]
fn export_dialog_and_loaded_rows() {
    let (mut a, dir, rx) = export_app();
    a.new_query_tab(None);
    assert!(!a.is_enabled(Command::ExportResult)); // no result yet
    let q = a.query_mut().unwrap();
    let mut rs = crate::grid::ResultSet::new(vec![
        pgc_db::Column {
            name: "id".into(),
            type_oid: 23,
        },
        pgc_db::Column {
            name: "name".into(),
            type_oid: 25,
        },
    ]);
    rs.push_rows(vec![
        vec![Some("1".into()), Some("Čaša".into())],
        vec![Some("2".into()), None],
    ]);
    q.results.push(rs);
    assert!(a.is_enabled(Command::ExportResult));
    // the results context menu offers it
    a.focus = Focus::Results;
    a.run(Command::ContextMenu);
    assert!(a.popup.is_some());
    a.popup = None;

    a.run(Command::ExportResult);
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.kind, DialogKind::ExportFile);
    assert_eq!(d.text("path"), Some("result_1.csv"));
    // choose JSON: the file name follows
    key(&mut a, "Alt+M");
    key(&mut a, "Space");
    key(&mut a, "Down");
    key(&mut a, "Enter");
    assert_eq!(
        a.dialogs.last().unwrap().text("path"),
        Some("result_1.json")
    );
    key(&mut a, "Alt+O");
    assert!(a.dialogs.is_empty());
    finish_export(&mut a, &rx);
    let out = std::fs::read_to_string(dir.path().join("result_1.json")).unwrap();
    assert_eq!(
        out,
        "[\n  {\"id\": 1, \"name\": \"Čaša\"},\n  {\"id\": 2, \"name\": null}\n]\n"
    );
    assert_eq!(a.settings.export.format, ExportFormat::Json);
    assert!(a.status.starts_with("Exported 2 rows"), "{}", a.status);
    // the grid result was complete: no "rest exported" note
    assert_eq!(a.query().unwrap().results[0].exported_total, None);

    // again: remembered format; the existing file needs confirmation
    a.run(Command::ExportResult);
    assert_eq!(
        a.dialogs.last().unwrap().text("path"),
        Some("result_1.json")
    );
    key(&mut a, "Alt+O");
    assert_eq!(
        a.dialogs.last().unwrap().kind,
        DialogKind::ConfirmExportOverwrite
    );
    key(&mut a, "n");
    assert!(a.dialogs.is_empty());
    assert!(a.query().unwrap().export.is_none());
}

#[test]
fn split_is_dragged_by_the_results_bar() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    a.query_mut().unwrap().results_visible = true;
    let draw = |a: &App| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        let mut hits = Hits::default();
        terminal
            .draw(|f| crate::view::render(f, a, &mut hits))
            .unwrap();
        hits
    };
    let hits = draw(&a);
    assert_eq!(hits.split.height, 1);
    let mouse = |kind, y| MouseEvent {
        kind,
        column: hits.split.x + 60,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    let target = hits.split.y - 6;
    a.on_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), hits.split.y),
        &hits,
    );
    a.on_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), target),
        &hits,
    );
    a.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), target), &hits);
    assert_eq!(draw(&a).split.y, target);
    // after release, dragging elsewhere does not move it
    a.on_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), target + 3),
        &hits,
    );
    assert_eq!(draw(&a).split.y, target);
}

#[test]
fn save_and_open_sql_files_through_the_dialog() {
    let (mut a, dir) = app();
    let files = dir.path().join("sql");
    std::fs::create_dir_all(files.join("sub")).unwrap();
    a.file_dir = files.clone();
    a.new_query_tab(None); // unbound (no server): still editable
    typing(&mut a, "select 'čšž';");
    // Save: the name field starts with query.sql
    a.run(Command::SaveFile);
    assert_eq!(a.dialogs[0].kind, DialogKind::SaveFile);
    key(&mut a, "Enter");
    let saved = files.join("query.sql");
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "select 'čšž';");
    assert!(a.tabs[a.active_tab].label().starts_with("query.sql"));
    // Save again: asks before overwriting
    a.run(Command::SaveFile);
    key(&mut a, "Enter");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::ConfirmOverwrite);
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
    // Open: the list shows ../, sub/ and query.sql (highlighted); pick the file
    a.open_file_dialog(false);
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.focused_id(), Some("files"));
    key(&mut a, "Down"); // sub/
    key(&mut a, "Enter"); // into sub/
    assert_eq!(a.file_dir, files.join("sub"));
    key(&mut a, "Enter"); // ../
    assert_eq!(a.file_dir, files);
    key(&mut a, "End"); // query.sql
    key(&mut a, "Enter");
    assert!(
        a.dialogs.is_empty(),
        "{:?}",
        a.dialogs.last().map(|d| d.kind)
    );
    assert_eq!(a.editor().text(), "select 'čšž';");
    assert_eq!(a.tabs.len(), 3);
}

#[test]
fn query_tab_layout_draws_results_and_messages() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    typing(&mut a, "select 1");
    let q = a.query_mut().unwrap();
    q.start_run(
        1,
        Sent {
            offset: 0,
            plpgsql_lines: None,
        },
    );
    let none = |_: usize| 0;
    let lines = LineMap {
        line_of: &none,
        line_start: &none,
    };
    q.apply(
        pgc_db::QueryEvent::ResultStart {
            columns: vec![pgc_db::Column {
                name: "answer".into(),
                type_oid: 23,
            }],
            title: None,
        },
        &lines,
    );
    q.apply(
        pgc_db::QueryEvent::Rows {
            rows: vec![vec![Some("42".into())], vec![None]],
            more: true,
        },
        &lines,
    );
    let text = screen_text(&a);
    assert!(text.contains("Results"), "{text}");
    assert!(text.contains("1: 2+ rows"), "{text}");
    assert!(text.contains("answer"), "{text}");
    assert!(text.contains("[null]"), "{text}");
    assert!(text.contains("2 rows loaded – more available"), "{text}");
    assert!(
        text.contains("F5 Exec "),
        "F5 in the editor's key-hint bar: {text}"
    );
    key(&mut a, "Alt+M");
    let text = screen_text(&a);
    assert!(text.contains("Messages (0)"), "{text}");
}

/// The text cursor is drawn in reverse video too, so it is visible in every
/// colour scheme (the terminal's cursor colour may equal the background).
#[test]
fn editor_cursor_cell_is_reversed() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    typing(&mut a, "abc");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    let mut hits = Hits::default();
    terminal
        .draw(|f| crate::view::render(f, &a, &mut hits))
        .unwrap();
    let editor = hits.editor;
    let gutter = pgc_editor::gutter_width(&a.tabs[a.active_tab].view.editor, true);
    let buf = terminal.backend().buffer();
    let cursor = &buf[(editor.x + gutter + 3, editor.y)];
    assert!(cursor.modifier.contains(ratatui::style::Modifier::REVERSED));
    let text = &buf[(editor.x + gutter, editor.y)];
    assert!(!text.modifier.contains(ratatui::style::Modifier::REVERSED));
}

#[test]
fn closing_an_unconnected_tab_does_not_ask() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    a.query_mut().unwrap().tx = TxStatus::InTransaction;
    // not connected: closing does not ask (nothing to commit)
    key(&mut a, "Ctrl+W");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
}

#[test]
fn toggle_system_objects_is_saved() {
    let (mut a, _d) = app();
    a.run(Command::Settings);
    key(&mut a, "Alt+Y");
    key(&mut a, "Alt+O");
    assert!(a.dialogs.is_empty());
    assert!(a.settings.ui.show_system_objects);
    let saved = Settings::load(&a.paths.settings_file()).unwrap();
    assert!(saved.ui.show_system_objects);
}

#[test]
fn unverified_version_warns() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "new");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(190001));
    assert!(a.status.contains("not yet verified"), "{}", a.status);
}

#[test]
fn password_required_opens_login_and_auth_failure_reopens_it() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "srv");
    let id = fake_connecting(&mut a, uid);
    a.update(
        Msg::Db(DbEvent::ConnectFailed {
            id,
            error: DbError::PasswordRequired,
        }),
        &Hits::default(),
    );
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::Login);
    a.dialogs.clear();
    let id = fake_connecting(&mut a, uid);
    a.update(
        Msg::Db(DbEvent::ConnectFailed {
            id,
            error: DbError::AuthFailed("FATAL: password authentication failed".into()),
        }),
        &Hits::default(),
    );
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.kind, DialogKind::Login);
    assert_eq!(d.tag, uid);
    assert_eq!(a.tree.servers[0].state, ConnState::Disconnected);
}

#[test]
fn connection_dialog_validates_and_edits() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "old");
    a.run(Command::EditConnection);
    assert_eq!(a.dialogs[0].kind, DialogKind::Connection);
    key(&mut a, "Alt+P");
    typing(&mut a, "x");
    key(&mut a, "Enter");
    assert_eq!(
        a.dialogs.len(),
        2,
        "invalid port: error on top of the dialog"
    );
    key(&mut a, "Enter"); // close the error
    key(&mut a, "Backspace");
    key(&mut a, "Alt+N");
    key(&mut a, "End");
    typing(&mut a, "2");
    key(&mut a, "Alt+L");
    key(&mut a, "Right"); // prefer → require
    key(&mut a, "Enter"); // on a combo: opens its list
    key(&mut a, "Enter"); // keeps "require"
    key(&mut a, "Tab");
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
    let node = a.tree.by_uid(uid).unwrap();
    assert_eq!(node.conn.name, "old2");
    assert_eq!(node.conn.ssl_mode, SslMode::Require);
    let saved = Settings::load(&a.paths.settings_file()).unwrap();
    assert_eq!(saved.servers[0].name, "old2");
}

#[test]
fn delete_connection_with_confirmation() {
    let (mut a, _d) = app();
    add_server(&mut a, "a");
    add_server(&mut a, "b");
    key(&mut a, "Down");
    key(&mut a, "F8");
    assert_eq!(a.dialogs[0].kind, DialogKind::ConfirmDelete);
    key(&mut a, "n");
    assert_eq!(a.tree.servers.len(), 2);
    key(&mut a, "F8");
    key(&mut a, "Enter");
    assert_eq!(a.tree.servers.len(), 1);
    assert_eq!(a.tree.servers[0].conn.name, "a");
    let saved = Settings::load(&a.paths.settings_file()).unwrap();
    assert_eq!(saved.servers.len(), 1);
}

#[test]
fn deleting_a_connection_closes_its_tabs_like_disconnect() {
    let (mut a, _d) = app();
    let a_uid = add_server(&mut a, "a");
    let b_uid = add_server(&mut a, "b");
    let unsaved = bound_tab(&mut a, a_uid, "shop", "select 1;");
    let other = bound_tab(&mut a, b_uid, "shop", "");
    a.tree.select_server(a_uid);
    a.run(Command::ShowInNewTab);
    assert_eq!(a.tabs.len(), 4);

    // the questions of disconnecting come first; Cancel keeps everything
    a.tree.select_server(a_uid);
    a.run(Command::DeleteConnection);
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::ConfirmDelete);
    key(&mut a, "Enter");
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.kind, DialogKind::QuitSaveChanges);
    assert_eq!(d.title, "Delete connection");
    assert_eq!(a.tabs[a.active_tab].id, unsaved);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 4);
    assert_eq!(a.tree.servers.len(), 2);
    assert_eq!(a.bulk, None);

    // No (do not save): the server's tabs close, then it leaves the tree
    a.tree.select_server(a_uid);
    a.run(Command::DeleteConnection);
    key(&mut a, "Enter");
    key(&mut a, "n");
    assert!(a.dialogs.is_empty());
    let ids: Vec<u64> = a.tabs.iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![0, other]);
    assert_eq!(a.tree.servers.len(), 1);
    assert_eq!(a.tree.servers[0].uid, b_uid);
    assert_eq!(a.status, "Connection \"a\" deleted");
    let saved = Settings::load(&a.paths.settings_file()).unwrap();
    assert_eq!(saved.servers.len(), 1);
}

#[test]
fn query_tab_lookups_outlive_the_template_form_and_end_with_the_connection() {
    use crate::completion::Need;
    let (mut a, _d) = app();
    a.new_query_tab(None);
    let tab = a.tabs[a.active_tab].id;
    let conn = 77;
    a.connections.insert(conn, Target::Query(tab));
    let names = |tab, generation, need| LookupTarget::Names {
        tab,
        generation,
        need,
    };
    // the tab waits for its names while a template form closes
    a.query_mut().unwrap().names.objects_pending = true;
    a.template_lookups.insert(1, names(tab, 0, Need::Objects));
    a.template_lookups.insert(
        2,
        LookupTarget::Field("schema".into(), devtemplate::Shape::Name),
    );
    a.form_event(FormEvent::Cancel);
    let keys: Vec<u64> = a.template_lookups.keys().copied().collect();
    assert_eq!(keys, vec![1], "only the form's own lookup is dropped");
    let row = ["", "public", "schema", "t"].map(str::to_owned).to_vec();
    a.update(
        Msg::Db(DbEvent::Lookup {
            id: conn,
            key: 1,
            result: Ok(vec![row]),
        }),
        &Hits::default(),
    );
    let q = a.query().unwrap();
    assert!(!q.names.objects_pending);
    assert_eq!(q.names.objects.as_ref().map(Vec::len), Some(1));

    // a lost connection: nothing stays "pending", the names are read again
    let orders = (None, "orders".to_owned());
    let q = a.query_mut().unwrap();
    q.names.columns_pending.insert(orders.clone());
    let generation = q.names.generation;
    a.template_lookups
        .insert(3, names(tab, generation, Need::Columns(orders)));
    a.update(
        Msg::Db(DbEvent::Closed {
            id: conn,
            error: Some(DbError::Other("gone".into())),
        }),
        &Hits::default(),
    );
    let q = a.query().unwrap();
    assert!(q.names.columns_pending.is_empty());
    assert!(q.names.objects.is_none());
    assert!(q.names.generation > generation);
    assert!(a.template_lookups.is_empty());
}

#[tokio::test]
async fn no_lookups_while_a_result_is_running_or_paused() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "a");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    // a connected query tab (no real connection behind it) with a running query
    a.new_query_tab(None);
    let q = a.query_mut().unwrap();
    q.binding = Some((uid, "shop".into()));
    q.conn = Some(QueryConn {
        id: 99,
        database: "shop".into(),
        handle: pgc_db::ConnectionHandle::detached(99),
        cancel: None,
        state: QueryConnState::Connected,
        ssl_mode: SslMode::default(),
        ssl_root_cert: None,
    });
    q.start_run(
        1,
        Sent {
            offset: 0,
            plpgsql_lines: None,
        },
    );
    let gts = |a: &mut App| {
        let d = a.psql_dialog("gts orders");
        a.run_psql(d, "gts orders");
    };

    // typing (the list opens by itself) and Ctrl+Space read no names
    typing(&mut a, "select * from ord");
    a.run(Command::Complete);
    assert!(a.template_lookups.is_empty());
    assert!(!a.query().unwrap().names.objects_pending);
    assert_eq!(a.status, NAMES_HELD);

    // the same while the result is paused; a generate command is refused
    a.query_mut().unwrap().running.as_mut().unwrap().paused = true;
    a.run(Command::Complete);
    gts(&mut a);
    assert!(a.template_lookups.is_empty());
    let d = a.dialogs.pop().unwrap();
    assert!(d.error, "{}", d.title);

    // the result has ended: names and the table are read
    a.query_mut().unwrap().running = None;
    a.run(Command::Complete);
    assert!(a.query().unwrap().names.objects_pending);
    assert_eq!(a.template_lookups.len(), 1);
    gts(&mut a);
    assert!(a.dialogs.is_empty());
    assert_eq!(a.template_lookups.len(), 2);
}

#[test]
fn context_menu_runs_commands() {
    let (mut a, _d) = app();
    add_server(&mut a, "a");
    key(&mut a, "F2");
    assert!(a.popup.is_some());
    key(&mut a, "e"); // Edit connection…
    assert!(a.popup.is_none());
    assert_eq!(a.dialogs[0].kind, DialogKind::Connection);
}

/// Ctrl+P switches PL/pgSQL mode of the active query tab, from the editor and from
/// the results (D-020).
#[test]
fn ctrl_p_toggles_plpgsql_mode_of_the_tab() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    assert!(!a.query().unwrap().plpgsql);
    key(&mut a, "Ctrl+P");
    assert!(a.query().unwrap().plpgsql);
    assert_eq!(a.status, "PL/pgSQL mode on");
    assert!(
        !a.editor().text().contains('p'),
        "not typed into the editor"
    );
    a.focus = Focus::Results;
    key(&mut a, "Ctrl+P");
    assert!(!a.query().unwrap().plpgsql);
    // another tab keeps its own mode
    a.focus = Focus::Tabs;
    key(&mut a, "Ctrl+P");
    a.new_query_tab(None);
    assert!(!a.query().unwrap().plpgsql);
}

/// Autocomplete in the template form's `sql` box (D-052): names of the query tab, the
/// list over the form, Enter inserts into the box, Esc closes only the list.
#[tokio::test]
async fn autocomplete_in_the_template_sql_box() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "a");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    a.new_query_tab(None);
    let tab = a.tabs[a.active_tab].id;
    let q = a.query_mut().unwrap();
    q.binding = Some((uid, "shop".into()));
    q.conn = Some(QueryConn {
        id: 99,
        database: "shop".into(),
        handle: pgc_db::ConnectionHandle::detached(99),
        cancel: None,
        state: QueryConnState::Connected,
        ssl_mode: SslMode::default(),
        ssl_root_cert: None,
    });
    q.names.set_objects(vec![vec![
        "public".into(),
        "orders".into(),
        "table".into(),
        "t".into(),
    ]]);
    let file = devtemplate::parse(
        "+++\nname = \"F\"\n[[param]]\nname = \"body\"\ntype = \"sql\"\nfill = true\n+++\n{{ body }}\n",
    )
    .unwrap();
    a.template = Some(TemplateForm::new(
        tab,
        "F".into(),
        file,
        devtemplate::TemplateContext {
            version: pgc_core::ServerVersion(170002),
            database: "shop".into(),
        },
        4,
    ));
    assert!(a.completion_in_form());
    typing(&mut a, "select * from ord");
    let p = a.completion.as_ref().expect("the list opened");
    assert!(p.in_form && p.shown(), "{p:?}");
    // drawn over the form
    let screen = screen_text(&a);
    assert!(screen.contains("orders"), "{screen}");
    // Enter takes the entry into the box (not a new line), the preview follows
    key(&mut a, "Enter");
    assert!(a.completion.is_none());
    let form = a.template.as_ref().unwrap();
    assert_eq!(form.sql_editor().unwrap().text(), "select * from orders");
    assert!(form.preview.text().contains("from orders"));
    assert!(
        a.tabs[a.active_tab].view.editor.text().is_empty(),
        "not the tab's editor"
    );
    // Esc closes only the list; Ctrl+Space opens it again
    typing(&mut a, " o");
    key(&mut a, "Ctrl+Space");
    assert!(a.completion.is_some());
    key(&mut a, "Esc");
    assert!(a.completion.is_none());
    assert!(a.template.is_some(), "the form stays open");
}

/// Ctrl+E (D-049): the definition of the read-only view in a new query tab on the same
/// server and database, as unsaved text; refused without a definition, a database or
/// a connected server; nothing in a query tab.
#[tokio::test]
async fn ctrl_e_edits_the_definition_in_a_new_query_tab() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "a");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    let show = |a: &mut App, text: &str, database: Option<&str>| {
        a.tabs[0].server = Some(uid);
        a.tabs[0].database = database.map(str::to_owned);
        a.next_key += 1;
        a.pending_defs.insert(a.next_key, 0);
        a.default_key = a.next_key;
        a.definition_loaded(a.next_key, Ok(text.to_owned()));
        a.active_tab = 0;
        a.focus = Focus::Tabs;
    };
    let def = "CREATE OR REPLACE FUNCTION public.f() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql;\n";
    show(&mut a, def, Some("shop"));
    assert!(a.is_enabled(Command::EditInQueryTab));
    key(&mut a, "Ctrl+E");
    assert_eq!(a.tabs.len(), 2);
    let tab = &a.tabs[a.active_tab];
    assert!(matches!(tab.kind, TabKind::Query(_)));
    assert_eq!(tab.view.editor.text(), def);
    assert!(tab.view.editor.modified(), "unsaved text");
    assert!(!tab.view.editor.read_only());
    assert_eq!(
        tab.query.as_ref().unwrap().binding,
        Some((uid, "shop".to_owned()))
    );
    // in a query tab Ctrl+E does nothing
    a.run(Command::EditInQueryTab);
    assert_eq!(a.tabs.len(), 2);

    // from the tree: the default tab's definition
    show(&mut a, def, Some("shop"));
    a.focus = Focus::Tree;
    a.run(Command::EditInQueryTab);
    assert_eq!(a.tabs.len(), 3);

    // refused: still loading, an info text, no database, server not connected
    show(&mut a, def, Some("shop"));
    a.tabs[0].view.loading = true;
    a.run(Command::EditInQueryTab);
    assert_eq!(a.status, "The definition is still loading");
    a.tabs[0]
        .view
        .set_text("Tables".into(), "-- Tables (2)\n".into());
    a.run(Command::EditInQueryTab);
    assert!(a.status.starts_with("Select an object"), "{}", a.status);
    show(&mut a, "CREATE ROLE r;\n", None);
    a.run(Command::EditInQueryTab);
    assert!(a.status.contains("no database"), "{}", a.status);
    show(&mut a, def, Some("shop"));
    a.tree.servers[0].state = ConnState::Disconnected;
    assert!(!a.is_enabled(Command::EditInQueryTab));
    a.run(Command::EditInQueryTab);
    assert_eq!(a.status, "Connect the server first");
    assert_eq!(a.tabs.len(), 3);
}
