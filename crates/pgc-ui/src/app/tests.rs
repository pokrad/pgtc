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
use crate::theme::Role;
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
        if let Effect::Timer(Timer::Esc(n)) = e {
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
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::ConfirmTemplatesOverwrite { .. }
    ));
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
    assert_eq!(key(&mut a, "Esc"), vec![Effect::Timer(Timer::Esc(1))]);
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
    assert_eq!(a.tabs.len(), 2);
    assert!(matches!(a.tabs[0].body, TabBody::Default));
    assert!(matches!(a.tabs[1].body, TabBody::Query(1, _)));
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
    key(&mut a, "Space"); // the colour scheme combo has the focus: open it
    key(&mut a, "Down"); // Dark
    key(&mut a, "Enter"); // pick it
    key(&mut a, "Alt+T");
    key(&mut a, "Backspace");
    key(&mut a, "Backspace");
    typing(&mut a, "30");
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
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
        text.lines()
            .nth(2)
            .is_some_and(|l| l.contains("Connections")),
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
    let q = a.tabs[a.active_tab].query().unwrap();
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
    assert!(matches!(a.dialogs[0].kind, DialogKind::Connection { .. }));
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
    a.templates.form = Some(TemplateForm::new(
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
        a.templates.form.as_mut().unwrap().focus = stop;
        let (pos, hits) = draw(&a);
        let r = hits.template.iter().find(|(s, _)| *s == stop).unwrap().1;
        assert!(r.contains(pos), "{stop:?}: cursor {pos:?} not in {r:?}");
    }
    // an open list: on its highlighted row
    a.templates.form.as_mut().unwrap().focus = Stop::Field(0);
    key(&mut a, "Enter");
    let (pos, _) = draw(&a);
    let area = a.templates.form.as_ref().unwrap().popup_area.get().unwrap();
    assert_eq!((pos.x, pos.y), (area.x + 2, area.y + 1));

    // a dialog: on the focused combo's value
    a.templates.form = None;
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
    screen_text_sized(a, 100, 30)
}

/// Draws the app on a test screen of the given size and returns its text.
fn screen_text_sized(a: &App, width: u16, height: u16) -> String {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
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
    // rows from the bottom: window status line, frame, tab status
    let bar = rows[rows.len() - 3];
    assert!(bar.contains("║ No database"), "{screen}");
    assert!(bar.contains("Ln 1, Col 1 ║"), "{screen}");
    assert!(rows[rows.len() - 4].contains("║╟───"), "{screen}");
    assert!(rows[rows.len() - 2].contains("╝╚═══"), "{screen}");
    // the tab strip is the first row inside the right frame, a separator below it
    assert!(rows[1].contains("╗╔═══"), "{screen}");
    assert!(rows[2].contains("Query 2 ×"), "{screen}");
    assert!(rows[3].contains("╢╟───"), "{screen}");
    // only Messages without a result set (D-066)
    assert!(screen.contains(" Messages (1) "), "{screen}");
    assert!(!screen.contains(" Results "), "{screen}");
    // the tree has the same look: title strip, separator, rows (D-014)
    assert!(rows[2].starts_with("║") && rows[2].contains("Connections"));
    assert!(rows[3].starts_with("╟───"), "{screen}");
}

#[test]
fn active_tabs_are_coloured_only_in_the_focused_frame() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    a.query_mut().unwrap().results_visible = true;
    let style_at = |a: &App, text: &str, row: u16| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|f| crate::view::render(f, a, &mut Hits::default()))
            .unwrap();
        let buf = terminal.backend().buffer();
        let line: String = (0..100).map(|x| buf[(x, row)].symbol()).collect();
        let x = line.find(text).map(|b| line[..b].chars().count()).unwrap();
        let cell = &buf[(x as u16, row)];
        (cell.fg, cell.bg)
    };
    let theme = |r: Role| {
        let s = a.theme.get(r);
        (s.fg.unwrap(), s.bg.unwrap())
    };
    let (active, unfocused) = (theme(Role::TabsActive), theme(Role::TabsActiveUnfocused));
    // the editor has the focus: the query tab, not the tree title nor Results
    a.focus = Focus::Tabs;
    assert_eq!(style_at(&a, "Query 1", 2), active);
    assert_eq!(style_at(&a, "Connections", 2), unfocused);
    let results_row = (0..30)
        .find(|&y| {
            screen_text(&a)
                .lines()
                .nth(y)
                .unwrap()
                .contains(" Messages (")
        })
        .unwrap() as u16;
    assert_eq!(style_at(&a, "Messages", results_row), unfocused);
    // the results have the focus: the query tab and Results
    a.focus = Focus::Results;
    assert_eq!(style_at(&a, "Query 1", 2), active);
    assert_eq!(style_at(&a, "Messages", results_row), active);
    // the tree has the focus: only its title
    a.focus = Focus::Tree;
    assert_eq!(style_at(&a, "Connections", 2), active);
    assert_eq!(style_at(&a, "Query 1", 2), unfocused);
}

/// The text of the `F1` window for the current state.
fn f1_text(a: &App) -> String {
    a.key_help_lines().lines.join("\n")
}

/// `F1` lists only the keys that do something now (D-063).
#[test]
fn f1_lists_only_the_keys_that_work_now() {
    let (mut a, _d) = app();
    // tree without connections: none of its keys work, the global ones do
    a.focus = Focus::Tree;
    let t = f1_text(&a);
    assert!(!t.contains("Object tree"), "{t}");
    assert!(t.contains("Everywhere\n"), "{t}");
    assert!(t.contains("Open the menu bar"), "{t}");
    assert!(t.contains("Quit pgtc"), "{t}");
    assert!(!t.contains("New query tab"), "no database selected: {t}");
    assert!(!t.contains("Next tab"), "one tab only: {t}");
    assert!(t.ends_with("All keys: Help → Keyboard shortcuts"), "{t}");
    // a server row (not connected): F8 deletes it; nothing to refresh
    add_server(&mut a, "srv");
    key(&mut a, "Home");
    let t = f1_text(&a);
    assert!(t.starts_with("Object tree\n"), "{t}");
    assert!(t.contains("Delete the saved connection"), "{t}");
    assert!(t.contains("Quick search"), "{t}");
    assert!(!t.contains("Refresh the object tree"), "{t}");
    assert!(!t.contains("Focus the object tree"), "already there: {t}");
    // query editor: Execute, no Cancel while idle; editing keys
    a.new_query_tab(None);
    a.focus = Focus::Tabs;
    let t = f1_text(&a);
    assert!(t.starts_with("Query editor\n"), "{t}");
    assert!(
        t.lines()
            .any(|l| l.starts_with("    F5 ") && l.contains("Execute"))
    );
    assert!(t.contains("Ctrl+V, Shift+Insert"), "{t}");
    // in groups (D-065): actions first, the file, then moving and editing
    let pos = |s: &str| t.find(s).unwrap_or_else(|| panic!("{s}: {t}"));
    assert!(
        pos("  Actions
") < pos("  File
"),
        "{t}"
    );
    assert!(
        pos("  File
") < pos("  Moving
"),
        "{t}"
    );
    assert!(
        pos("Save SQL file")
            < pos("  Moving
"),
        "{t}"
    );
    assert!(
        pos("  Editing
") < pos("Delete the line"),
        "{t}"
    );
    assert!(!t.contains("Cancel the running query"), "{t}");
    assert!(!t.contains("Fetch all rows"), "{t}");
    assert!(t.contains("Close the tab"), "{t}");
    assert!(t.contains("Focus the object tree"), "{t}");
    // the context key hides the global one: Ctrl+R is the results here
    assert!(!t.contains("Refresh the object tree"), "{t}");
    // results without a result set: Messages (D-066)
    a.focus = Focus::Results;
    let t = f1_text(&a);
    assert!(t.starts_with("Messages\n"), "{t}");
    assert!(t.contains("Switch between results and messages"), "{t}");
    assert!(!t.contains("Paste"), "{t}");
    // read-only object view: no editing keys, nothing to execute
    a.active_tab = 0;
    a.focus = Focus::Tabs;
    let t = f1_text(&a);
    assert!(t.starts_with("Object view\n"), "{t}");
    assert!(t.contains("Find / replace"), "{t}");
    assert!(t.contains("Ctrl+C, Ctrl+Insert"), "{t}");
    assert!(!t.contains("Paste"), "{t}");
    assert!(!t.contains("Execute"), "{t}");
    assert!(!t.contains("Close the tab"), "the default tab: {t}");
    // a dialog: its keys and F1, not the menu or Quit
    a.open_connection_dialog(None);
    let t = f1_text(&a);
    assert!(t.starts_with("Dialogs and lists\n"), "{t}");
    assert!(t.contains("Toggle the checkbox"), "{t}");
    assert!(
        t.contains("Dialog field with that underlined letter"),
        "{t}"
    );
    assert!(!t.contains("Quit pgtc"), "{t}");
    assert!(!t.contains("Open the menu bar"), "{t}");
}

/// The window status line is the coloured bottom row of the screen (D-014).
#[test]
fn status_line_is_the_bottom_row() {
    let (mut a, _d) = app();
    a.status = "Saved x".into();
    let screen = screen_text(&a);
    let rows: Vec<&str> = screen.lines().collect();
    assert!(rows[29].starts_with(" Saved x"), "{screen}");
    assert!(
        rows[28].starts_with("╚═"),
        "the windows end above it: {screen}"
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| crate::view::render(f, &a, &mut Hits::default()))
        .unwrap();
    let cell = &terminal.backend().buffer()[(1, 29)];
    let want = a.theme.get(Role::StatusWindow);
    assert_eq!((Some(cell.fg), Some(cell.bg)), (want.fg, want.bg));
}

#[test]
fn help_menu_shortcuts_licences_about() {
    let (mut a, _d) = app();
    a.frame.screen = Rect::new(0, 0, 100, 30);
    for cmd in [
        Command::Help,
        Command::KeyboardShortcuts,
        Command::ThirdPartyLicences,
        Command::About,
    ] {
        assert!(a.is_enabled(cmd), "{cmd:?}");
    }
    // F1 in the tree: only the keys that work now (D-063)
    a.focus = Focus::Tree;
    key(&mut a, "F1");
    let d = a.dialogs.last().expect("keys window");
    assert_eq!(d.kind, DialogKind::Viewer);
    assert_eq!(d.title, "Keys available now");
    let screen = screen_text(&a);
    assert!(screen.contains("Everywhere"), "{screen}");
    assert!(screen.contains("Quit pgtc"), "{screen}");
    assert!(!screen.contains("Refresh the object tree"), "{screen}");
    // F1 again does not stack a second window
    key(&mut a, "F1");
    assert_eq!(a.dialogs.len(), 1);
    // (Esc waits for an Esc+digit F-key; F10 closes a dialog at once)
    key(&mut a, "F10");
    assert!(a.dialogs.is_empty());
    // the menu item lists every key; Down scrolls
    a.run(Command::KeyboardShortcuts);
    assert_eq!(a.dialogs[0].title, "Keyboard shortcuts");
    assert_eq!(a.dialogs[0].text_offset("text"), Some(0));
    let screen = screen_text(&a);
    assert!(screen.contains("Everywhere"), "{screen}");
    assert!(screen.contains("Open the menu bar"), "{screen}");
    key(&mut a, "Down");
    assert_eq!(a.dialogs[0].text_offset("text"), Some(1));
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
        [Effect::Timer(Timer::Definition(_))]
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
    assert!(matches!(a.tabs[1].body, TabBody::Object));
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
    // D-060: F6 only switches tree ↔ right frame, back to the part last used there
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Tree);
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Tabs);
    key(&mut a, "Alt+Down");
    assert_eq!(a.focus, Focus::Results);
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Tree);
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Results, "the results were used last");
}

#[test]
fn psql_window_lists_every_command() {
    let (mut a, _d) = app();
    a.frame.screen = Rect::new(0, 0, 100, 40);
    let d = a.psql_dialog("");
    a.dialogs.push(d);
    let rows = crate::psql::list_rows();
    let screen = screen_text_sized(&a, 100, 40);
    assert!(
        screen.contains("dt         [pattern] list tables"),
        "{screen}"
    );
    assert!(!screen.contains("\\dt"), "no psql backslash: {screen}");
    assert!(screen.contains(" pgtc "), "{screen}");
    assert!(screen.contains("gtt"), "the whole list fits: {screen}");
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
    a.files.dir = dir.path().to_path_buf();
    // unchanged: closes without asking
    a.new_query_tab(None);
    key(&mut a, "Ctrl+W");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 1);
    // Cancel keeps the tab, No closes it
    a.new_query_tab(None);
    typing(&mut a, "select 1;");
    key(&mut a, "Ctrl+W");
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::SaveChanges { .. }
    ));
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
    assert_eq!(a.closing.after_save, None);
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
    a.files.dir = dir.path().to_path_buf();
    for text in ["select 1;", "select 2;"] {
        a.new_query_tab(None);
        typing(&mut a, text);
    }
    a.new_query_tab(None); // unchanged: never asked about
    // Cancel stops quitting
    a.request_quit();
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::QuitSaveChanges { .. }
    ));
    assert_eq!(a.active_tab, 1);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert!(!a.should_quit());
    assert_eq!(a.closing.bulk, None);
    // No for the first tab, Yes (save as query.sql) for the second: then quits
    a.request_quit();
    key(&mut a, "n");
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::QuitSaveChanges { .. }
    ));
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

/// Quitting with a changed tab that has a file: Yes saves to that file, no dialog.
#[test]
fn quitting_saves_a_tab_with_a_file_without_asking_for_a_name() {
    let (mut a, dir) = app();
    let path = dir.path().join("known.sql");
    std::fs::write(&path, "select 1;").unwrap();
    a.open_path(path.clone());
    typing(&mut a, " -- edited");
    key(&mut a, "F10");
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::QuitSaveChanges { .. }
    ));
    key(&mut a, "y");
    assert!(
        a.dialogs.is_empty(),
        "{:?}",
        a.dialogs.iter().map(|d| &d.kind).collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        " -- editedselect 1;"
    );
    assert!(a.should_quit());

    // the same for a tab saved with Ctrl+S (name asked once), then edited
    let (mut a, dir) = app();
    a.files.dir = dir.path().to_path_buf();
    a.new_query_tab(None);
    typing(&mut a, "select 2;");
    key(&mut a, "Ctrl+S");
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
    typing(&mut a, " -- edited");
    key(&mut a, "F10");
    key(&mut a, "y");
    assert!(
        a.dialogs.is_empty(),
        "{:?}",
        a.dialogs.iter().map(|d| &d.kind).collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("query.sql")).unwrap(),
        "select 2; -- edited"
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
    let screen = screen_text(&a);
    let last_row = screen.lines().last().unwrap();
    assert!(last_row.trim() == "Saved x", "{last_row:?}");
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
    assert!(matches!(d.kind, DialogKind::QuitSaveChanges { .. }));
    assert_eq!(d.title, "Disconnect");
    assert_eq!(a.tabs[a.active_tab].id, unsaved);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 5);
    assert_eq!(a.closing.bulk, None);

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
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::SaveChanges { .. }
    ));
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
        a.dialogs.last().map(|d| &d.kind)
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
    a.files.dir = dir.path().to_path_buf();
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
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::ConfirmExportOverwrite { .. }
    ));
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

/// Double clicks in the file dialog (D-067): a folder opens, a file is chosen.
#[test]
fn double_click_in_the_file_dialog() {
    let (mut a, dir) = app();
    let files = dir.path().join("sql");
    std::fs::create_dir_all(files.join("sub")).unwrap();
    std::fs::write(files.join("sub").join("pick.sql"), "x").unwrap();
    a.files.dir = files.clone();
    a.new_query_tab(None);
    typing(&mut a, "select 1;");
    a.run(Command::SaveFileAs);
    // clicks on the listing row showing `text`
    let click = |a: &mut App, text: &str, times: usize| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        let mut hits = Hits::default();
        terminal
            .draw(|f| crate::view::render(f, a, &mut hits))
            .unwrap();
        let screen = screen_text(a);
        let (y, line) = screen
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(&format!(" {text} ")))
            .unwrap_or_else(|| panic!("{text}: {screen}"));
        let col = line.find(text).map(|b| line[..b].chars().count()).unwrap() as u16;
        for _ in 0..times {
            a.on_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: col,
                    row: y as u16,
                    modifiers: KeyModifiers::NONE,
                },
                &hits,
            );
        }
    };
    let sub = format!("sub{}", std::path::MAIN_SEPARATOR);
    // a single click only selects
    click(&mut a, &sub, 1);
    assert_eq!(a.files.dir, files);
    assert_eq!(a.dialogs.last().unwrap().focused_id(), Some("files"));
    a.mouse.last_click = None; // later, not a double click
    // a double click opens the folder
    click(&mut a, &sub, 2);
    assert_eq!(a.files.dir, files.join("sub"));
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::SaveFile);
    // a double click on a file chooses it: Save asks before overwriting
    click(&mut a, "pick.sql", 2);
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::ConfirmOverwrite { .. }
    ));
}

#[test]
fn typed_paths_in_file_dialogs() {
    use super::files::resolve;
    let dir = std::path::Path::new(if cfg!(windows) {
        r"C:\work\sql"
    } else {
        "/work/sql"
    });
    let home = std::path::Path::new(if cfg!(windows) {
        r"C:\Users\me"
    } else {
        "/home/me"
    });
    let r = |s: &str| resolve(dir, s, Some(home));
    assert_eq!(r("a.sql"), dir.join("a.sql"));
    assert_eq!(
        r("../x/./b.sql"),
        dir.parent().unwrap().join("x").join("b.sql")
    );
    assert_eq!(r("~"), home);
    assert_eq!(r("~/q.sql"), home.join("q.sql"));
    assert_eq!(r("../../../.."), dir.ancestors().last().unwrap());
    if cfg!(windows) {
        assert_eq!(r("D:"), std::path::PathBuf::from(r"D:\"));
        assert_eq!(r(r"D:\data\..\x"), std::path::PathBuf::from(r"D:\x"));
        assert_eq!(r(r"~\q.sql"), home.join("q.sql"));
        assert_eq!(
            r(r"\\srv\share\a"),
            std::path::PathBuf::from(r"\\srv\share\a")
        );
    } else {
        assert_eq!(r("/etc/../tmp"), std::path::PathBuf::from("/tmp"));
    }
}

/// Places (D-070): the button lists them in the listing, `..` goes back, a place
/// opens; Cancel leaves the places.
#[test]
fn places_in_the_file_dialog() {
    let (mut a, dir) = app();
    let files = dir.path().join("sql");
    std::fs::create_dir_all(files.join("sub")).unwrap();
    std::fs::write(files.join("sub").join("q.sql"), "select 1;").unwrap();
    a.files.dir = files.clone();
    // a tab with a file: its folder is a place
    a.open_path(files.join("sub").join("q.sql"));
    a.open_file_dialog(false);
    key(&mut a, "Alt+P");
    let screen = screen_text(&a);
    assert!(screen.contains("Folder: Places"), "{screen}");
    assert!(screen.contains("Current file"), "{screen}");
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.focused_id(), Some("files"));
    // `..`: back to the folder
    key(&mut a, "Enter");
    assert!(a.files.places.is_none());
    assert_eq!(a.files.dir, files);
    // the place of the current file (after `~` when there is a home folder)
    key(&mut a, "Alt+P");
    let at = a
        .files
        .places
        .as_ref()
        .unwrap()
        .iter()
        .position(|p| p.note == "Current file");
    for _ in 0..=at.unwrap() {
        key(&mut a, "Down");
    }
    key(&mut a, "Enter");
    assert_eq!(a.files.dir, files.join("sub"));
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::OpenFile);
    // Cancel in the places: the next dialog shows the folder
    key(&mut a, "Alt+P");
    assert!(a.files.places.is_some());
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert!(a.files.places.is_none());
    // the Cancel button does the same as Esc
    a.open_file_dialog(false);
    key(&mut a, "Alt+P");
    assert!(a.files.places.is_some());
    key(&mut a, "Alt+C");
    assert!(a.dialogs.is_empty());
    assert!(a.files.places.is_none());
}

/// A typed path (D-070): a folder opens, a missing folder is an error.
#[test]
fn typed_folder_and_missing_folder() {
    let (mut a, dir) = app();
    let files = dir.path().join("sql");
    std::fs::create_dir_all(files.join("sub")).unwrap();
    a.files.dir = files.join("sub");
    a.new_query_tab(None);
    a.run(Command::SaveFileAs);
    for _ in 0.."query.sql".len() {
        key(&mut a, "Backspace");
    }
    typing(&mut a, "..");
    key(&mut a, "Enter");
    assert_eq!(a.files.dir, files);
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::SaveFile);
    for _ in 0.."query.sql".len() {
        key(&mut a, "Backspace");
    }
    typing(&mut a, "missing/x.sql");
    key(&mut a, "Enter");
    assert_eq!(a.dialogs.len(), 2);
    assert_eq!(a.dialogs[0].kind, DialogKind::SaveFile);
    assert!(a.dialogs[1].error);
    assert!(!files.join("missing").exists());
}

#[test]
fn file_paths_are_shortened_from_the_left() {
    use crate::view::shorten_left;
    assert_eq!(shorten_left(r"C:\a\b.sql", 20), r"C:\a\b.sql");
    assert_eq!(shorten_left(r"C:\work\report.sql", 12), r"…\report.sql");
    assert_eq!(shorten_left("/home/dž/ćevapi.sql", 11), "…ćevapi.sql");
}

#[test]
fn save_and_open_sql_files_through_the_dialog() {
    let (mut a, dir) = app();
    let files = dir.path().join("sql");
    std::fs::create_dir_all(files.join("sub")).unwrap();
    a.files.dir = files.clone();
    a.new_query_tab(None); // unbound (no server): still editable
    typing(&mut a, "select 'čšž';");
    // no file: no file bar, the editor follows the tab strip's line
    let screen = screen_text(&a);
    let lines: Vec<&str> = screen.lines().collect();
    let first = lines
        .iter()
        .position(|l| l.contains("select 'čšž';"))
        .unwrap();
    assert!(lines[first - 1].contains("──"), "{screen}");
    // Save: the name field starts with query.sql
    a.run(Command::SaveFile);
    assert_eq!(a.dialogs[0].kind, DialogKind::SaveFile);
    key(&mut a, "Enter");
    let saved = files.join("query.sql");
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "select 'čšž';");
    assert!(a.tabs[a.active_tab].label().starts_with("query.sql"));
    // the full path is shown in a bar right above the editor (D-064)
    let tail = format!("sql{}query.sql", std::path::MAIN_SEPARATOR);
    let screen = screen_text(&a);
    let lines: Vec<&str> = screen.lines().collect();
    let bar = lines.iter().position(|l| l.contains(&tail));
    assert!(
        bar.is_some_and(|i| lines[i + 1].contains("select 'čšž';")),
        "{screen}"
    );
    assert!(
        !lines[bar.unwrap()].contains(" *"),
        "saved: no mark {screen}"
    );
    // Ctrl+S with a file: written at once, no dialog (D-064)
    typing(&mut a, " -- more");
    // unsaved changes: ` *` after the path
    let screen = screen_text(&a);
    assert!(
        screen.lines().any(|l| l.contains(&format!("{tail} *"))),
        "{screen}"
    );
    key(&mut a, "Ctrl+S");
    assert!(a.dialogs.is_empty());
    assert_eq!(
        std::fs::read_to_string(&saved).unwrap(),
        "select 'čšž'; -- more"
    );
    assert!(!a.editor().modified());
    // Save as: always the dialog, in the file's folder; asks before overwriting
    a.files.dir = files.join("sub");
    a.run(Command::SaveFileAs);
    assert_eq!(a.dialogs[0].kind, DialogKind::SaveFile);
    assert_eq!(a.files.dir, files);
    key(&mut a, "Enter");
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::ConfirmOverwrite { .. }
    ));
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
    // Save as under another name: the tab belongs to the new file
    a.run(Command::SaveFileAs);
    for _ in 0.."query.sql".len() {
        key(&mut a, "Backspace");
    }
    typing(&mut a, "other.sql");
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
    assert_eq!(a.query().unwrap().file, Some(files.join("other.sql")));
    assert!(files.join("other.sql").exists());
    // Open: the list shows ../, sub/ and query.sql (highlighted); pick the file
    a.open_file_dialog(false);
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.focused_id(), Some("files"));
    key(&mut a, "Down"); // sub/
    key(&mut a, "Enter"); // into sub/
    assert_eq!(a.files.dir, files.join("sub"));
    key(&mut a, "Enter"); // ../
    assert_eq!(a.files.dir, files);
    key(&mut a, "End"); // query.sql
    key(&mut a, "Enter");
    assert!(
        a.dialogs.is_empty(),
        "{:?}",
        a.dialogs.last().map(|d| &d.kind)
    );
    assert_eq!(a.editor().text(), "select 'čšž'; -- more");
    assert_eq!(a.tabs.len(), 3);
}

#[test]
fn query_tab_layout_draws_results_and_messages() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    typing(&mut a, "select 1");
    let q = a.query_mut().unwrap();
    q.start_run(1, Sent::new(0, ""));
    let none = |_: usize| 0;
    let lines = LineMap { line_of: &none };
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
    // Messages first, then the shown result set (D-066)
    let text = screen_text(&a);
    assert!(text.contains(" Messages (0)  │  1: 2+ rows "), "{text}");
    assert!(text.contains("answer"), "{text}");
    assert!(text.contains("[null]"), "{text}");
    assert!(text.contains("2 rows loaded – more available"), "{text}");
    key(&mut a, "Alt+M");
    assert!(a.query().unwrap().on_messages());
    let text = screen_text(&a);
    assert!(
        text.contains(" Messages (0)  │  1: 2+ rows "),
        "result tabs stay: {text}"
    );
}

/// D-019: an error shows psql's `LINE n:` with a caret; Enter jumps to it and selects
/// the token; `Ctrl+A` + copy copies all messages; the Messages context menu.
#[test]
fn message_positions_jump_and_copy() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    let text = "select 1;\nselect nosuchcol from t;";
    a.editor().set_text(text);
    let q = a.query_mut().unwrap();
    q.start_run(1, Sent::new(0, text));
    let line_of = |c: usize| text[..c].matches('\n').count();
    let lines = LineMap { line_of: &line_of };
    q.apply(
        pgc_db::QueryEvent::Command {
            tag: "SELECT 1".into(),
        },
        &lines,
    );
    q.apply(
        pgc_db::QueryEvent::Error(pgc_db::ServerMessage {
            severity: "ERROR".into(),
            message: "column \"nosuchcol\" does not exist".into(),
            position: Some(18),
            ..Default::default()
        }),
        &lines,
    );
    let m = &a.query().unwrap().messages[1];
    assert_eq!(
        m.text,
        format!(
            "ERROR:  column \"nosuchcol\" does not exist\n\
             LINE 2: select nosuchcol from t;\n{}^",
            " ".repeat(15)
        )
    );
    a.focus = Focus::Results;
    a.query_mut().unwrap().message_cursor = 1;
    key(&mut a, "Enter");
    assert_eq!(a.focus, Focus::Tabs);
    assert_eq!(a.editor().selected_text().as_deref(), Some("nosuchcol"));

    // Ctrl+A, copy: every message; moving the cursor ends the selection
    a.focus = Focus::Results;
    key(&mut a, "Ctrl+A");
    assert!(a.query().unwrap().messages_all_selected);
    key(&mut a, "Ctrl+C");
    let all = a.clipboard.paste();
    assert!(
        all.starts_with("SELECT 1\nERROR:") && all.contains("LINE 2:"),
        "{all}"
    );
    key(&mut a, "Up");
    assert!(!a.query().unwrap().messages_all_selected);
    key(&mut a, "Ctrl+C");
    assert_eq!(a.clipboard.paste(), "SELECT 1");

    // F2: Jump to line (only on a message with a position), Copy message, Copy all
    key(&mut a, "F2");
    let screen = screen_text(&a);
    for item in ["Jump to line", "Copy message", "Copy all messages"] {
        assert!(screen.contains(item), "{screen}");
    }
    assert!(!a.is_enabled(Command::Accept), "SELECT 1 has no position");
    assert!(a.is_enabled(Command::CopyAllMessages));
    a.popup = None;
    key(&mut a, "Down");
    assert!(a.is_enabled(Command::Accept));
    a.run(Command::CopyAllMessages);
    assert_eq!(a.clipboard.paste(), all);
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
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::Login { .. }
    ));
    // the focus is in the password field: its keys are kept out of the key log
    assert!(a.dialogs.last().unwrap().focused_masked());
    typing(&mut a, "secret");
    key(&mut a, "Tab");
    assert!(!a.dialogs.last().unwrap().focused_masked(), "the checkbox");
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
    assert_eq!(d.kind, DialogKind::Login { server: uid });
    assert_eq!(a.tree.servers[0].state, ConnState::Disconnected);
}

#[test]
fn connection_dialog_validates_and_edits() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "old");
    a.run(Command::EditConnection);
    assert!(matches!(a.dialogs[0].kind, DialogKind::Connection { .. }));
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
    assert!(matches!(
        a.dialogs[0].kind,
        DialogKind::ConfirmDelete { .. }
    ));
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
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::ConfirmDelete { .. }
    ));
    key(&mut a, "Enter");
    let d = a.dialogs.last().unwrap();
    assert!(matches!(d.kind, DialogKind::QuitSaveChanges { .. }));
    assert_eq!(d.title, "Delete connection");
    assert_eq!(a.tabs[a.active_tab].id, unsaved);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert_eq!(a.tabs.len(), 4);
    assert_eq!(a.tree.servers.len(), 2);
    assert_eq!(a.closing.bulk, None);

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
    q.start_run(1, Sent::new(0, ""));
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
    assert!(matches!(a.dialogs[0].kind, DialogKind::Connection { .. }));
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
    a.templates.form = Some(TemplateForm::new(
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
    let form = a.templates.form.as_ref().unwrap();
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
    assert!(a.templates.form.is_some(), "the form stays open");
}

/// Autocomplete in the F4 input (D-054): only on `Ctrl+Space` / `Alt+/`, the
/// command's own object kinds, Enter puts the name into the input (does not run),
/// Esc closes only the list.
#[tokio::test]
async fn autocomplete_in_the_psql_command_line() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "a");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
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
    let row = |s: &str, n: &str, k: &str, v: &str| vec![s.into(), n.into(), k.into(), v.into()];
    q.names.set_objects(vec![
        row("public", "orders", "table", "t"),
        row("public", "order_view", "view", "t"),
        row("sales", "Order Items", "table", "f"),
    ]);
    key(&mut a, "F4");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::PsqlCommand);
    // never by itself
    typing(&mut a, "dt ord");
    assert!(a.psql_completion.is_none());
    // dt: tables only
    key(&mut a, "Ctrl+Space");
    let p = a.psql_completion.as_ref().expect("the list opened");
    let labels: Vec<&str> = p.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["orders"]);
    let screen = screen_text(&a);
    assert!(
        screen.contains("orders") && screen.contains("table"),
        "{screen}"
    );
    // Esc closes only the list
    key(&mut a, "Esc");
    assert!(a.psql_completion.is_none());
    assert_eq!(a.dialogs.len(), 1, "the command line stays open");
    // gts: tables and views; schema-qualified when not on the search path
    a.dialogs.last_mut().unwrap().set_text("cmd", "gts ");
    key(&mut a, "Alt+/");
    let labels: Vec<String> = a
        .psql_completion
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|i| i.label.clone())
        .collect();
    assert_eq!(labels, ["order_view", "orders", "sales.Order Items"]);
    // typing filters; Enter puts the quoted name into the input and does not run
    typing(&mut a, "sal");
    assert_eq!(a.psql_completion.as_ref().unwrap().items.len(), 1);
    key(&mut a, "Enter");
    assert!(a.psql_completion.is_none());
    let d = a.dialogs.last().unwrap();
    assert_eq!(d.kind, DialogKind::PsqlCommand);
    assert_eq!(d.text("cmd"), Some("gts sales.\"Order Items\""));
    // a command without object kinds offers nothing
    a.dialogs.last_mut().unwrap().set_text("cmd", "du ");
    key(&mut a, "Ctrl+Space");
    assert!(a.psql_completion.is_none());
    assert_eq!(a.status, "No suggestions");
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
    assert!(tab.query().is_some());
    assert_eq!(tab.view.editor.text(), def);
    assert!(tab.view.editor.modified(), "unsaved text");
    assert!(!tab.view.editor.read_only());
    assert_eq!(tab.query().unwrap().binding, Some((uid, "shop".to_owned())));
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

/// Database statistics (D-062): only on database rows, Alt+I in the tree, one tab per
/// database, read on connect and on F5 only, closed with the tab.
#[tokio::test]
async fn alt_i_opens_the_database_statistics() {
    use crate::objects::ObjType;
    use crate::tree::{NodeKind, RowKind};
    use pgc_db::NodeRow;
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "srv");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    let dbs = a.tree.servers[0]
        .children
        .iter()
        .find(|n| n.kind == NodeKind::Group(ObjType::Database))
        .unwrap()
        .nid;
    a.tree.set_children(
        dbs,
        vec![NodeRow {
            oid: 5,
            name: "shop".into(),
            label: None,
            partitioned: false,
        }],
    );
    a.tree.node_mut(dbs).unwrap().expanded = true;
    let db = a.tree.node(dbs).unwrap().children.as_ref().unwrap()[0].nid;
    // not on the server row or the Databases group
    a.tree.select_kind(RowKind::Server);
    assert!(!a.is_enabled(Command::DbStats));
    a.tree.select_kind(RowKind::Node(dbs));
    assert!(!a.is_enabled(Command::DbStats));
    a.tree.select_kind(RowKind::Node(db));
    assert!(a.is_enabled(Command::DbStats));
    a.focus = Focus::Tree;
    key(&mut a, "Alt+I");
    assert_eq!(a.tabs.len(), 2);
    assert!(a.tabs[1].db_stats().is_some());
    assert_eq!(a.tabs[1].label(), "Stats: srv · shop");
    assert_eq!(a.context(), Some(crate::keymap::Context::DbStats));
    // the same database again: the open tab
    a.focus = Focus::Tree;
    key(&mut a, "Alt+I");
    assert_eq!(a.tabs.len(), 2);
    assert_eq!(a.active_tab, 1);
    let tab = a.tabs[1].id;

    // pretend the connection is open (a detached handle keeps the requests)
    let conn = {
        let s = a.tabs[1].db_stats_mut().unwrap();
        let c = s.conn.as_mut().unwrap();
        c.handle.abort();
        c.handle = pgc_db::ConnectionHandle::detached(c.id);
        c.connected = true;
        s.version = Some(pgc_core::ServerVersion(170002));
        c.id
    };
    a.db_stats_refresh(tab);
    let k = a.tabs[1].db_stats().unwrap().key.unwrap();
    // F5 while a read is on its way sends nothing more
    key(&mut a, "F5");
    assert_eq!(a.tabs[1].db_stats().unwrap().key, Some(k));
    let r = |key: &str, label: &str, kind: &str, value: &str| -> Vec<String> {
        [key, "Database", label, kind, value]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    };
    a.on_db_stats_event(
        conn,
        tab,
        DbEvent::Lookup {
            id: conn,
            key: k,
            result: Ok(vec![
                r("read_at", "Read at", "time", "2026-11-13 10:00:00+01"),
                r("size", "Size", "bytes", "12582912"),
                r("backends", "Backends", "count", "3"),
            ]),
        },
    );
    {
        let s = a.tabs[1].db_stats().unwrap();
        assert_eq!(s.key, None);
        assert_eq!(s.grid.rows.len(), 2);
        let (_, right) = a.monitor_status(&a.tabs[1]).unwrap();
        assert!(right.starts_with("Read at 2026-11-13"), "{right}");
    }
    // F5 reads again
    key(&mut a, "F5");
    assert!(a.tabs[1].db_stats().unwrap().key.is_some());
    // closed with the tab
    key(&mut a, "Ctrl+W");
    assert_eq!(a.tabs.len(), 1);
    assert!(!a.connections.contains_key(&conn));
}

/// Ctrl+B (D-056): the top 100 rows of the table, view or partition that the selected
/// row is or is below, in a new query tab run once its connection is open; refused
/// elsewhere.
#[tokio::test]
async fn ctrl_b_selects_the_top_rows_of_the_relation() {
    use crate::objects::ObjType;
    use crate::tree::{DbConn, NodeKind, RowKind};
    use pgc_db::NodeRow;
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "srv");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    let row = |oid: u32, name: &str, label: Option<&str>, partitioned: bool| NodeRow {
        oid,
        name: name.into(),
        label: label.map(str::to_owned),
        partitioned,
    };
    let child = |a: &App, parent: Option<u64>, kind: NodeKind, oid: u32| -> u64 {
        let kids = match parent {
            None => &a.tree.servers[0].children,
            Some(p) => a.tree.node(p).unwrap().children.as_ref().unwrap(),
        };
        kids.iter()
            .find(|n| n.kind == kind && (oid == 0 || n.oid == oid))
            .unwrap()
            .nid
    };
    let dbs = child(&a, None, NodeKind::Group(ObjType::Database), 0);
    a.tree.set_children(dbs, vec![row(5, "shop", None, false)]);
    let db = child(&a, Some(dbs), NodeKind::Object(ObjType::Database), 5);
    let groups = a.tree.groups(&ObjType::Database.children(false));
    {
        let n = a.tree.node_mut(db).unwrap();
        n.conn = Some(DbConn {
            state: ConnState::Connected,
            handle: Some(pgc_db::ConnectionHandle::detached(51)),
        });
        n.children = Some(groups);
    }
    let schemas = child(&a, Some(db), NodeKind::Group(ObjType::Schema), 0);
    a.tree
        .set_children(schemas, vec![row(2200, "My Schema", None, false)]);
    let schema = child(&a, Some(schemas), NodeKind::Object(ObjType::Schema), 2200);
    let tables = child(&a, Some(schema), NodeKind::Group(ObjType::Table), 0);
    a.tree
        .set_children(tables, vec![row(10, "orders", None, true)]);
    let orders = child(&a, Some(tables), NodeKind::Object(ObjType::Table), 10);
    let columns = child(&a, Some(orders), NodeKind::Group(ObjType::Column), 0);
    a.tree
        .set_children(columns, vec![row(1, "id", None, false)]);
    let id = child(&a, Some(columns), NodeKind::Object(ObjType::Column), 1);
    let parts = child(&a, Some(orders), NodeKind::Group(ObjType::Partition), 0);
    a.tree.set_children(
        parts,
        vec![
            row(11, "orders_2025", None, false),
            row(12, "orders_2026", Some("archive.orders_2026"), false),
        ],
    );
    let p25 = child(&a, Some(parts), NodeKind::Object(ObjType::Partition), 11);
    let p26 = child(&a, Some(parts), NodeKind::Object(ObjType::Partition), 12);
    let views = child(&a, Some(schema), NodeKind::Group(ObjType::View), 0);
    a.tree.set_children(views, vec![row(20, "v", None, false)]);
    let v = child(&a, Some(views), NodeKind::Object(ObjType::View), 20);
    let roles = child(&a, None, NodeKind::Group(ObjType::Role), 0);
    for nid in [
        dbs, db, schemas, schema, tables, orders, columns, parts, views,
    ] {
        a.tree.node_mut(nid).unwrap().expanded = true;
    }

    // the query of the nearest relation above (or at) the row
    let sql = |a: &mut App, nid: u64| {
        a.tree.select_kind(RowKind::Node(nid));
        a.top_rows_source().map(|(_, db, sql)| (db, sql))
    };
    let orders_sql = "SELECT * FROM \"My Schema\".orders LIMIT 100;\n";
    for nid in [orders, columns, id, parts] {
        assert_eq!(
            sql(&mut a, nid),
            Ok(("shop".to_owned(), orders_sql.to_owned()))
        );
    }
    assert_eq!(
        sql(&mut a, p25).unwrap().1,
        "SELECT * FROM \"My Schema\".orders_2025 LIMIT 100;\n"
    );
    assert_eq!(
        sql(&mut a, p26).unwrap().1,
        "SELECT * FROM archive.orders_2026 LIMIT 100;\n"
    );
    assert_eq!(
        sql(&mut a, v).unwrap().1,
        "SELECT * FROM \"My Schema\".v LIMIT 100;\n"
    );
    // refused: groups above tables, schemas, server-level rows
    for nid in [tables, schema, db, roles] {
        a.tree.select_kind(RowKind::Node(nid));
        assert!(!a.is_enabled(Command::SelectTopRows));
    }
    a.focus = Focus::Tree;
    key(&mut a, "Ctrl+B");
    assert!(a.status.starts_with("Select a table"), "{}", a.status);
    assert_eq!(a.tabs.len(), 1);

    // Ctrl+B on a column: a new query tab with the query, run once connected
    a.tree.select_kind(RowKind::Node(id));
    a.focus = Focus::Tree;
    key(&mut a, "Ctrl+B");
    assert_eq!(a.tabs.len(), 2);
    assert_eq!(a.focus, Focus::Tabs);
    let tab = &a.tabs[a.active_tab];
    let tab_id = tab.id;
    assert_eq!(tab.view.editor.text(), orders_sql);
    assert!(tab.view.editor.modified(), "unsaved text");
    let q = tab.query().unwrap();
    assert_eq!(q.binding, Some((uid, "shop".to_owned())));
    assert!(
        q.run_on_connect && q.running.is_none(),
        "waits for the connection"
    );
    // the connection opens (no real one behind it) while another tab is active
    let q = a.tabs[1].query_mut().unwrap();
    q.conn = Some(QueryConn {
        id: 99,
        database: "shop".into(),
        handle: pgc_db::ConnectionHandle::detached(99),
        cancel: None,
        state: QueryConnState::Connected,
        ssl_mode: SslMode::default(),
        ssl_root_cert: None,
    });
    a.active_tab = 0;
    a.run_queued(tab_id);
    assert_eq!(a.active_tab, 0, "the active tab stays");
    let q = a.tabs[1].query().unwrap();
    assert!(!q.run_on_connect && q.running.is_some(), "executed");
    // only once
    a.run_queued(tab_id);
    assert!(!a.tabs[1].query().unwrap().run_on_connect);
}

/// Find object (D-055): scopes of tree rows, the dialog's search, and opening the tree
/// level by level down to the chosen object (waiting for unloaded groups).
#[tokio::test]
async fn find_object_searches_and_opens_the_tree_to_the_object() {
    use crate::objects::ObjType;
    use crate::tree::{DbConn, NodeKind, RowKind};
    use pgc_db::NodeRow;
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "srv");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    a.tree.by_uid(uid).unwrap().handle = Some(pgc_db::ConnectionHandle::detached(50));
    let row = |oid: u32, name: &str| NodeRow {
        oid,
        name: name.into(),
        label: None,
        partitioned: false,
    };
    let child = |a: &App, parent: Option<u64>, kind: NodeKind, oid: u32| -> u64 {
        let kids = match parent {
            None => &a.tree.servers[0].children,
            Some(p) => a.tree.node(p).unwrap().children.as_ref().unwrap(),
        };
        kids.iter()
            .find(|n| n.kind == kind && (oid == 0 || n.oid == oid))
            .unwrap()
            .nid
    };
    // a loaded Databases group and a connected database "shop" with its groups
    let dbs = child(&a, None, NodeKind::Group(ObjType::Database), 0);
    a.tree.set_children(dbs, vec![row(5, "shop")]);
    a.tree.node_mut(dbs).unwrap().expanded = true;
    let db = child(&a, Some(dbs), NodeKind::Object(ObjType::Database), 5);
    let groups = a.tree.groups(&ObjType::Database.children(false));
    {
        let n = a.tree.node_mut(db).unwrap();
        n.conn = Some(DbConn {
            state: ConnState::Connected,
            handle: Some(pgc_db::ConnectionHandle::detached(51)),
        });
        n.children = Some(groups);
    }
    let schemas = child(&a, Some(db), NodeKind::Group(ObjType::Schema), 0);
    a.tree.set_children(schemas, vec![row(2200, "public")]);
    let public = child(&a, Some(schemas), NodeKind::Object(ObjType::Schema), 2200);
    let tables = child(&a, Some(public), NodeKind::Group(ObjType::Table), 0);

    // scopes: server row, Databases group, database, group below it
    a.focus = Focus::Tree;
    a.tree.select_server(uid);
    let scope = a.find_scope().unwrap();
    assert!(scope.database.is_none() && scope.all_databases);
    a.tree.select_kind(RowKind::Node(dbs));
    let scope = a.find_scope().unwrap();
    assert_eq!(scope.prefix, "databases:");
    assert!(
        scope.all_databases,
        "the Databases group searches in them too"
    );
    a.tree.select_kind(RowKind::Node(db));
    let scope = a.find_scope().unwrap();
    assert_eq!((scope.database, scope.prefix.as_str()), (Some((db, 5)), ""));

    // the dialog: typing starts the pause, the timer searches, the answer fills the list
    key(&mut a, "Ctrl+F");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::FindObject);
    assert!(
        !a.dialogs[0].checked("columns"),
        "Show columns is off by default"
    );
    assert_eq!(
        a.dialogs[0].selected("match"),
        Some(0),
        "Contains by default"
    );
    typing(&mut a, "cust");
    let seq = a.find_seq;
    assert!(seq > 0);
    // another match mode is another search
    key(&mut a, "Alt+M");
    key(&mut a, "Right");
    assert!(a.find_seq > seq, "a changed mode starts a new search");
    assert_eq!(
        a.find.as_ref().unwrap().seen.1,
        super::find_object::Match::StartsWith
    );
    let seq = a.find_seq;
    a.update(Msg::FindTimer(seq), &Hits::default());
    let pending = &a.find.as_ref().unwrap().pending;
    assert_eq!(pending.len(), 1, "the search was sent to the database");
    let key_sent = *pending.keys().next().unwrap();
    let r = |v: [&str; 5]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
    a.find_loaded(
        key_sent,
        Ok(vec![
            r([
                "table",
                "public",
                "customers",
                "schemas:2200/tables:101",
                "2",
            ]),
            r([
                "column",
                "public",
                "orders.customer",
                "schemas:2200/tables:100/columns:2",
                "25",
            ]),
        ]),
    );
    let f = a.find.as_ref().unwrap();
    assert_eq!(f.hits.len(), 2);
    assert_eq!(
        (f.hits[1].database.as_str(), f.hits[1].schema.as_str()),
        ("shop", "public"),
        "database and schema columns"
    );
    let screen = screen_text(&a);
    assert!(
        screen.contains("Database") && screen.contains("Schema"),
        "{screen}"
    );
    assert!(screen.contains("orders.customer"), "{screen}");
    a.dialogs[0].list_select("list", 1);
    // the focus is on the Match combo (Enter would open it): Ctrl+Enter is OK
    key(&mut a, "Ctrl+Enter");
    assert!(a.dialogs.is_empty() && a.find.is_none());

    // Tables is not loaded: the tree waits for it, then for the table's Columns
    assert!(a.reveal.is_some());
    assert!(
        a.tree.node(tables).unwrap().loading,
        "Tables is being loaded"
    );
    a.tree
        .set_children(tables, vec![row(100, "orders"), row(101, "customers")]);
    a.after_tree_event();
    let orders = child(&a, Some(tables), NodeKind::Object(ObjType::Table), 100);
    let columns = child(&a, Some(orders), NodeKind::Group(ObjType::Column), 0);
    assert!(a.reveal.is_some());
    a.tree
        .set_children(columns, vec![row(1, "id"), row(2, "customer")]);
    a.after_tree_event();
    assert!(a.reveal.is_none());
    let customer = child(&a, Some(columns), NodeKind::Object(ObjType::Column), 2);
    assert_eq!(a.tree.selected().unwrap().kind, RowKind::Node(customer));
    assert_eq!(a.focus, Focus::Tree);

    // scopes below the database; a column has nothing below it
    a.tree.select_kind(RowKind::Node(tables));
    assert_eq!(a.find_scope().unwrap().prefix, "schemas:2200/tables:");
    a.tree.select_kind(RowKind::Node(orders));
    assert_eq!(a.find_scope().unwrap().prefix, "schemas:2200/tables:100/");
    a.tree.select_kind(RowKind::Node(customer));
    assert!(a.find_scope().is_err());

    // an object that is gone: the cursor stays on the deepest level reached
    a.reveal = Some(super::find_object::Reveal {
        uid,
        steps: vec![
            (ObjType::Database, 5),
            (ObjType::Schema, 2200),
            (ObjType::Table, 999),
        ],
        tried: Default::default(),
    });
    a.continue_reveal();
    assert!(a.reveal.is_none());
    assert!(a.status.contains("not there"), "{}", a.status);
    assert_eq!(a.tree.selected().unwrap().kind, RowKind::Node(tables));
}

/// Find object from a server row (D-055): the server's objects and every database —
/// databases that are not connected are opened (and closed again with the dialog),
/// failures are listed, results of all sources are merged with the database shown.
#[tokio::test]
async fn find_object_searches_all_databases_of_the_server() {
    use super::find_object::Source;
    use crate::objects::ObjType;
    use crate::tree::{DbConn, NodeKind};
    use pgc_db::NodeRow;
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "srv");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    a.tree.by_uid(uid).unwrap().handle = Some(pgc_db::ConnectionHandle::detached(50));
    let row = |oid: u32, name: &str| NodeRow {
        oid,
        name: name.into(),
        label: None,
        partitioned: false,
    };
    let dbs = a.tree.servers[0]
        .children
        .iter()
        .find(|n| n.kind == NodeKind::Group(ObjType::Database))
        .unwrap()
        .nid;
    a.tree
        .set_children(dbs, vec![row(5, "mantis"), row(6, "shop")]);
    let db_nid = |a: &App, oid: u32| {
        a.tree
            .node(dbs)
            .unwrap()
            .children
            .as_ref()
            .unwrap()
            .iter()
            .find(|n| n.oid == oid)
            .unwrap()
            .nid
    };
    let (mantis, shop) = (db_nid(&a, 5), db_nid(&a, 6));
    // both databases look connected (no real connections in tests)
    for nid in [mantis, shop] {
        let groups = a.tree.groups(&ObjType::Database.children(false));
        let n = a.tree.node_mut(nid).unwrap();
        n.conn = Some(DbConn {
            state: ConnState::Connected,
            handle: Some(pgc_db::ConnectionHandle::detached(60 + nid)),
        });
        n.children = Some(groups);
    }
    a.focus = Focus::Tree;
    a.tree.select_server(uid);
    key(&mut a, "Ctrl+F");
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::FindObject);
    // pretend the search opened "mantis" itself
    a.find.as_mut().unwrap().opened.insert(mantis);
    typing(&mut a, "mantis");
    let seq = a.find_seq;
    a.update(Msg::FindTimer(seq), &Hits::default());
    let pending: Vec<(u64, Source)> = a
        .find
        .as_ref()
        .unwrap()
        .pending
        .iter()
        .map(|(k, s)| (*k, *s))
        .collect();
    assert_eq!(pending.len(), 3, "server and both databases: {pending:?}");
    let key_of = |s: Source| pending.iter().find(|(_, x)| *x == s).unwrap().0;
    let r = |v: [&str; 5]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
    a.find_loaded(
        key_of(Source::Server),
        Ok(vec![r(["database", "", "mantis", "databases:5", "-3"])]),
    );
    a.find_loaded(
        key_of(Source::Database(mantis)),
        Ok(vec![r([
            "table",
            "public",
            "mantis_bug_table",
            "schemas:2200/tables:100",
            "2",
        ])]),
    );
    let info = |a: &App| {
        a.dialogs[0]
            .widgets
            .iter()
            .find(|w| w.id == "info")
            .unwrap()
            .label
            .text
            .clone()
    };
    assert!(info(&a).contains("1 of 2 databases"), "{}", info(&a));
    a.find_loaded(key_of(Source::Database(shop)), Ok(Vec::new()));
    let f = a.find.as_ref().unwrap();
    let shown: Vec<(&str, &str, &str)> = f
        .hits
        .iter()
        .map(|h| (h.database.as_str(), h.schema.as_str(), h.name.as_str()))
        .collect();
    assert_eq!(
        shown,
        [("", "", "mantis"), ("mantis", "public", "mantis_bug_table")],
        "the database object first (no database, no schema), then the table"
    );
    assert_eq!(f.hits[1].steps[0], (ObjType::Database, 5));
    assert_eq!(info(&a), "2 objects");

    // a database the search could not open is listed, without an error dialog
    a.find.as_mut().unwrap().opened.insert(shop);
    assert!(a.find_connect_failed(shop, &pgc_db::DbError::PasswordRequired));
    assert!(
        info(&a).contains("not searched: shop (password needed)"),
        "{}",
        info(&a)
    );
    assert_eq!(a.dialogs.len(), 1);

    // closing the dialog closes the connections opened for the search
    key(&mut a, "Esc");
    a.update(Msg::EscTimeout(a.esc.seq), &Hits::default());
    assert!(a.dialogs.is_empty() && a.find.is_none());
    let conn = a.tree.node(mantis).unwrap().conn.as_ref().unwrap();
    assert_eq!(conn.state, ConnState::Disconnected);
    assert_eq!(
        a.tree.node(shop).unwrap().conn.as_ref().unwrap().state,
        ConnState::Connected,
        "a database connected before the search stays"
    );
}

/// Server configuration on the server row (D-037): read once on selection, shown below
/// the version, read again with Ctrl+R, dropped on disconnect.
#[tokio::test]
async fn server_row_shows_configuration_settings() {
    use crate::tree::ServerSettings;
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "srv");
    let id = fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    a.tree.by_uid(uid).unwrap().handle = Some(pgc_db::ConnectionHandle::detached(id));
    a.focus = Focus::Tree;
    a.tree.select_server(uid);
    a.follow_selection();
    let ServerSettings::Loading(k) = a.tree.servers[0].settings else {
        panic!("requested on selection: {:?}", a.tree.servers[0].settings);
    };
    assert!(a.tabs[0].view.shown.contains("-- Configuration: loading…"));
    // selecting again does not read them again
    a.follow_selection();
    assert_eq!(a.tree.servers[0].settings, ServerSettings::Loading(k));

    let row = |v: [&str; 6]| v.map(str::to_owned).to_vec();
    a.update(
        Msg::Db(DbEvent::Lookup {
            id,
            key: k,
            result: Ok(vec![
                row([
                    "max_connections",
                    "100",
                    "configuration file",
                    "/pg/postgresql.conf",
                    "64",
                    "f",
                ]),
                row(["search_path", "", "default", "", "", "f"]),
                row([
                    "shared_buffers",
                    "128MB",
                    "configuration file",
                    "/pg/postgresql.conf",
                    "127",
                    "t",
                ]),
            ]),
        }),
        &Hits::default(),
    );
    let text = &a.tabs[0].view.shown;
    let version = text.find("--   Version:").unwrap();
    let config = text.find("-- Configuration (3 settings):").unwrap();
    assert!(version < config, "below the version:\n{text}");
    assert!(
        text.contains(
            "--   max_connections = 100    (configuration file: /pg/postgresql.conf:64)\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("--   search_path     = ''     (default)\n"),
        "{text}"
    );
    assert!(text.contains(
        "--   shared_buffers  = 128MB  (configuration file: /pg/postgresql.conf:127)  (restart needed)\n"
    ), "{text}");

    // Ctrl+R on the server row reads them again
    a.refresh();
    let ServerSettings::Loading(k2) = a.tree.servers[0].settings else {
        panic!("read again on refresh");
    };
    assert_ne!(k, k2);
    // an error is shown as comments
    a.update(
        Msg::Db(DbEvent::Lookup {
            id,
            key: k2,
            result: Err(DbError::Other("permission denied".into())),
        }),
        &Hits::default(),
    );
    assert!(
        a.tabs[0]
            .view
            .shown
            .contains("-- Configuration: cannot read it\n--   permission denied\n")
    );

    // disconnected: nothing to show, read again after the next connect
    for h in a.tree.by_uid(uid).unwrap().reset() {
        h.abort();
    }
    a.follow_selection();
    assert_eq!(a.tree.servers[0].settings, ServerSettings::NotLoaded);
    assert!(!a.tabs[0].view.shown.contains("Configuration"));
}

/// Server dashboard (D-057): opened once per server / database, refreshed by the
/// answers of its connection, cancel query after a question, closed with the tab.
#[tokio::test]
async fn dashboard_tab_refreshes_and_cancels() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "a");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    key(&mut a, "Home");
    assert!(a.is_enabled(Command::Dashboard));
    a.run(Command::Dashboard);
    assert_eq!(a.tabs.len(), 2);
    assert!(a.tabs[1].dashboard().is_some());
    assert_eq!(a.tabs[1].label(), "Dashboard: a");
    assert_eq!(a.context(), Some(crate::keymap::Context::Dashboard));
    // the same server again: the open tab
    a.focus = Focus::Tree;
    a.run(Command::Dashboard);
    assert_eq!(a.tabs.len(), 2);
    assert_eq!(a.active_tab, 1);
    let tab = a.tabs[1].id;

    // pretend the connection is open (a detached handle keeps the requests)
    let conn = {
        let d = a.tabs[1].dashboard_mut().unwrap();
        let c = d.conn.as_mut().unwrap();
        c.handle.abort();
        c.handle = pgc_db::ConnectionHandle::detached(c.id);
        c.connected = true;
        d.version = Some(pgc_core::ServerVersion(170002));
        c.id
    };
    a.dashboard_refresh(tab, true);
    let (stats, size, list) = {
        let d = a.tabs[1].dashboard().unwrap();
        (
            d.stats_key.unwrap(),
            d.size_key.unwrap(),
            d.list_key.unwrap().0,
        )
    };
    let stats_row = |t: &str| -> Vec<String> {
        let mut r: Vec<String> = ["3", "1", "2"].iter().map(|s| s.to_string()).collect();
        r.extend((0..9).map(|_| "100".to_owned()));
        r.push(t.to_owned());
        r
    };
    a.on_dashboard_event(
        conn,
        tab,
        DbEvent::Lookup {
            id: conn,
            key: stats,
            result: Ok(vec![stats_row("1000")]),
        },
    );
    assert!(a.effects.is_empty(), "the list is still on its way");
    a.on_dashboard_event(
        conn,
        tab,
        DbEvent::Lookup {
            id: conn,
            key: size,
            result: Ok(vec![vec!["10485760".into(), "1000".into()]]),
        },
    );
    assert!(a.effects.is_empty(), "the list is still on its way");
    let mut session = vec![String::new(); crate::dashboard::SESSION_QUERY_COLUMNS];
    session[0] = "4242".into();
    session[1] = "db".into();
    session[2] = "bob".into();
    session[5] = "active".into();
    a.on_dashboard_event(
        conn,
        tab,
        DbEvent::Lookup {
            id: conn,
            key: list,
            result: Ok(vec![session]),
        },
    );
    assert!(matches!(
        a.effects.last(),
        Some(Effect::Timer(Timer::Tab { tab: t, after, .. }))
            if *t == tab && *after == std::time::Duration::from_secs(2)
    ));
    let screen = screen_text(&a);
    assert!(screen.contains("Sessions (1)"), "{screen}");
    assert!(screen.contains("4242"), "{screen}");
    // the summary strip: counts from the old 13-column stats row, the size
    assert!(screen.contains("size 10 MB"), "{screen}");
    assert!(screen.contains("Workers"), "{screen}");
    // with the statement pane the graphs do not fit (they go first); without it they do
    assert!(!screen.contains("Transactions"), "{screen}");
    key(&mut a, "Ctrl+R");
    let screen = screen_text(&a);
    assert!(screen.contains("Transactions"), "{screen}");
    assert!(screen.contains("total 3"), "{screen}");
    key(&mut a, "Ctrl+R");

    // cancel query: asked first, No does nothing, Yes sends it
    a.run(Command::CancelBackend);
    let d = a.dialogs.last().unwrap();
    assert!(matches!(d.kind, DialogKind::ConfirmCancelBackend { .. }));
    assert!(d.widgets[0].label.text.contains("PID 4242 (bob on db)"));
    key(&mut a, "n");
    assert!(a.tabs[1].dashboard().unwrap().signals.pending.is_empty());
    a.run(Command::CancelBackend);
    key(&mut a, "y");
    let pending = a.tabs[1].dashboard().unwrap().signals.pending.clone();
    let [(k, s)] = pending.as_slice() else {
        panic!("one signal: {pending:?}");
    };
    let k = *k;
    assert_eq!(s.pid, "4242");
    a.on_dashboard_event(
        conn,
        tab,
        DbEvent::Lookup {
            id: conn,
            key: k,
            result: Ok(vec![vec!["true".into()]]),
        },
    );
    assert_eq!(a.status, "Cancel request sent to PID 4242 (bob on db)");

    // pause: no new timer after the next answers
    key(&mut a, "Ctrl+P");
    assert!(a.tabs[1].dashboard().unwrap().paused);
    // Alt+Left / Right in the list switch its lists (D-060), also in the F1 window
    let h = f1_text(&a);
    assert!(h.starts_with("Server dashboard\n"), "{h}");
    for want in [
        "Refresh the dashboard now",
        "Next dashboard list",
        "Switch between the object tree and the right frame",
    ] {
        assert!(h.contains(want), "{want} in {h}");
    }
    assert!(!h.contains("Next tab"), "Alt+Right is the lists here: {h}");
    // Insert tags (and moves down), Alt+T the duration mode, Esc clears the tags
    key(&mut a, "Insert");
    assert!(a.tabs[1].dashboard().unwrap().tags.contains("4242"));
    assert!(screen_text(&a).contains("1 tagged"));
    key(&mut a, "Alt+T");
    assert_eq!(
        a.tabs[1].dashboard().unwrap().mode,
        crate::dashboard::DurationMode::Transaction
    );
    esc(&mut a);
    assert!(a.tabs[1].dashboard().unwrap().tags.is_empty());
    key(&mut a, "Alt+Right");
    assert_eq!(
        a.tabs[1].dashboard().unwrap().list,
        crate::dashboard::List::Blocking
    );
    key(&mut a, "Alt+Left");
    assert_eq!(
        a.tabs[1].dashboard().unwrap().list,
        crate::dashboard::List::Sessions
    );
    assert_eq!(a.active_tab, 1);
    // closing the tab drops its connection
    key(&mut a, "Ctrl+W");
    assert_eq!(a.tabs.len(), 1);
    assert!(!a.connections.contains_key(&conn));
}

/// Sends answer `rows` for `key` of profiler `tab` on connection `conn`.
fn profiler_answer(a: &mut App, conn: ConnId, tab: u64, key: u64, rows: Vec<Vec<&str>>) {
    let rows = rows
        .into_iter()
        .map(|r| r.into_iter().map(str::to_owned).collect())
        .collect();
    a.on_profiler_event(
        conn,
        tab,
        DbEvent::Lookup {
            id: conn,
            key,
            result: Ok(rows),
        },
    );
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// SQL profiler (D-058): Start asks before changing the logging settings, reads the
/// log from the end of the current file, Stop and quitting put the settings back.
#[tokio::test]
async fn profiler_asks_reads_and_restores() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "a");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    key(&mut a, "Home");
    a.run(Command::SqlProfiler);
    assert!(a.tabs[1].profiler().is_some());
    assert_eq!(a.tabs[1].label(), "Profiler: a");
    assert_eq!(a.context(), Some(crate::keymap::Context::Profiler));
    let tab = a.tabs[1].id;
    let conn = {
        let p = a.tabs[1].profiler_mut().unwrap();
        let c = p.conn.as_mut().unwrap();
        c.handle.abort();
        c.handle = pgc_db::ConnectionHandle::detached(c.id);
        c.connected = true;
        p.version = Some(pgc_core::ServerVersion(170002));
        c.id
    };
    fn prof(a: &App) -> &crate::profiler::Profiler {
        a.tabs[1].profiler().unwrap()
    }

    // F5: check → question listing the changes
    key(&mut a, "F5");
    assert_eq!(prof(&a).phase, crate::profiler::Phase::Checking);
    let k = prof(&a).step_keys[0];
    profiler_answer(
        &mut a,
        conn,
        tab,
        k,
        vec![vec!["on", "stderr", "default", "-1", "default", "99"]],
    );
    let d = a.dialogs.last().unwrap();
    assert!(matches!(d.kind, DialogKind::ConfirmProfilerSettings { .. }));
    let text = &d.widgets[0].label.text;
    assert!(
        text.contains("log_destination = 'stderr,jsonlog'"),
        "{text}"
    );
    assert!(text.contains("log_min_duration_statement = '0'"), "{text}");
    // Esc = No: nothing changed
    esc(&mut a);
    assert_eq!(prof(&a).phase, crate::profiler::Phase::Stopped);
    assert!(!prof(&a).needs_restore());

    // again, Yes: two ALTER SYSTEM statements and the reload
    key(&mut a, "F5");
    let k = prof(&a).step_keys[0];
    profiler_answer(
        &mut a,
        conn,
        tab,
        k,
        vec![vec!["on", "stderr", "default", "-1", "default", "99"]],
    );
    key(&mut a, "y");
    let keys = prof(&a).step_keys.clone();
    assert_eq!(keys.len(), 3);
    for k in keys {
        profiler_answer(&mut a, conn, tab, k, vec![]);
    }
    assert_eq!(prof(&a).phase, crate::profiler::Phase::Running);
    assert!(prof(&a).needs_restore());

    // first read: only the end of the current file
    let k = prof(&a).read_key.unwrap();
    profiler_answer(
        &mut a,
        conn,
        tab,
        k,
        vec![vec!["log/p.json", "500", "", ""]],
    );
    assert_eq!(prof(&a).file.as_deref(), Some("log/p.json"));
    assert_eq!(prof(&a).offset, 500);
    assert!(matches!(
        a.effects.last(),
        Some(Effect::Timer(Timer::Tab { .. }))
    ));
    let Some(Effect::Timer(Timer::Tab { seq, .. })) = a.effects.pop() else {
        unreachable!()
    };
    a.update(Msg::TabTimer(tab, seq), &Hits::default());
    let line = |pid: &str, user: &str, sql: &str| {
        serde_json::json!({"user": user, "dbname": "shop", "pid": pid,
            "error_severity": "LOG", "message": format!("duration: 0.5 ms  statement: {sql}")})
        .to_string()
            + "\n"
    };
    let data = line("7", "bob", "SELECT 1")
        + &line("99", "me", "SELECT own")
        + &line("8", "eve", "SELECT 2");
    let k = prof(&a).read_key.unwrap();
    profiler_answer(
        &mut a,
        conn,
        tab,
        k,
        vec![vec!["log/p.json", "800", "800", &hex(&data)]],
    );
    assert_eq!(prof(&a).grid.rows.len(), 2, "its own PID 99 is left out");
    assert_eq!(prof(&a).offset, 500 + data.len() as u64);
    let screen = screen_text(&a);
    assert!(screen.contains("SELECT 2"), "{screen}");

    // the statement pane follows the list cursor (D-059); it is filled in `update`
    a.update(Msg::Tick, &Hits::default());
    assert_eq!(a.tabs[1].view.editor.text(), "SELECT 2");
    key(&mut a, "Up");
    a.update(Msg::Tick, &Hits::default());
    assert_eq!(a.tabs[1].view.editor.text(), "SELECT 1");
    // Alt+Down: list → pane (editor keys); F6 only tree ↔ right frame (D-060)
    assert_eq!(a.focus, Focus::Results);
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Tree);
    key(&mut a, "F6");
    assert_eq!(a.focus, Focus::Results);
    let h = f1_text(&a);
    assert!(h.starts_with("SQL profiler\n"), "{h}");
    for want in ["Start / stop the profiler", "Filter the profiler entries"] {
        assert!(h.contains(want), "list: {want} in {h}");
    }
    key(&mut a, "Alt+Down");
    assert_eq!(a.focus, Focus::Tabs);
    assert_eq!(a.context(), Some(crate::keymap::Context::Editor));
    let h = f1_text(&a);
    assert!(h.starts_with("Statement pane\n"), "{h}");
    for want in [
        "Find / replace",
        "Edit the shown definition",
        "Show / hide the results",
    ] {
        assert!(h.contains(want), "pane: {want} in {h}");
    }
    assert!(!h.contains("Paste"), "read only: {h}");
    // Ctrl+R hides the pane: the focus goes back to the list
    key(&mut a, "Ctrl+R");
    assert_eq!(a.focus, Focus::Results);
    assert!(!a.pane_shown());
    key(&mut a, "Ctrl+R");
    assert!(a.pane_shown());
    // Ctrl+E: the statement in a new query tab on the row's database
    key(&mut a, "Ctrl+E");
    assert_eq!(a.tabs.len(), 3);
    assert_eq!(a.tabs[2].view.editor.text(), "SELECT 1");
    assert_eq!(
        a.tabs[2].query().unwrap().binding,
        Some((uid, "shop".to_owned()))
    );
    let id = a.tabs[2].id;
    a.close_tab(id);
    a.active_tab = 1;
    a.focus = Focus::Results;

    // the filter dialog
    key(&mut a, "Ctrl+F");
    assert!(matches!(
        a.dialogs.last().unwrap().kind,
        DialogKind::ProfilerFilter { .. }
    ));
    typing(&mut a, "eve");
    key(&mut a, "Alt+O");
    assert_eq!(prof(&a).grid.rows.len(), 1);
    assert!(screen_text(&a).contains("Filter: user eve"));

    // quitting puts the settings back first
    key(&mut a, "F10");
    assert!(!a.should_quit());
    assert_eq!(prof(&a).phase, crate::profiler::Phase::Restoring);
    let keys = prof(&a).step_keys.clone();
    assert_eq!(keys.len(), 3, "RESET, RESET-or-SET, reload");
    for k in keys {
        assert!(!a.should_quit());
        profiler_answer(&mut a, conn, tab, k, vec![]);
    }
    assert!(a.should_quit());
    assert!(!prof(&a).needs_restore());
}

/// Direct keys for the frames (D-060): tree, right frame (the part last used), lower /
/// upper part; the second keys do the same.
#[test]
fn direct_keys_for_the_frames() {
    let (mut a, _d) = app();
    a.new_query_tab(None);
    a.query_mut().unwrap().results_visible = false;
    assert_eq!(a.focus, Focus::Tabs);
    key(&mut a, "Alt+1");
    assert_eq!(a.focus, Focus::Tree);
    key(&mut a, "Alt+2");
    assert_eq!(a.focus, Focus::Tabs, "at first the upper part");
    // the lower part is shown first when hidden
    key(&mut a, "Alt+Down");
    assert_eq!(a.focus, Focus::Results);
    assert!(a.query().unwrap().results_visible);
    // Alt+2 comes back to the part last used
    key(&mut a, "Alt+1");
    key(&mut a, "Alt+2");
    assert_eq!(a.focus, Focus::Results);
    key(&mut a, "Alt+Up");
    assert_eq!(a.focus, Focus::Tabs);
    key(&mut a, "Ctrl+Shift+Down");
    assert_eq!(a.focus, Focus::Results);
    key(&mut a, "Ctrl+Shift+Up");
    assert_eq!(a.focus, Focus::Tabs);
    key(&mut a, "Ctrl+1");
    assert_eq!(a.focus, Focus::Tree);
    // the tree has no tabs: Alt+Left / Right leave the tabs and the focus alone
    let tab = a.active_tab;
    key(&mut a, "Alt+Right");
    key(&mut a, "Alt+Left");
    key(&mut a, "Alt+Left");
    assert_eq!((a.active_tab, a.focus), (tab, Focus::Tree));
    // the menu still switches tabs from the tree
    a.run(Command::NextTab);
    assert_ne!(a.active_tab, tab);
    a.run(Command::PrevTab);
    key(&mut a, "Ctrl+2");
    assert_eq!(a.focus, Focus::Tabs);
    // without a result set only Messages is there, both ways (D-066)
    key(&mut a, "Alt+Down");
    assert!(a.query().unwrap().on_messages());
    key(&mut a, "Alt+Right");
    assert!(a.query().unwrap().on_messages());
    key(&mut a, "Alt+Left");
    assert!(a.query().unwrap().on_messages());
    key(&mut a, "Alt+M");
    assert!(a.query().unwrap().on_messages(), "Alt+M stays on Messages");
    key(&mut a, "Alt+Up");
    // in the results Alt+Left / Right go through Messages and the result sets (D-066)
    {
        let q = a.query_mut().unwrap();
        q.results = vec![
            crate::grid::ResultSet::new(Vec::new()),
            crate::grid::ResultSet::new(Vec::new()),
        ];
        q.active_result = 0;
        q.show_messages = false;
    }
    key(&mut a, "Alt+Down");
    key(&mut a, "Alt+Right");
    assert_eq!(a.query().unwrap().active_result, 1);
    key(&mut a, "Alt+Right");
    assert!(a.query().unwrap().show_messages);
    key(&mut a, "Alt+Right");
    assert!(!a.query().unwrap().show_messages);
    assert_eq!(a.query().unwrap().active_result, 0);
    key(&mut a, "Alt+Left");
    assert!(
        a.query().unwrap().show_messages,
        "backwards: Messages is before the first"
    );
    assert_eq!(a.active_tab, 1, "the main tabs stay");
    // from the editor they switch the main tabs; the default tab has no lower part
    key(&mut a, "Alt+Up");
    key(&mut a, "Alt+Left");
    assert_eq!(a.active_tab, 0);
    key(&mut a, "Alt+Down");
    assert_eq!(a.focus, Focus::Tabs);
    // the editor keeps Ctrl+Shift+Left / Right for word selection
    assert_eq!(
        a.keymap.lookup(
            crate::keymap::Context::Editor,
            KeyCombo::parse("Ctrl+Shift+Left").unwrap()
        ),
        Some(Command::SelectWordLeft)
    );
}

/// A connected query tab (no real connection behind it) on connection `conn`.
fn connected_tab(a: &mut App, conn: ConnId) -> u64 {
    a.new_query_tab(None);
    let tab = a.tabs[a.active_tab].id;
    let q = a.query_mut().unwrap();
    q.conn = Some(QueryConn {
        id: conn,
        database: "shop".into(),
        handle: pgc_db::ConnectionHandle::detached(conn),
        cancel: None,
        state: QueryConnState::Connected,
        ssl_mode: SslMode::default(),
        ssl_root_cert: None,
    });
    a.connections.insert(conn, Target::Query(tab));
    tab
}

/// The events that end the tab's running statement: an error or a command tag, then
/// the end of the run with an idle session.
fn end_run(a: &mut App, conn: ConnId, tab: u64, outcome: Result<&str, &str>) {
    let run = a.next_run;
    let event = match outcome {
        Ok(tag) => pgc_db::QueryEvent::Command {
            tag: tag.to_owned(),
        },
        Err(message) => pgc_db::QueryEvent::Error(pgc_db::ServerMessage {
            severity: "ERROR".into(),
            message: message.to_owned(),
            ..Default::default()
        }),
    };
    for event in [
        event,
        pgc_db::QueryEvent::Finished {
            elapsed: std::time::Duration::from_millis(1),
            status: TxStatus::Idle,
        },
    ] {
        a.on_query_event(
            conn,
            tab,
            DbEvent::Query {
                id: conn,
                run,
                event,
            },
        );
    }
}

/// "Commit" when a tab closes (D-019): the tab closes only when the server answered
/// COMMIT. A failed transaction answers ROLLBACK and a deferred constraint fails the
/// COMMIT itself: the tab then stays, with the reason in Messages.
#[tokio::test]
async fn closing_with_commit_needs_a_real_commit() {
    let (mut a, _d) = app();
    let tab = connected_tab(&mut a, 99);
    let commit_on_close = |a: &mut App| {
        a.query_mut().unwrap().tx = TxStatus::InTransaction;
        a.run(Command::CloseTab);
        assert!(matches!(
            a.dialogs.last().unwrap().kind,
            DialogKind::CloseTransaction { .. }
        ));
        key(a, "m");
        assert!(a.dialogs.is_empty());
        assert!(a.query().unwrap().busy(), "COMMIT was sent");
    };

    // the transaction had failed: COMMIT rolls it back
    commit_on_close(&mut a);
    end_run(&mut a, 99, tab, Ok("ROLLBACK"));
    assert!(a.tab_index(tab).is_some(), "the tab stays");
    assert!(a.status.contains("not committed"), "{}", a.status);
    assert!(a.query().unwrap().on_messages());

    // the COMMIT itself fails (a deferred constraint)
    commit_on_close(&mut a);
    end_run(&mut a, 99, tab, Err("violates foreign key constraint"));
    assert!(a.tab_index(tab).is_some(), "the tab stays");
    assert!(a.status.contains("not committed"), "{}", a.status);

    // committed: the tab closes
    commit_on_close(&mut a);
    end_run(&mut a, 99, tab, Ok("COMMIT"));
    assert!(a.tab_index(tab).is_none());
}

/// "Commit all" when quitting (D-043): a transaction that was not committed stops
/// the quit; so does a connection lost before the COMMIT was answered.
#[tokio::test]
async fn quitting_with_commit_all_needs_real_commits() {
    let (mut a, _d) = app();
    let tab = connected_tab(&mut a, 99);
    let commit_all = |a: &mut App| {
        a.query_mut().unwrap().tx = TxStatus::InTransaction;
        key(a, "F10");
        assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::QuitTransactions);
        key(a, "m");
        assert!(a.dialogs.is_empty());
        assert!(!a.should_quit(), "waits for the COMMIT");
    };

    commit_all(&mut a);
    end_run(&mut a, 99, tab, Ok("ROLLBACK"));
    assert!(!a.should_quit());
    assert_eq!(a.closing.bulk_pending, None);
    assert!(a.status.contains("not committed"), "{}", a.status);

    // committed: quits
    commit_all(&mut a);
    end_run(&mut a, 99, tab, Ok("COMMIT"));
    assert!(a.should_quit());

    // the connection is lost while the COMMIT is on its way
    let (mut a, _d) = app();
    let tab = connected_tab(&mut a, 99);
    commit_all(&mut a);
    a.on_query_event(
        99,
        tab,
        DbEvent::Closed {
            id: 99,
            error: Some(DbError::Other("server closed the connection".into())),
        },
    );
    assert!(!a.should_quit());
    assert_eq!(a.closing.bulk_pending, None);
    let q = a.query().unwrap();
    assert!(q.conn.is_none() && !q.busy());
    // nothing is open any more: F10 quits
    key(&mut a, "F10");
    assert!(a.should_quit());
}

/// An export that still needs rows from the paused result ends when the tab's
/// connection is lost: the incomplete file is deleted and the tab is free again
/// (D-042).
#[tokio::test]
async fn export_ends_when_the_connection_is_lost() {
    let (mut a, dir, rx) = export_app();
    let tab = connected_tab(&mut a, 99);
    a.query_mut()
        .unwrap()
        .start_run(1, Sent::new(0, "select 1"));
    a.next_run = 1;
    for event in [
        pgc_db::QueryEvent::ResultStart {
            columns: vec![pgc_db::Column {
                name: "id".into(),
                type_oid: 23,
            }],
            title: None,
        },
        pgc_db::QueryEvent::Rows {
            rows: vec![vec![Some("1".into())]; 2],
            more: true,
        },
    ] {
        a.on_query_event(
            99,
            tab,
            DbEvent::Query {
                id: 99,
                run: 1,
                event,
            },
        );
    }
    assert!(a.query().unwrap().paused());

    let path = dir.path().join("out.csv");
    a.start_export(
        path.clone(),
        crate::export::ExportOptions {
            format: ExportFormat::Csv,
            separator: pgc_core::settings::CsvSeparator::Comma,
            bom: false,
        },
    );
    // the rows of the grid are written; the next batch is asked from the stream
    let written = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("export event");
    a.update(Msg::Export(written), &Hits::default());
    assert!(a.query().unwrap().export.as_ref().unwrap().awaiting_rows);

    a.on_query_event(
        99,
        tab,
        DbEvent::Closed {
            id: 99,
            error: Some(DbError::Other("server closed the connection".into())),
        },
    );
    finish_export(&mut a, &rx);
    let q = a.query().unwrap();
    assert!(q.export.is_none());
    assert!(!q.paused(), "no more rows are offered");
    assert!(
        q.messages
            .iter()
            .any(|m| m.text.contains("The connection was lost during the export")),
        "{:?}",
        q.messages
    );
    assert!(!path.exists(), "the incomplete file is deleted");
}

/// A UTF-8 byte order mark at the start of an SQL file is not part of the text sent
/// to the server.
#[test]
fn opening_a_file_drops_the_byte_order_mark() {
    let (mut a, dir) = app();
    let path = dir.path().join("bom.sql");
    std::fs::write(&path, "\u{feff}select 1;\n").unwrap();
    a.open_path(path.clone());
    assert_eq!(a.editor().text(), "select 1;\n");
    assert!(!a.editor().modified());
    assert_eq!(a.query().unwrap().file, Some(path));
}

/// Quitting while the profiler cannot put the logging settings back (D-058): the
/// quit stops and the message with the statements to run is shown.
#[tokio::test]
async fn profiler_restore_failure_stops_the_quit() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "a");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    key(&mut a, "Home");
    a.run(Command::SqlProfiler);
    let tab = a.tabs[1].id;
    let conn = {
        let p = a.tabs[1].profiler_mut().unwrap();
        let c = p.conn.as_mut().unwrap();
        c.handle.abort();
        c.handle = pgc_db::ConnectionHandle::detached(c.id);
        c.connected = true;
        p.version = Some(pgc_core::ServerVersion(170002));
        c.id
    };
    fn prof(a: &App) -> &crate::profiler::Profiler {
        a.tabs[1].profiler().unwrap()
    }
    // start: check, Yes, the settings are changed
    key(&mut a, "F5");
    let k = prof(&a).step_keys[0];
    profiler_answer(
        &mut a,
        conn,
        tab,
        k,
        vec![vec!["on", "stderr", "default", "-1", "default", "99"]],
    );
    key(&mut a, "y");
    for k in prof(&a).step_keys.clone() {
        profiler_answer(&mut a, conn, tab, k, vec![]);
    }
    assert!(prof(&a).needs_restore());

    // F10: the first statement of the restore fails
    key(&mut a, "F10");
    assert_eq!(prof(&a).phase, crate::profiler::Phase::Restoring);
    let keys = prof(&a).step_keys.clone();
    a.on_profiler_event(
        conn,
        tab,
        DbEvent::Lookup {
            id: conn,
            key: keys[0],
            result: Err(DbError::Other("ERROR: permission denied".into())),
        },
    );
    for k in &keys[1..] {
        profiler_answer(&mut a, conn, tab, *k, vec![]);
    }
    assert!(!a.should_quit(), "the message must be seen");
    let d = a.dialogs.last().expect("the message");
    let text = &d.widgets[0].label.text;
    assert!(text.contains("could not be put back"), "{text}");
    assert!(
        text.contains("ALTER SYSTEM RESET log_destination"),
        "{text}"
    );
    assert!(text.contains("pg_reload_conf()"), "{text}");
    // read, then F10 quits
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty());
    key(&mut a, "F10");
    assert!(a.should_quit());
}

/// Quitting ends every open connection (D-071): the tabs give up theirs and the
/// handles are returned for the short wait.
#[tokio::test]
async fn quitting_ends_the_open_connections() {
    let (mut a, _d) = app();
    let uid = add_server(&mut a, "a");
    let id = fake_connecting(&mut a, uid);
    a.tree.by_uid(uid).unwrap().handle = Some(pgc_db::ConnectionHandle::detached(id));
    connected_tab(&mut a, 99);
    let handles = a.end_connections();
    assert_eq!(handles.len(), 2, "the server's and the tab's");
    assert!(a.query().unwrap().conn.is_none());
    assert!(a.connections.is_empty());
    assert_eq!(a.tree.by_uid(uid).unwrap().state, ConnState::Disconnected);
}

/// Saving in the background (D-072): the text is taken when the save starts, the tab
/// stays editable and keeps its "modified" mark when it changed meanwhile, and a
/// second save waits for the first.
#[tokio::test]
async fn saving_in_the_background() {
    let (mut a, dir) = app();
    let (tx, rx) = std::sync::mpsc::channel();
    a.set_file_sink(Arc::new(move |e| {
        let _ = tx.send(e);
    }));
    let next = |a: &mut App| {
        let e = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("file event");
        a.update(Msg::File(e), &Hits::default());
    };
    a.new_query_tab(None);
    typing(&mut a, "one");
    let path = dir.path().join("q.sql");
    a.save_to(path.clone());
    assert!(a.query().unwrap().saving);
    assert!(a.status.starts_with("Saving "), "{}", a.status);
    assert!(a.editor().modified(), "not saved yet");
    // edited while the file is written, and saved again: the second save waits
    typing(&mut a, " two");
    a.save_to(path.clone());
    assert_eq!(a.query().unwrap().save_next, Some(path.clone()));

    next(&mut a);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
    assert!(a.editor().modified(), "the text changed since");
    assert!(a.query().unwrap().saving, "the waiting save runs now");
    assert_eq!(a.query().unwrap().file, Some(path.clone()));

    next(&mut a);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one two");
    assert!(!a.editor().modified());
    assert!(!a.query().unwrap().saving);
    assert!(a.status.starts_with("Saved "), "{}", a.status);

    // opening: the tab appears when the file is read
    let tabs = a.tabs.len();
    a.open_path(path.clone());
    assert_eq!(a.tabs.len(), tabs);
    assert!(a.status.starts_with("Opening "), "{}", a.status);
    next(&mut a);
    assert_eq!(a.tabs.len(), tabs + 1);
    assert_eq!(a.editor().text(), "one two");
}

/// A save that fails stops the closing or quitting that waited for it (D-072).
#[test]
fn a_failed_save_stops_closing() {
    let (mut a, dir) = app();
    a.new_query_tab(None);
    typing(&mut a, "select 1;");
    let tab = a.tabs[a.active_tab].id;
    a.closing.after_save = Some(AfterSave::Close(tab));
    a.save_to(dir.path().join("missing").join("q.sql"));
    assert!(a.tab_index(tab).is_some(), "the tab stays");
    assert_eq!(a.closing.after_save, None);
    assert!(a.editor().modified());
    assert!(!a.query().unwrap().saving);
    let d = a.dialogs.last().expect("the error");
    assert!(d.widgets[0].label.text.contains("Cannot save"));
}

/// A connected server `a` with a (detached) tree connection: (uid, connection id).
fn connected_server(a: &mut App) -> (u64, ConnId) {
    let uid = add_server(a, "a");
    let conn = fake_connecting(a, uid);
    a.tree.by_uid(uid).unwrap().handle = Some(pgc_db::ConnectionHandle::detached(conn));
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    (uid, conn)
}

/// The answer to a lookup on a server's tree connection.
fn server_answer(a: &mut App, conn: ConnId, key: u64, result: Result<Vec<Vec<&str>>, &str>) {
    let result = result
        .map(|rows| {
            rows.into_iter()
                .map(|r| r.into_iter().map(str::to_owned).collect())
                .collect()
        })
        .map_err(|e| DbError::Other(e.to_owned()));
    a.on_db(DbEvent::Lookup {
        id: conn,
        key,
        result,
    });
}

/// The logging settings the profiler changed, as it notes them.
fn profiler_changes() -> Vec<crate::profiler::Change> {
    vec![
        crate::profiler::Change {
            name: "log_destination",
            value: "stderr,jsonlog".into(),
            restore: None,
        },
        crate::profiler::Change {
            name: "log_min_duration_statement",
            value: "0".into(),
            restore: Some("250".into()),
        },
    ]
}

/// Settings the profiler left changed (D-058): when the server connects and they are
/// still changed, pgtc asks; Yes puts them back and the note goes.
#[tokio::test]
async fn profiler_settings_are_offered_for_restore_on_connect() {
    let (mut a, _d) = app();
    let note = a.paths.profiler_restore_file();
    let (uid, conn) = connected_server(&mut a);
    assert!(a.dialogs.is_empty(), "no note, no question");
    a.record_profiler_changes(uid, &profiler_changes());
    assert!(note.exists());
    let still = vec!["on", "stderr,jsonlog", "f", "0", "f", "7"];

    // the server connects (again): the current settings are read, then the question
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    let k = a.next_key;
    server_answer(&mut a, conn, k, Ok(vec![still.clone()]));
    let d = a.dialogs.last().expect("the question");
    assert_eq!(d.kind, DialogKind::ConfirmProfilerRestore { server: uid });
    let text = d.widgets[0].label.text.clone();
    assert!(
        text.contains("log_min_duration_statement = '0'  (before: '250')"),
        "{text}"
    );
    assert!(text.contains("(before: not set)"), "{text}");
    assert!(text.contains("profiler_restore.toml"), "{text}");

    // Not now: the note stays
    key_n(&mut a);
    assert!(a.dialogs.is_empty());
    assert!(note.exists());

    // again, Yes: two statements and the reload; a failure keeps the note
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    let k = a.next_key;
    server_answer(&mut a, conn, k, Ok(vec![still.clone()]));
    key(&mut a, "y");
    let last = a.next_key;
    server_answer(&mut a, conn, last - 2, Err("ERROR: permission denied"));
    server_answer(&mut a, conn, last - 1, Ok(vec![]));
    assert!(a.dialogs.is_empty(), "not finished yet");
    server_answer(&mut a, conn, last, Ok(vec![]));
    let d = a.dialogs.last().expect("the error");
    assert!(d.widgets[0].label.text.contains("could not be put back"));
    assert!(note.exists());
    key(&mut a, "Enter");

    // again, Yes, and it works: the note goes
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    let k = a.next_key;
    server_answer(&mut a, conn, k, Ok(vec![still.clone()]));
    key(&mut a, "y");
    let last = a.next_key;
    for k in last - 2..=last {
        server_answer(&mut a, conn, k, Ok(vec![]));
    }
    assert!(a.dialogs.is_empty());
    assert_eq!(a.status, "Logging settings restored");
    assert!(!note.exists());
}

fn key_n(a: &mut App) {
    key(a, "n");
}

/// No question when the settings are no longer what the profiler set (the note is
/// dropped), and Forget drops it without a restore (D-058).
#[tokio::test]
async fn profiler_note_is_dropped_when_nothing_is_changed_or_on_forget() {
    let (mut a, _d) = app();
    let note = a.paths.profiler_restore_file();
    let (uid, conn) = connected_server(&mut a);
    a.record_profiler_changes(uid, &profiler_changes());
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    let k = a.next_key;
    server_answer(
        &mut a,
        conn,
        k,
        Ok(vec![vec!["on", "stderr", "f", "250", "f", "7"]]),
    );
    assert!(a.dialogs.is_empty());
    assert!(!note.exists());

    // only one of them is still changed: only that one is asked about
    a.record_profiler_changes(uid, &profiler_changes());
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    let k = a.next_key;
    server_answer(
        &mut a,
        conn,
        k,
        Ok(vec![vec!["on", "stderr", "f", "0", "f", "7"]]),
    );
    let text = a.dialogs.last().unwrap().widgets[0].label.text.clone();
    assert!(text.contains("log_min_duration_statement"), "{text}");
    assert!(!text.contains("log_destination"), "{text}");
    // Forget: nothing is restored
    key(&mut a, "f");
    assert!(a.dialogs.is_empty());
    assert_ne!(a.status, "Restoring logging settings…");
    assert!(!note.exists());

    // a running profiler of this pgtc is not "left changed"
    a.record_profiler_changes(uid, &profiler_changes());
    key(&mut a, "Home");
    a.run(Command::SqlProfiler);
    a.tabs[1].profiler_mut().unwrap().applied = profiler_changes();
    let before = a.next_key;
    a.check_profiler_restore(uid);
    assert_eq!(a.next_key, before, "nothing is read");
}

/// Start notes the changed settings; Stop removes the note once they are back
/// (D-058).
#[tokio::test]
async fn profiler_notes_its_changes_until_they_are_restored() {
    let (mut a, _d) = app();
    let note = a.paths.profiler_restore_file();
    let uid = add_server(&mut a, "a");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    key(&mut a, "Home");
    a.run(Command::SqlProfiler);
    let tab = a.tabs[1].id;
    let conn = {
        let p = a.tabs[1].profiler_mut().unwrap();
        let c = p.conn.as_mut().unwrap();
        c.handle.abort();
        c.handle = pgc_db::ConnectionHandle::detached(c.id);
        c.connected = true;
        p.version = Some(pgc_core::ServerVersion(170002));
        c.id
    };
    fn prof(a: &App) -> &crate::profiler::Profiler {
        a.tabs[1].profiler().unwrap()
    }
    key(&mut a, "F5");
    let k = prof(&a).step_keys[0];
    profiler_answer(
        &mut a,
        conn,
        tab,
        k,
        vec![vec!["on", "stderr", "default", "-1", "default", "99"]],
    );
    assert!(!note.exists(), "nothing changed yet");
    key(&mut a, "y");
    for k in prof(&a).step_keys.clone() {
        profiler_answer(&mut a, conn, tab, k, vec![]);
    }
    let text = std::fs::read_to_string(&note).unwrap();
    assert!(text.contains("log_min_duration_statement"), "{text}");
    assert!(text.contains("host = \"localhost\""), "{text}");

    // F5 stops: restored, the note goes
    key(&mut a, "F5");
    for k in prof(&a).step_keys.clone() {
        profiler_answer(&mut a, conn, tab, k, vec![]);
    }
    assert!(!prof(&a).needs_restore());
    assert!(!note.exists());
}

/// File dialogs read nothing themselves (D-072): the dialog opens with *Reading…*,
/// the listing and the places follow, and a chosen path is checked in the background
/// while the dialog stays.
#[tokio::test]
async fn file_dialogs_list_and_check_in_the_background() {
    let (mut a, dir) = app();
    let (tx, rx) = std::sync::mpsc::channel();
    a.set_file_sink(Arc::new(move |e| {
        let _ = tx.send(e);
    }));
    let next = |a: &mut App| {
        let e = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("file event");
        a.update(Msg::File(e), &Hits::default());
    };
    let rows = |a: &App| -> String {
        let d = a.dialogs.last().unwrap();
        let mut d = d.clone();
        let mut out = Vec::new();
        loop {
            let row = d.selected_item("files").unwrap_or("").to_owned();
            if out.last() == Some(&row) {
                break;
            }
            out.push(row);
            d.list_step("files", 1);
        }
        out.join("|")
    };
    let files = dir.path().join("sql");
    std::fs::create_dir_all(files.join("sub")).unwrap();
    std::fs::write(files.join("a.sql"), "select 1;").unwrap();
    a.files.dir = files.clone();
    let sep = std::path::MAIN_SEPARATOR;

    // the dialog is there at once; its listing is not
    a.open_file_dialog(false);
    assert_eq!(a.dialogs.last().unwrap().kind, DialogKind::OpenFile);
    assert_eq!(rows(&a), "Reading…");
    // a resize now fits the list to its one row; the listing gets the height back
    let height = a.dialogs.last().unwrap().list_rows_of("files");
    a.dialogs
        .last_mut()
        .unwrap()
        .fit_text(Rect::new(0, 0, 100, 40));
    assert_eq!(a.dialogs.last().unwrap().list_rows_of("files"), 1);
    // nothing to choose yet
    key(&mut a, "Enter");
    assert_eq!(a.dialogs.len(), 1);
    assert_eq!(a.tabs.len(), 1);
    next(&mut a);
    assert_eq!(rows(&a), format!("..{sep}|sub{sep}|a.sql"));
    assert_eq!(a.dialogs.last().unwrap().list_rows_of("files"), height);

    // a folder: the dialog stays while it is checked, then shows the folder
    key(&mut a, "Down");
    key(&mut a, "Enter");
    assert_eq!(a.files.dir, files, "not known to be a folder yet");
    assert_eq!(a.dialogs.len(), 1);
    next(&mut a);
    assert_eq!(a.files.dir, files.join("sub"));
    assert_eq!(rows(&a), "Reading…");
    next(&mut a);
    assert_eq!(rows(&a), format!("..{sep}"));

    // the places are read the same way
    key(&mut a, "Alt+P");
    assert!(a.files.places.as_ref().is_some_and(Vec::is_empty));
    assert_eq!(rows(&a), "Reading…");
    next(&mut a);
    assert!(a.files.places.as_ref().is_some_and(|p| !p.is_empty()));
    assert!(rows(&a).starts_with(&format!("..{sep}|")), "{}", rows(&a));

    // closed before the answer: the answer is not waited for
    esc(&mut a);
    a.files.dir = files.clone();
    a.open_file_dialog(false);
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    next(&mut a);
    assert!(a.dialogs.is_empty());

    // a typed path in a folder that is not there: checked, then the error
    a.open_file_dialog(true);
    next(&mut a);
    for _ in 0.."query.sql".len() {
        key(&mut a, "Backspace");
    }
    typing(&mut a, "nowhere/x.sql");
    key(&mut a, "Enter");
    assert_eq!(a.dialogs.len(), 1, "still being checked");
    next(&mut a);
    assert_eq!(a.dialogs.len(), 2);
    let text = a.dialogs.last().unwrap().widgets[0].label.text.clone();
    assert!(text.contains("does not exist"), "{text}");
}
