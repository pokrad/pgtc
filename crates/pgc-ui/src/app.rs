//! Central application state and the `update` function (Elm style). Input events,
//! database events and timers arrive as [`Msg`]s; `update` is the only place that
//! mutates state.

use std::collections::HashMap;
use std::time::Duration;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use pgc_core::paths::{self, Paths};
use pgc_core::settings::{CsvSeparator, ExportFormat, SavedConnection, Settings, SslMode};
use pgc_db::conninfo::{PasswordPrompt, Resolved};
use pgc_db::pgpass::{self, PgPassKey};
use pgc_db::{
    ConnId, ConnectParams, DbError, DbEvent, EventSink, ExecuteRequest, QueryEvent, TxStatus,
};
use pgc_editor::{Action, Editor, Motion, SearchOptions};
use pgc_sql::{RenderContext, SqlCatalog};
use std::path::Path;

use ratatui::layout::{Position, Rect};

use crate::clipboard::Clipboard;
use crate::dialog::{Dialog, DialogEvent, DialogKind, Widget};
use crate::export::{ExportEvent, ExportOptions, ExportSink, ToWriter};
use crate::keymap::{Command, Context, Keymap};
use crate::keys::KeyCombo;
use crate::menu::{self, MenuDef, MenuEntry, MenuEvent, MenuState, Popup};
use crate::objects::ObjType;
use crate::query_tab::{AfterRun, ExportRun, LineMap, QueryConn, QueryConnState, QueryState, Sent};
use crate::setup;
use crate::template_dialog::{Choice, FormEvent, TemplateForm};
use crate::theme::{Theme, ThemeSet};
use crate::tree::{ConnState, DbConn, RowKind, Tree};
use crate::view::Hits;
use pgc_sql::devtemplate::{self, ParamType, ParamValue, QualName, TemplateLibrary};

/// Messages processed by [`App::update`].
#[derive(Debug)]
pub enum Msg {
    Input(Event),
    /// The `Esc` + digit window with this sequence number expired (D-032).
    EscTimeout(u64),
    /// The tree cursor rested long enough: load the default tab (D-037).
    DefinitionTimer(u64),
    /// Time for another slice of the editors' background lexer scan (D-038).
    ScanTick,
    /// Periodic tick while a query runs (elapsed time, cancel wait, D-041).
    Tick,
    Db(DbEvent),
    /// From a result export's writer thread (D-042).
    Export(ExportEvent),
}

/// Side effects requested by `update`, carried out by the event loop.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Send `Msg::EscTimeout(seq)` after the Esc timeout.
    StartEscTimer(u64),
    /// Send `Msg::DefinitionTimer(seq)` after the definition delay.
    StartDefinitionTimer(u64),
    /// Send `Msg::ScanTick` soon.
    ScanTick,
    /// Send `Msg::Tick` after the tick interval.
    Tick,
    /// Put this text into the terminal's clipboard with OSC 52 (D-023).
    Osc52(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    /// The editor / text view of the active tab.
    Tabs,
    /// The results pane of the active query tab (D-041).
    Results,
}

/// Kinds of tabs in the right pane (D-009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabKind {
    /// The default object tab; always present, cannot be closed; follows the tree.
    Default,
    /// An object opened with "Show in new tab" (F3).
    Object,
    /// Query tab number `n`.
    Query(u32),
}

/// Content of a tab: an editor (D-038), read-only for object views (D-037).
#[derive(Debug)]
pub struct TextView {
    /// Shown as the pane title (object name).
    pub title: String,
    pub editor: Editor,
    pub loading: bool,
    /// Last text loaded with `set_text` (identical text is not reloaded).
    shown: String,
}

impl TextView {
    pub fn new(read_only: bool, tab_width: usize) -> Self {
        Self {
            title: String::new(),
            editor: Editor::new("", read_only, tab_width),
            loading: false,
            shown: String::new(),
        }
    }

    /// Shows `text` under `title` (cursor to the top when the text changes).
    fn set_text(&mut self, title: String, text: String) {
        if self.title != title || self.shown != text {
            self.editor.set_text(&text);
            self.shown = text;
        }
        self.title = title;
        self.loading = false;
    }

    /// Replaces the text, keeping the title.
    fn load(&mut self, text: String) {
        let title = std::mem::take(&mut self.title);
        self.set_text(title, text);
    }

    pub fn is_empty(&self) -> bool {
        self.editor.is_empty()
    }
}

#[derive(Debug)]
pub struct Tab {
    /// Stable id (definition answers are matched by it).
    pub id: u64,
    pub kind: TabKind,
    /// Object tabs: the server whose object they show (closed with it, D-043); the
    /// default tab: the server of the selected tree row.
    pub server: Option<u64>,
    /// Object and default tabs: the database of the shown object, for the tab status.
    pub database: Option<String>,
    pub view: TextView,
    /// Query tabs: connection, results, messages (D-019).
    pub query: Option<QueryState>,
}

impl Tab {
    /// Label in the tab strip.
    pub fn label(&self) -> String {
        match &self.kind {
            TabKind::Default => "Default".to_owned(),
            TabKind::Object => self.view.title.clone(),
            TabKind::Query(n) => {
                let q = self.query.as_ref();
                let name = q
                    .and_then(|q| q.file.as_ref())
                    .and_then(|f| f.file_name())
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("Query {n}"));
                format!(
                    "{name}{}",
                    q.map(QueryState::label_suffix).unwrap_or_default()
                )
            }
        }
    }
}

/// What a connection id belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Server(u64),
    /// Server uid, database node id.
    Database(u64, u64),
    /// Query tab id (D-011: one connection per query tab).
    Query(u64),
    /// A definition export / load; the job is in `App::jobs` (D-048).
    Structure,
}

/// A definition export or load running on its own connection (D-048).
struct StructureJob {
    /// Server uid and database name.
    uid: u64,
    database: String,
    kind: JobKind,
    handle: pgc_db::ConnectionHandle,
}

enum JobKind {
    Export(std::path::PathBuf),
    /// The definition read from the file, compared with the live database.
    Load(Box<pgc_model::Database>),
}

pub struct App {
    pub paths: Paths,
    pub settings: Settings,
    pub keymap: Keymap,
    pub themes: ThemeSet,
    pub theme: Theme,
    pub menus: Vec<MenuDef>,
    pub menu: Option<MenuState>,
    pub popup: Option<Popup>,
    pub dialogs: Vec<Dialog>,
    pub focus: Focus,
    pub tree: Tree,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub status: String,
    /// Quick search text while searching (D-036).
    pub search: Option<String>,
    /// The open autocomplete list (D-052).
    pub(crate) completion: Option<crate::completion::Popup>,
    /// F4 list: input text and selected row last seen, to tell typing from choosing
    /// (D-054).
    psql_seen: (String, usize),
    sql: SqlCatalog,
    /// Tree queries in flight: request key → group node id.
    pending: HashMap<u64, u64>,
    /// Definition queries in flight: request key → tab id.
    pending_defs: HashMap<u64, u64>,
    /// Latest definition request of the default tab (older answers are dropped).
    default_key: u64,
    /// Row the default tab shows; `def_seq` invalidates older timers.
    followed: Option<RowKind>,
    def_seq: u64,
    next_tab: u64,
    clipboard: Clipboard,
    /// Effects produced by commands, returned from the next `update`.
    effects: Vec<Effect>,
    next_run: u64,
    tick_scheduled: bool,
    /// Grid area of the active query tab (rows incl. header, columns).
    grid_size: (usize, usize),
    /// Directory shown in the file dialog; a save waiting for overwrite confirmation.
    file_dir: std::path::PathBuf,
    pending_save: Option<std::path::PathBuf>,
    /// What to do once a "Save changes?" save has succeeded (D-019).
    after_save: Option<AfterSave>,
    /// Quitting: tabs whose unsaved text the user chose not to save (D-019).
    /// Quitting / disconnecting (D-043): what, and the tabs whose unsaved text the
    /// user chose not to save.
    bulk: Option<(Bulk, Vec<u64>)>,
    /// The editor / results split is being dragged with the mouse (D-019).
    split_drag: bool,
    /// The tree / tabs border is being dragged with the mouse (D-045).
    pane_drag: bool,
    /// Developer templates (D-046): the open form, the picker's state, lookups on
    /// their way (key → what they fill) and the row being edited (list field, row).
    pub template: Option<TemplateForm>,
    picker: Option<Picker>,
    template_lookups: HashMap<u64, LookupTarget>,
    row_choices: HashMap<(String, String), Vec<Choice>>,
    row_edit: Option<(usize, Option<usize>)>,
    /// Result export (D-042): events from writer threads, last id, a target waiting
    /// for overwrite confirmation, grid area (anchor of the results context menu).
    export_sink: Option<ExportSink>,
    next_export: u64,
    pending_export: Option<(std::path::PathBuf, ExportOptions)>,
    /// Database (server uid, name) of an open definition file dialog, and a path
    /// waiting for overwrite confirmation (D-048).
    definition_target: Option<(u64, String)>,
    pending_definition: Option<std::path::PathBuf>,
    jobs: HashMap<ConnId, StructureJob>,
    grid_area: Rect,
    /// Quitting / disconnecting waits for COMMITs to finish.
    bulk_pending: Option<Bulk>,
    /// Last find options and replacement (F3, D-038).
    last_search: Option<(SearchOptions, String)>,
    /// A scan tick is already scheduled.
    scan_scheduled: bool,
    /// Text area of the active editor (for mouse clicks).
    editor_area: Rect,
    next_key: u64,
    sink: EventSink,
    connections: HashMap<ConnId, Target>,
    next_conn: ConnId,
    tree_height: usize,
    /// The whole screen of the last frame (sizes the Help windows).
    pub(crate) screen: Rect,
    next_query: u32,
    quit: bool,
    esc_seq: u64,
    esc_pending: bool,
}

const ESC: KeyEvent = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
/// Lines lexed per background scan tick (D-038).
const SCAN_BUDGET_LINES: usize = 100_000;
const CONN_LABEL_WIDTH: u16 = 19;

impl App {
    /// Builds the app: loads colour schemes and `keymap.toml`, creates the tree of saved
    /// connections, and adds/connects the command-line connection (D-031).
    pub fn new(
        paths: Paths,
        settings: Settings,
        sink: EventSink,
        startup: Option<Resolved>,
    ) -> Self {
        let mut problems = Vec::new();

        let (themes, warnings) = ThemeSet::load(&paths.user_themes_dir());
        for w in &warnings {
            tracing::warn!("colour scheme: {w}");
        }
        if !warnings.is_empty() {
            problems.push("colour schemes");
        }

        let mut keymap = Keymap::default();
        let keymap_file = paths.keymap_file();
        match std::fs::read_to_string(&keymap_file) {
            Ok(text) => match keymap.apply_overrides(&text) {
                Ok(warnings) => {
                    for w in &warnings {
                        tracing::warn!("{}: {w}", keymap_file.display());
                    }
                    if !warnings.is_empty() {
                        problems.push("keymap.toml");
                    }
                }
                Err(e) => {
                    tracing::warn!("{}: {e}", keymap_file.display());
                    problems.push("keymap.toml");
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!("{}: {e}", keymap_file.display());
                problems.push("keymap.toml");
            }
        }

        let mut theme = themes.get(&settings.ui.color_scheme).clone();
        theme.ascii = settings.ui.ascii_frames;
        let status = if problems.is_empty() {
            "Ready".to_owned()
        } else {
            format!("Problems in {} — see the log file", problems.join(", "))
        };
        let tree = Tree::new(&settings.servers);
        let settings_clipboard = settings.clipboard.clone();
        let tab_width = settings.editor.tab_width as usize;

        let mut app = Self {
            paths,
            settings,
            keymap,
            themes,
            theme,
            menus: menu::menu_bar(),
            menu: None,
            popup: None,
            dialogs: Vec::new(),
            focus: Focus::Tree,
            tree,
            tabs: vec![Tab {
                id: 0,
                kind: TabKind::Default,
                server: None,
                database: None,
                view: TextView::new(true, tab_width),
                query: None,
            }],
            active_tab: 0,
            status,
            search: None,
            completion: None,
            psql_seen: (String::new(), 0),
            sql: SqlCatalog::new(),
            pending: HashMap::new(),
            pending_defs: HashMap::new(),
            default_key: 0,
            followed: None,
            def_seq: 0,
            next_tab: 0,
            clipboard: Clipboard::new(&settings_clipboard),
            effects: Vec::new(),
            next_run: 0,
            tick_scheduled: false,
            grid_size: (0, 0),
            file_dir: std::env::current_dir().unwrap_or_default(),
            pending_save: None,
            after_save: None,
            bulk: None,
            split_drag: false,
            pane_drag: false,
            template: None,
            picker: None,
            template_lookups: HashMap::new(),
            row_choices: HashMap::new(),
            row_edit: None,
            export_sink: None,
            next_export: 0,
            pending_export: None,
            definition_target: None,
            pending_definition: None,
            jobs: HashMap::new(),
            grid_area: Rect::default(),
            bulk_pending: None,
            last_search: None,
            scan_scheduled: false,
            editor_area: Rect::default(),
            next_key: 0,
            sink,
            connections: HashMap::new(),
            next_conn: 0,
            tree_height: 0,
            screen: Rect::default(),
            next_query: 0,
            quit: false,
            esc_seq: 0,
            esc_pending: false,
        };
        if let Some(resolved) = startup {
            app.startup_connection(resolved);
        }
        app
    }

    /// D-031: reuse an identical saved connection or save a new one, then connect.
    fn startup_connection(&mut self, resolved: Resolved) {
        let existing = self
            .tree
            .servers
            .iter()
            .find(|s| s.conn.same_target(&resolved.connection))
            .map(|s| s.uid);
        let uid = match existing {
            Some(uid) => uid,
            None => {
                let uid = self.tree.add(resolved.connection);
                self.persist_servers();
                uid
            }
        };
        let prompt = resolved.password_prompt;
        if let Some(node) = self.tree.by_uid(uid) {
            if resolved.password.is_some() {
                node.password = resolved.password;
            }
            node.never_prompt = prompt == PasswordPrompt::Never;
        }
        self.tree.select_server(uid);
        if prompt == PasswordPrompt::Always {
            self.open_login(uid, None);
        } else {
            self.connect_server(uid);
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// True while `Esc` waits for a digit.
    pub fn esc_pending(&self) -> bool {
        self.esc_pending
    }

    fn modal(&self) -> bool {
        !self.dialogs.is_empty()
            || self.menu.is_some()
            || self.popup.is_some()
            || self.template.is_some()
    }

    /// Keymap context of the focused panel (`None` = only `[global]`).
    pub fn context(&self) -> Option<Context> {
        match self.focus {
            Focus::Tree => Some(Context::Tree),
            Focus::Tabs => Some(Context::Editor),
            Focus::Results => Some(Context::Results),
        }
    }

    /// Command bound to `key` for the current state.
    pub fn command_for(&self, key: KeyCombo) -> Option<Command> {
        if self.modal() {
            // Modal: dialog keys, plus the global keys that make sense there.
            return self.keymap.lookup_only(Context::Dialog, key).or_else(|| {
                self.keymap
                    .lookup_only(Context::Global, key)
                    .filter(|c| matches!(c, Command::Help | Command::Menu | Command::Quit))
            });
        }
        match self.context() {
            Some(ctx) => self.keymap.lookup(ctx, key),
            None => self.keymap.lookup_only(Context::Global, key),
        }
    }

    /// Whether a command can run now (menu items of unimplemented features are off).
    pub fn is_enabled(&self, cmd: Command) -> bool {
        let server = self.tree.selected_server();
        match cmd {
            Command::Quit | Command::Settings | Command::NewConnection => true,
            // a database or an object below it, on a connected server (D-049)
            Command::NewQuery | Command::OpenFile => self.query_target().is_some(),
            Command::SaveFile => self.query().is_some(),
            Command::NextTab | Command::PrevTab => self.tabs.len() > 1,
            Command::CloseTab => self.active_tab != 0,
            Command::Connect => server.is_some_and(|s| s.state == ConnState::Disconnected),
            Command::Disconnect => server.is_some_and(|s| s.state != ConnState::Disconnected),
            Command::EditConnection | Command::DeleteConnection => server.is_some(),
            Command::RefreshTree => server.is_some_and(|s| s.state == ConnState::Connected),
            Command::ShowInNewTab => self.tree.selected().is_some(),
            Command::ToggleSystemObjects => true,
            Command::Execute => self
                .query()
                .is_some_and(|q| !q.busy() && q.export.is_none()),
            Command::CancelQuery => self
                .query()
                .is_some_and(|q| q.running.is_some() || q.export.is_some()),
            Command::FetchAll => self
                .query()
                .is_some_and(|q| q.paused() && q.export.is_none()),
            Command::NewFromTemplate => self.query().is_some(),
            Command::ExportDefinition | Command::LoadDefinition => {
                self.selected_database().is_some()
            }
            Command::InstallTemplates | Command::ShowTemplatesFolder | Command::RunSetup => true,
            Command::Help
            | Command::KeyboardShortcuts
            | Command::ThirdPartyLicences
            | Command::About => true,
            Command::ExportResult => self.query().is_some_and(|q| {
                !q.busy() && q.export.is_none() && q.results.get(q.active_result).is_some()
            }),
            Command::Commit | Command::Rollback => {
                self.query().is_some_and(|q| q.connected() && !q.busy())
            }
            Command::ToggleAutoCommit
            | Command::ToggleAutoRollback
            | Command::TogglePlpgsql
            | Command::ToggleResults
            | Command::ShowMessages => self.query().is_some(),
            Command::NextResult | Command::PrevResult => {
                self.query().is_some_and(|q| q.results.len() > 1)
            }
            Command::Psql => self.query().is_some_and(|q| q.connected() && !q.busy()),
            _ => false,
        }
    }

    /// On/off state shown as `[x]` / `[ ]` in front of a menu item.
    pub fn checked(&self, cmd: Command) -> Option<bool> {
        match cmd {
            Command::ToggleSystemObjects => Some(self.settings.ui.show_system_objects),
            Command::ToggleAutoCommit => self.query().map(|q| q.auto_commit),
            Command::ToggleAutoRollback => self.query().map(|q| q.auto_rollback),
            Command::TogglePlpgsql => self.query().map(|q| q.plpgsql),
            _ => None,
        }
    }

    /// Shortcut shown next to a menu item.
    pub fn shortcut_text(&self, cmd: Command) -> Option<String> {
        self.keymap
            .shortcut(
                &[
                    Context::Global,
                    Context::Tree,
                    Context::Editor,
                    Context::Results,
                ],
                cmd,
            )
            .map(|k| k.to_string())
    }

    // ---- update -------------------------------------------------------------------

    pub fn update(&mut self, msg: Msg, hits: &Hits) -> Vec<Effect> {
        self.tree_height = hits.tree.height.saturating_sub(2) as usize;
        // (an empty screen: no frame drawn yet)
        if !hits.screen.is_empty() && hits.screen != self.screen {
            self.screen = hits.screen;
            for d in &mut self.dialogs {
                d.fit_text(hits.screen);
            }
        }
        self.grid_size = (hits.grid.height as usize, hits.grid.width as usize);
        self.grid_area = hits.grid;
        // The active editor learns its size from the last frame.
        self.editor_area = hits.editor;
        if hits.editor.width > 0 {
            let line_numbers = self.settings.editor.line_numbers;
            let editor = &mut self.tabs[self.active_tab].view.editor;
            let gutter = pgc_editor::gutter_width(editor, line_numbers);
            editor.set_view(
                hits.editor.height as usize,
                hits.editor.width.saturating_sub(gutter) as usize,
            );
        }
        let mut effects = match msg {
            Msg::Input(Event::Key(ev)) if ev.kind != KeyEventKind::Release => self.on_key(ev),
            Msg::Input(Event::Mouse(m)) => {
                self.on_mouse(m, hits);
                Vec::new()
            }
            Msg::Input(Event::Paste(text)) => {
                if let Some(d) = self.dialogs.last_mut() {
                    d.paste(&text);
                } else if let Some(form) = self.template.as_mut() {
                    form.paste(&text);
                } else if self.focus == Focus::Tabs && !self.modal() {
                    self.edit(Action::Insert(text));
                }
                Vec::new()
            }
            Msg::Input(_) => Vec::new(),
            Msg::EscTimeout(seq) => {
                if self.esc_pending && seq == self.esc_seq {
                    self.esc_pending = false;
                    self.dispatch(ESC);
                }
                Vec::new()
            }
            Msg::ScanTick => {
                self.scan_scheduled = false;
                for tab in &self.tabs {
                    if tab.view.editor.needs_scan() {
                        tab.view.editor.background_scan(SCAN_BUDGET_LINES);
                    }
                }
                Vec::new()
            }
            Msg::Tick => {
                self.tick_scheduled = false;
                self.check_cancel_wait();
                Vec::new()
            }
            Msg::DefinitionTimer(seq) => {
                if seq == self.def_seq
                    && let Some(RowKind::Node(nid)) = self.followed
                {
                    self.request_definition(0, nid);
                }
                Vec::new()
            }
            Msg::Db(event) => {
                let selected = self.tree.selected();
                self.on_db(event);
                if let Some(row) = selected {
                    self.tree.select_kind(row.kind);
                }
                Vec::new()
            }
            Msg::Export(event) => {
                self.on_export_event(event);
                Vec::new()
            }
        };
        self.sync_export_dialog();
        self.sync_picker();
        self.sync_psql_dialog();
        if let Some(form) = self.template.as_mut() {
            form.set_screen(hits.screen);
        }
        self.tree.clamp();
        self.tree.ensure_visible(self.tree_height);
        effects.extend(self.follow_selection());
        effects.append(&mut self.effects);
        if !self.scan_scheduled && self.tabs.iter().any(|t| t.view.editor.needs_scan()) {
            self.scan_scheduled = true;
            effects.push(Effect::ScanTick);
        }
        let running = self
            .tabs
            .iter()
            .any(|t| t.query.as_ref().is_some_and(QueryState::busy));
        if running && !self.tick_scheduled {
            self.tick_scheduled = true;
            effects.push(Effect::Tick);
        }
        if self.focus == Focus::Results && self.query().is_none_or(|q| !q.results_visible) {
            self.focus = Focus::Tabs;
        }
        // the list belongs to the editor of the active query tab (D-052)
        if self
            .completion
            .as_ref()
            .is_some_and(|p| p.tab != self.tabs[self.active_tab].id)
            || self.focus != Focus::Tabs
            || self.modal()
        {
            self.completion = None;
        }
        effects
    }

    fn on_key(&mut self, ev: KeyEvent) -> Vec<Effect> {
        let combo = KeyCombo::from_event(&ev);
        if self.esc_pending {
            self.esc_pending = false;
            if combo.mods.is_empty()
                && let KeyCode::Char(d @ '0'..='9') = combo.code
            {
                let n = if d == '0' { 10 } else { d as u8 - b'0' };
                self.dispatch(KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE));
                return Vec::new();
            }
            self.dispatch(ESC);
            if self.quit {
                return Vec::new();
            }
        }
        if combo == KeyCombo::plain(KeyCode::Esc) && self.completion.is_some() {
            // closes the list at once, without the Esc + digit wait (D-052)
            self.completion = None;
            return Vec::new();
        }
        if combo == KeyCombo::plain(KeyCode::Esc) {
            self.esc_seq += 1;
            self.esc_pending = true;
            return vec![Effect::StartEscTimer(self.esc_seq)];
        }
        self.dispatch(ev);
        Vec::new()
    }

    /// Handles one (already Esc-resolved) key.
    fn dispatch(&mut self, ev: KeyEvent) {
        let combo = KeyCombo::from_event(&ev);
        let cmd = self.command_for(combo);

        if let Some(dialog) = self.dialogs.last_mut() {
            if cmd == Some(Command::Help) {
                self.run(Command::Help);
                return;
            }
            // F4 list: arrows and pages move in the list while typing in the input
            if dialog.kind == DialogKind::PsqlCommand {
                let page = dialog.list_rows_of("list").max(1) as isize;
                let step = match cmd {
                    Some(Command::Up) => Some(-1),
                    Some(Command::Down) => Some(1),
                    Some(Command::PageUp) => Some(-page),
                    Some(Command::PageDown) => Some(page),
                    _ => None,
                };
                if let Some(step) = step {
                    dialog.list_step("list", step);
                    self.sync_psql_dialog();
                    return;
                }
            }
            let event = dialog.handle_key(cmd, &ev);
            self.dialog_event(event);
            return;
        }

        if let Some(form) = self.template.as_mut() {
            // an SQL box gets the editor's keys (D-046)
            let action = if form.editing_sql() {
                self.keymap
                    .lookup_only(Context::Editor, combo)
                    .and_then(editor_action)
            } else {
                None
            };
            let event = form.handle_key(cmd, action, &ev);
            self.form_event(event);
            return;
        }

        if let Some(mut popup) = self.popup.take() {
            match popup.handle_key(cmd, &ev, &|c| self.is_enabled(c)) {
                MenuEvent::None => self.popup = Some(popup),
                MenuEvent::Close => {}
                MenuEvent::Run(c) => self.run(c),
            }
            return;
        }

        if let Some(mut state) = self.menu {
            let event = state.handle_key(&self.menus, cmd, &ev, &|c| self.is_enabled(c));
            match event {
                MenuEvent::None => self.menu = Some(state),
                MenuEvent::Close => self.menu = None,
                MenuEvent::Run(c) => {
                    self.menu = None;
                    self.run(c);
                }
            }
            return;
        }

        if self.search.is_some() && self.search_key(&ev, combo, cmd) {
            return;
        }

        if self.completion.is_some() && self.completion_key(&ev, cmd) {
            return;
        }

        if let Some(cmd) = cmd {
            self.run(cmd);
        } else if let KeyCode::Char(c) = ev.code
            && self.focus == Focus::Tabs
            && !ev
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.edit(Action::Insert(c.to_string()));
            self.after_typing(c);
        }
    }

    // ---- psql-style commands (D-054) -------------------------------------------------

    /// F4: the window with every command (D-054).
    fn open_psql_dialog(&mut self, text: &str) {
        if !self.is_enabled(Command::Psql) {
            self.status = "psql commands need a connected, idle query tab".to_owned();
            return;
        }
        let d = self.psql_dialog(text);
        self.dialogs.push(d);
    }

    /// The F4 window: input, then the list of all commands.
    fn psql_dialog(&mut self, text: &str) -> Dialog {
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
    fn sync_psql_dialog(&mut self) {
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
    fn run_psql(&mut self, mut dialog: Dialog, text: &str) {
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
    fn conninfo(&mut self) {
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
    fn generate_loaded(
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

    // ---- autocomplete (D-052) -----------------------------------------------------------

    /// Keys while the list is open; true when the key was used. Typed characters go
    /// on into the editor (and refilter the list afterwards).
    fn completion_key(&mut self, ev: &KeyEvent, cmd: Option<Command>) -> bool {
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
    fn after_typing(&mut self, c: char) {
        if self.completion.is_some() {
            self.refresh_completion();
        } else if c == '.' || c == '_' || c.is_alphanumeric() {
            self.open_completion(false);
        }
    }

    /// `Ctrl+Space` / `Alt+/` (`manual`) or automatic: opens the list at the cursor.
    fn open_completion(&mut self, manual: bool) {
        use crate::completion::{AUTO_LETTERS, Popup, build};
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
        let built = build(&a, &q.names, pgc_editor::lexer::keywords());
        let needs = built.needs.clone();
        if built.items.is_empty() && !built.loading {
            if manual {
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
        use crate::completion::build;
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
        let built = build(&a, &q.names, pgc_editor::lexer::keywords());
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
    fn names_loaded(
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

    /// Quick search keys (D-036). Returns true if the key was used by the search.
    fn search_key(&mut self, ev: &KeyEvent, combo: KeyCombo, cmd: Option<Command>) -> bool {
        if let KeyCode::Char(c) = ev.code
            && !ev
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            if let Some(text) = &mut self.search {
                text.push(c);
            }
            self.search_from(self.tree.cursor);
            return true;
        }
        if cmd == Some(Command::QuickSearch) {
            self.search_from(self.tree.cursor + 1);
            return true;
        }
        match self.keymap.lookup_only(Context::Dialog, combo) {
            Some(Command::Backspace) => {
                if let Some(text) = &mut self.search {
                    text.pop();
                }
                self.search_from(self.tree.cursor);
                true
            }
            Some(Command::Cancel | Command::Accept) => {
                self.end_search();
                true
            }
            _ => {
                self.end_search();
                false
            }
        }
    }

    /// Moves the cursor to the first row from `start` (wrapping) whose name starts with
    /// the search text.
    fn search_from(&mut self, start: usize) {
        let Some(text) = self.search.clone() else {
            return;
        };
        let needle = text.to_lowercase();
        let rows = self.tree.rows();
        let n = rows.len();
        let found = (0..n).map(|k| (start + k) % n).find(|&i| {
            self.tree
                .row_name(rows[i])
                .to_lowercase()
                .starts_with(&needle)
        });
        if let Some(i) = found {
            self.tree.cursor = i;
        }
        self.status = if found.is_some() || text.is_empty() {
            format!("Quick search: {text}")
        } else {
            format!("Quick search: {text}  (not found)")
        };
    }

    fn end_search(&mut self) {
        self.search = None;
        self.status.clear();
    }

    /// Executes a command.
    pub fn run(&mut self, cmd: Command) {
        // keys reach here without the menus' enabled check (D-049)
        if matches!(
            cmd,
            Command::NewQuery | Command::OpenFile | Command::SaveFile
        ) && !self.modal()
            && !self.is_enabled(cmd)
        {
            self.status = if cmd == Command::SaveFile {
                "Only a query tab can be saved".to_owned()
            } else {
                "Select a database (or an object in it) of a connected server first".to_owned()
            };
            return;
        }
        let in_tree = self.focus == Focus::Tree && !self.modal();
        let in_view = self.focus == Focus::Tabs && !self.modal();
        let in_results = self.focus == Focus::Results && !self.modal();
        match cmd {
            Command::Quit => self.request_quit(),
            Command::Menu => self.open_menu(0),
            // D-041: F6 / Ctrl+Tab cycle editor → results → tree → editor
            Command::SwitchPanel => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Tabs,
                    Focus::Tabs if self.query().is_some_and(|q| q.results_visible) => {
                        Focus::Results
                    }
                    Focus::Tabs | Focus::Results => Focus::Tree,
                }
            }
            c if in_results && self.results_command(c) => {}
            Command::Execute => self.execute(),
            Command::CancelQuery => self.cancel_query(),
            Command::FetchAll => self.fetch_all(),
            Command::Commit => self.send_simple("COMMIT", None),
            Command::Rollback => self.send_simple("ROLLBACK", None),
            Command::ToggleAutoCommit => self.toggle_query(|q| {
                q.auto_commit = !q.auto_commit;
                format!("Auto commit {}", on_off(q.auto_commit))
            }),
            Command::ToggleAutoRollback => self.toggle_query(|q| {
                q.auto_rollback = !q.auto_rollback;
                format!("Auto rollback on error {}", on_off(q.auto_rollback))
            }),
            Command::TogglePlpgsql => self.toggle_query(|q| {
                q.plpgsql = !q.plpgsql;
                format!("PL/pgSQL mode {}", on_off(q.plpgsql))
            }),
            Command::ToggleResults => self.toggle_query(|q| {
                q.results_visible = !q.results_visible;
                String::new()
            }),
            Command::ShowMessages => self.toggle_query(|q| {
                q.show_messages = !q.show_messages;
                q.results_visible = true;
                String::new()
            }),
            Command::NextResult | Command::PrevResult => {
                let next = cmd == Command::NextResult;
                self.toggle_query(|q| {
                    let n = q.results.len();
                    if n > 0 {
                        q.show_messages = false;
                        q.active_result = if next {
                            (q.active_result + 1) % n
                        } else {
                            (q.active_result + n - 1) % n
                        };
                    }
                    String::new()
                });
            }
            Command::ResultsSmaller | Command::ResultsLarger => {
                let delta: i16 = if cmd == Command::ResultsLarger { 5 } else { -5 };
                self.toggle_query(|q| {
                    q.results_percent = (q.results_percent as i16 + delta).clamp(15, 85) as u16;
                    String::new()
                });
            }
            Command::OpenFile => self.open_file_dialog(false),
            Command::SaveFile => self.open_file_dialog(true),
            Command::NextTab => self.active_tab = (self.active_tab + 1) % self.tabs.len(),
            Command::PrevTab => {
                self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
            }
            Command::CloseTab => {
                if self.active_tab == 0 {
                    self.status = "The default tab cannot be closed".to_owned();
                } else {
                    let id = self.tabs[self.active_tab].id;
                    self.request_close_tab(id, true);
                }
            }
            Command::NewQuery => self.new_query_tab(None),
            Command::Settings => self.open_settings(),
            Command::NewConnection => self.open_connection_dialog(None),
            Command::EditConnection => {
                if let Some(uid) = self.tree.selected_server().map(|s| s.uid) {
                    self.open_connection_dialog(Some(uid));
                }
            }
            Command::DeleteConnection => self.confirm_delete(),
            Command::DropObject
                if self
                    .tree
                    .selected()
                    .is_some_and(|r| r.kind == RowKind::Server) =>
            {
                self.confirm_delete();
            }
            Command::Connect => {
                if let Some(uid) = self.tree.selected_server().map(|s| s.uid) {
                    self.connect_server(uid);
                }
            }
            Command::Disconnect => {
                if let Some(uid) = self.tree.selected_server().map(|s| s.uid) {
                    self.request_bulk(Bulk::Disconnect(uid));
                }
            }
            Command::ContextMenu if self.focus == Focus::Results => {
                let a = self.grid_area;
                self.open_results_menu((a.x + 2, a.y + 1));
            }
            Command::ContextMenu if !self.tree.servers.is_empty() => self.open_context_menu(),
            Command::ExportResult => self.open_export_dialog(None, None),
            Command::NewFromTemplate => self.open_template_picker(),
            Command::ExportDefinition => {
                if let Some(target) = self.selected_database() {
                    let initial = format!("{}.pgdef", target.1);
                    self.definition_target = Some(target);
                    self.open_definition_dialog(true, &initial);
                }
            }
            Command::LoadDefinition => {
                if let Some(target) = self.selected_database() {
                    self.definition_target = Some(target);
                    self.open_definition_dialog(false, "");
                }
            }
            Command::InstallTemplates => {
                let steps = vec![setup::templates_step(&self.paths)];
                self.show_steps("Install templates", steps)
            }
            Command::RunSetup => {
                let steps = setup::run_setup(&self.paths);
                self.show_steps("Run setup", steps)
            }
            Command::ShowTemplatesFolder => self.show_templates_folder(),
            Command::Help => self.open_shortcuts(true),
            Command::KeyboardShortcuts => self.open_shortcuts(false),
            Command::ThirdPartyLicences => self.open_licences(),
            Command::About => self.open_about(),
            Command::RefreshTree => self.refresh(),
            Command::ShowInNewTab => self.show_in_new_tab(),
            Command::Complete if in_view => self.open_completion(true),
            Command::Psql => self.open_psql_dialog(""),
            c if in_view && editor_action(c).is_some() => {
                if let Some(action) = editor_action(c) {
                    self.edit(action);
                }
            }
            Command::Copy if in_view => self.copy(false),
            Command::Cut if in_view => self.copy(true),
            Command::Paste if in_view => {
                let text = self.clipboard.paste();
                if !text.is_empty() {
                    self.edit(Action::Insert(text));
                }
            }
            Command::Find if in_view => self.open_find(),
            Command::FindNext if in_view => self.find_again(true),
            Command::FindPrev if in_view => self.find_again(false),
            Command::ToggleSystemObjects => self.toggle_system_objects(),
            Command::QuickSearch if in_tree && !self.tree.servers.is_empty() => {
                self.search = Some(String::new());
                self.status = "Quick search: ".to_owned();
            }
            Command::Up if in_tree => self.tree.move_cursor(-1),
            Command::Down if in_tree => self.tree.move_cursor(1),
            Command::PageUp if in_tree => self.tree.move_cursor(-(self.page() as isize)),
            Command::PageDown if in_tree => self.tree.move_cursor(self.page() as isize),
            Command::Home if in_tree => self.tree.move_cursor(isize::MIN / 2),
            Command::End if in_tree => self.tree.move_cursor(isize::MAX / 2),
            Command::Right if in_tree => self.expand(),
            Command::Left if in_tree => self.collapse(),
            Command::Accept if in_tree => self.toggle(),
            Command::Up
            | Command::Down
            | Command::PageUp
            | Command::PageDown
            | Command::Home
            | Command::End
            | Command::Left
            | Command::Right
            | Command::DocStart
            | Command::DocEnd
            | Command::Accept => {}
            c if editor_action(c).is_some() => {}
            other => {
                self.status = format!("'{}' is not available yet", other.name().replace('_', " "));
            }
        }
    }

    fn page(&self) -> usize {
        self.tree_height.max(2) - 1
    }

    fn open_menu(&mut self, index: usize) {
        self.menu = Some(MenuState::open(&self.menus, index, &|c| self.is_enabled(c)));
    }

    // ---- tree ---------------------------------------------------------------------

    fn expand(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        match row.kind {
            RowKind::Server => {
                let s = &mut self.tree.servers[row.server];
                match s.state {
                    ConnState::Disconnected => {
                        let uid = s.uid;
                        self.connect_server(uid);
                    }
                    ConnState::Connected => s.expanded = true,
                    ConnState::Connecting => {}
                }
            }
            RowKind::Node(nid) => self.expand_node(nid),
            RowKind::Loading(_) => {}
        }
    }

    fn expand_node(&mut self, nid: u64) {
        let Some(node) = self.tree.node_mut(nid) else {
            return;
        };
        if !node.expandable() {
            return;
        }
        node.expanded = true;
        if node.is_database() {
            if node
                .conn
                .as_ref()
                .is_none_or(|c| c.state == ConnState::Disconnected)
            {
                self.connect_database(nid);
            }
        } else if node.is_group() && node.children.is_none() && !node.loading {
            self.load_group(nid);
        }
    }

    fn collapse(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        let was_expanded = match row.kind {
            RowKind::Server => {
                let s = &mut self.tree.servers[row.server];
                std::mem::replace(&mut s.expanded, false) && s.state == ConnState::Connected
            }
            RowKind::Node(nid) => self
                .tree
                .node_mut(nid)
                .is_some_and(|n| std::mem::replace(&mut n.expanded, false)),
            RowKind::Loading(_) => false,
        };
        if !was_expanded {
            self.tree.select_parent();
        }
    }

    fn toggle(&mut self) {
        let expanded = match self.tree.selected().map(|r| r.kind) {
            Some(RowKind::Server) => self
                .tree
                .selected_server()
                .is_some_and(|s| s.expanded && s.state == ConnState::Connected),
            Some(RowKind::Node(nid)) => self.tree.node(nid).is_some_and(|n| n.expanded),
            _ => return,
        };
        if expanded {
            self.collapse();
        } else {
            self.expand();
        }
    }

    /// Starts loading the children of group `nid` (D-036).
    fn load_group(&mut self, nid: u64) {
        let Some(ctx) = self.tree.load_context(nid) else {
            return;
        };
        let server = &self.tree.servers[ctx.server];
        let Some(version) = server.version else {
            return;
        };
        let handle = match ctx.database {
            Some(db) => self
                .tree
                .node(db)
                .and_then(|n| n.conn.as_ref())
                .filter(|c| c.state == ConnState::Connected)
                .and_then(|c| c.handle.clone()),
            None => server.handle.clone(),
        };
        let Some(handle) = handle else {
            return;
        };
        let render = RenderContext {
            version,
            show_system: self.settings.ui.show_system_objects,
        };
        let sql = match self.sql.render(ctx.obj.folder(), "nodes.sql", render) {
            Ok(sql) => sql,
            Err(e) => {
                tracing::error!("{e}");
                self.error(&e.to_string());
                return;
            }
        };
        let parent = if ctx.obj.takes_parent() {
            ctx.parent_oid
        } else {
            None
        };
        self.next_key += 1;
        self.pending.insert(self.next_key, nid);
        if let Some(n) = self.tree.node_mut(nid) {
            n.loading = true;
        }
        handle.load_nodes(self.next_key, sql, parent);
    }

    /// `Ctrl+R`: reloads the loaded groups below the selected row (D-036).
    fn refresh(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        if self.tree.servers[row.server].state != ConnState::Connected {
            return;
        }
        let groups = self.tree.loaded_groups(row);
        for g in groups {
            self.load_group(g);
        }
        // autocomplete reads the names again (D-052)
        for q in self.tabs.iter_mut().filter_map(|t| t.query.as_mut()) {
            q.names.clear();
        }
        self.status = "Refreshed".to_owned();
        self.followed = None;
    }

    /// `Alt+.`: shows / hides system objects everywhere and saves the setting (D-036).
    fn toggle_system_objects(&mut self) {
        let show = !self.settings.ui.show_system_objects;
        self.settings.ui.show_system_objects = show;
        self.save_settings();
        for g in self.tree.all_loaded_groups() {
            self.load_group(g);
        }
        self.status = if show {
            "System objects shown"
        } else {
            "System objects hidden"
        }
        .to_owned();
    }

    fn open_context_menu(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        // items that do not apply to the row kind are left out (D-049)
        let in_database = match row.kind {
            RowKind::Node(nid) | RowKind::Loading(nid) => self.tree.database_name_of(nid).is_some(),
            RowKind::Server => false,
        };
        let entries = match row.kind {
            RowKind::Server => vec![
                menu::item("&Connect", Command::Connect),
                menu::item("&Disconnect", Command::Disconnect),
                MenuEntry::Separator,
                menu::item("&Refresh", Command::RefreshTree),
                MenuEntry::Separator,
                menu::item("&Edit connection…", Command::EditConnection),
                menu::item("De&lete connection", Command::DeleteConnection),
            ],
            _ if in_database => {
                // "Show in new tab" is F3 only, not in context menus (D-049)
                let mut entries = vec![
                    menu::item("New &query", Command::NewQuery),
                    menu::item("&Open SQL file…", Command::OpenFile),
                    MenuEntry::Separator,
                    menu::item("&Refresh", Command::RefreshTree),
                ];
                // database definition export / load (D-048)
                if self.selected_database().is_some() {
                    entries.extend([
                        MenuEntry::Separator,
                        menu::item("&Export database definition…", Command::ExportDefinition),
                        menu::item("&Load database definition…", Command::LoadDefinition),
                    ]);
                }
                entries
            }
            // server-level groups and objects (Databases, roles, tablespaces, …)
            _ => vec![menu::item("&Refresh", Command::RefreshTree)],
        };
        let anchor = (
            2,
            (self.tree.cursor.saturating_sub(self.tree.offset) + 3) as u16,
        );
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }

    /// Context menu of the results pane (D-042).
    fn open_results_menu(&mut self, anchor: (u16, u16)) {
        let entries = vec![
            menu::item("E&xport result…", Command::ExportResult),
            menu::item("&Fetch all", Command::FetchAll),
        ];
        self.popup = Some(Popup::new(entries, anchor, &|c| self.is_enabled(c)));
    }

    // ---- object view (D-037) -------------------------------------------------------

    /// Node id of the selected row if it is an object (not a group).
    fn selected_object(&self) -> Option<u64> {
        match self.tree.selected()?.kind {
            RowKind::Node(nid) if self.tree.node(nid).is_some_and(|n| !n.is_group()) => Some(nid),
            _ => None,
        }
    }

    fn add_tab(&mut self, kind: TabKind, view: TextView) -> u64 {
        self.next_tab += 1;
        let id = self.next_tab;
        let query = matches!(kind, TabKind::Query(_)).then(|| {
            QueryState::new(
                self.settings.query.auto_commit,
                self.settings.query.auto_rollback,
            )
        });
        self.tabs.push(Tab {
            id,
            kind,
            server: None,
            database: None,
            view,
            query,
        });
        self.active_tab = self.tabs.len() - 1;
        self.focus = Focus::Tabs;
        id
    }

    fn tab_mut(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Keeps the default tab on the selected tree row: info text at once, object
    /// definitions after the cursor has rested (200 ms).
    fn follow_selection(&mut self) -> Vec<Effect> {
        let (server, database) = self.selected_place();
        self.tabs[0].server = server;
        self.tabs[0].database = database;
        let selected = self.tree.selected();
        let kind = selected.map(|r| r.kind);
        let changed = kind != self.followed;
        self.followed = kind;
        match (selected, self.selected_object()) {
            (_, Some(nid)) if changed => {
                let title = self
                    .tree
                    .node(nid)
                    .map(|n| n.label.clone())
                    .unwrap_or_default();
                let view = &mut self.tabs[0].view;
                if view.title != title {
                    view.set_text(title, String::new());
                }
                view.loading = true;
                self.def_seq += 1;
                vec![Effect::StartDefinitionTimer(self.def_seq)]
            }
            (_, Some(_)) => Vec::new(),
            (Some(row), None) => {
                // Info text is cheap: rebuild it on every update (counts, states).
                if changed {
                    self.def_seq += 1;
                }
                let (title, text) = self.info_text(row);
                self.tabs[0].view.set_text(title, text);
                Vec::new()
            }
            (None, None) if changed => {
                self.tabs[0].view.set_text(String::new(), String::new());
                Vec::new()
            }
            (None, None) => Vec::new(),
        }
    }

    /// Info text for rows that are not objects (D-037), as SQL comments.
    fn info_text(&self, row: crate::tree::Row) -> (String, String) {
        match row.kind {
            RowKind::Server => {
                let s = &self.tree.servers[row.server];
                let state = match s.state {
                    ConnState::Connected => "connected",
                    ConnState::Connecting => "connecting",
                    ConnState::Disconnected => "disconnected",
                };
                let mut text = format!(
                    "-- Server: {}\n--\n--   Host:            {}:{}\n--   Maintenance DB:  {}\n\
                     --   User name:       {}\n--   SSL mode:        {}\n--   State:           {state}\n",
                    s.conn.name,
                    s.conn.host,
                    s.conn.port,
                    s.conn.database,
                    s.conn.username,
                    s.conn.ssl_mode.name()
                );
                if let Some(v) = s.version {
                    text.push_str(&format!("--   Version:         PostgreSQL {v}\n"));
                }
                (s.conn.name.clone(), text)
            }
            RowKind::Node(nid) => {
                let Some(n) = self.tree.node(nid) else {
                    return (String::new(), String::new());
                };
                let text = match &n.children {
                    Some(children) => {
                        let mut t = format!("-- {} ({})\n", n.label, children.len());
                        if !children.is_empty() {
                            t.push_str("--\n");
                        }
                        for c in children {
                            t.push_str(&format!("--   {}\n", c.label));
                        }
                        t
                    }
                    None if n.loading => format!("-- {}: loading…\n", n.label),
                    None => format!("-- {}: not loaded yet (expand to load)\n", n.label),
                };
                (n.label.clone(), text)
            }
            RowKind::Loading(_) => ("".to_owned(), "-- loading…\n".to_owned()),
        }
    }

    /// Requests the definition of object `nid` for tab `tab`.
    fn request_definition(&mut self, tab: u64, nid: u64) {
        let Some(ctx) = self.tree.definition_context(nid) else {
            return;
        };
        let server = &self.tree.servers[ctx.server];
        let handle = match ctx.database {
            Some(db) => self
                .tree
                .node(db)
                .and_then(|n| n.conn.as_ref())
                .filter(|c| c.state == ConnState::Connected)
                .and_then(|c| c.handle.clone()),
            None => server.handle.clone(),
        };
        let (Some(version), Some(handle)) = (server.version, handle) else {
            return;
        };
        let render = RenderContext {
            version,
            show_system: self.settings.ui.show_system_objects,
        };
        let sql = match self
            .sql
            .render(ctx.obj.definition_folder(), "definition.sql", render)
        {
            Ok(sql) => sql,
            Err(e) => {
                tracing::error!("{e}");
                if let Some(t) = self.tab_mut(tab) {
                    t.view.load(format!("-- {e}\n"));
                }
                return;
            }
        };
        let mut params = vec![ctx.oid];
        if ctx.obj.definition_needs_parent() {
            let Some(parent) = ctx.parent_oid else {
                return;
            };
            params.push(parent);
        }
        self.next_key += 1;
        let key = self.next_key;
        self.pending_defs.insert(key, tab);
        if tab == 0 {
            self.default_key = key;
        }
        if let Some(t) = self.tab_mut(tab) {
            t.view.loading = true;
        }
        handle.load_definition(key, sql, params);
    }

    fn definition_loaded(&mut self, key: u64, result: Result<String, DbError>) {
        let Some(tab) = self.pending_defs.remove(&key) else {
            return;
        };
        if tab == 0 && key != self.default_key {
            return; // the cursor has moved on
        }
        let text = match result {
            Ok(text) => text,
            Err(e) => e
                .to_string()
                .lines()
                .map(|l| format!("-- {l}\n"))
                .collect::<String>(),
        };
        if let Some(t) = self.tab_mut(tab) {
            t.view.load(text);
        }
    }

    /// F3: the selected row in a new object tab (D-009).
    fn show_in_new_tab(&mut self) {
        let Some(row) = self.tree.selected() else {
            return;
        };
        let mut view = TextView::new(true, self.tab_width());
        match self.selected_object() {
            Some(nid) => {
                view.title = self
                    .tree
                    .node(nid)
                    .map(|n| n.label.clone())
                    .unwrap_or_default();
                view.loading = true;
                let id = self.add_tab(TabKind::Object, view);
                self.request_definition(id, nid);
            }
            None => {
                let (title, text) = self.info_text(row);
                view.set_text(title, text);
                self.add_tab(TabKind::Object, view);
            }
        }
        let (server, database) = self.selected_place();
        if let Some(t) = self.tabs.last_mut() {
            t.server = server;
            t.database = database;
        }
    }

    /// Server uid and database of the selected tree row (for the tab status).
    fn selected_place(&self) -> (Option<u64>, Option<String>) {
        let Some(row) = self.tree.selected() else {
            return (None, None);
        };
        let db = match row.kind {
            RowKind::Node(nid) | RowKind::Loading(nid) => self.tree.database_name_of(nid),
            RowKind::Server => None,
        };
        (Some(self.tree.servers[row.server].uid), db)
    }

    // ---- query tabs (D-019, D-020, D-041) ---------------------------------------------

    /// Query state of the active tab.
    fn query(&self) -> Option<&QueryState> {
        self.tabs[self.active_tab].query.as_ref()
    }

    fn query_mut(&mut self) -> Option<&mut QueryState> {
        self.tabs[self.active_tab].query.as_mut()
    }

    /// Runs `f` on the active query tab; a non-empty result goes to the status line.
    fn toggle_query(&mut self, f: impl FnOnce(&mut QueryState) -> String) {
        if let Some(q) = self.query_mut() {
            let status = f(q);
            if !status.is_empty() {
                self.status = status;
            }
        }
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Server and database for a new query tab: the database of the selected tree row
    /// (the database itself or an object below it) on a connected server (D-049).
    fn query_target(&self) -> Option<(usize, String)> {
        let row = self.tree.selected()?;
        if self.tree.servers[row.server].state != ConnState::Connected {
            return None;
        }
        let db = match row.kind {
            RowKind::Node(nid) | RowKind::Loading(nid) => self.tree.database_name_of(nid),
            RowKind::Server => None,
        }?;
        Some((row.server, db))
    }

    /// A new, empty query tab bound to `binding` (server uid, database), or to the
    /// tree selection's database.
    fn new_query_tab(&mut self, binding: Option<(u64, String)>) {
        self.next_query += 1;
        let n = self.next_query;
        let view = TextView::new(false, self.tab_width());
        let id = self.add_tab(TabKind::Query(n), view);
        if let Some(q) = self.query_mut() {
            q.binding = binding;
        }
        self.bind_query_tab(id);
    }

    /// Opens the query tab's own connection (D-011, D-041).
    fn bind_query_tab(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        // reconnect to the same database if the tab had one
        let previous = self.tabs[i]
            .query
            .as_ref()
            .and_then(|q| q.binding.clone())
            .and_then(|(uid, db)| self.tree.index_of(uid).map(|s| (s, db)))
            .filter(|(s, _)| self.tree.servers[*s].state == ConnState::Connected);
        let Some((si, database)) = previous.or_else(|| self.query_target()) else {
            if let Some(q) = self.tabs[i].query.as_mut() {
                q.info("Not connected — select a database in the tree, then press F5.");
            }
            return;
        };
        let timeout = self.timeout();
        let server = &self.tree.servers[si];
        let password = server
            .password
            .clone()
            .or_else(|| Self::pgpass_password(&server.conn, &database));
        let params = ConnectParams::new(&server.conn, &database, password, timeout);
        let (uid, ssl_mode, root) = (
            server.uid,
            server.conn.ssl_mode,
            server.conn.ssl_root_cert.clone(),
        );
        self.next_conn += 1;
        let id = self.next_conn;
        let handle = pgc_db::spawn(id, params, self.sink.clone());
        self.connections.insert(id, Target::Query(tab));
        if let Some(q) = self.tabs[i].query.as_mut() {
            q.binding = Some((uid, database.clone()));
            q.conn = Some(QueryConn {
                id,
                database: database.clone(),
                handle,
                cancel: None,
                state: QueryConnState::Connecting,
                ssl_mode,
                ssl_root_cert: root,
            });
            q.info(format!("Connecting to {database}…"));
        }
    }

    fn on_query_event(&mut self, conn: ConnId, tab: u64, event: DbEvent) {
        let Some(i) = self.tab_index(tab) else {
            self.connections.remove(&conn);
            return;
        };
        let Tab { view, query, .. } = &mut self.tabs[i];
        let Some(q) = query.as_mut() else {
            return;
        };
        match event {
            DbEvent::Connected {
                cancel, version, ..
            } => {
                if let Some(c) = q.conn.as_mut() {
                    c.state = QueryConnState::Connected;
                    c.cancel = Some(cancel);
                    let db = c.database.clone();
                    q.info(format!("Connected to {db} (PostgreSQL {version})"));
                }
            }
            DbEvent::ConnectFailed { error, .. } => {
                self.connections.remove(&conn);
                q.conn = None;
                q.info(format!("Cannot connect: {error}"));
                q.show_messages = true;
            }
            DbEvent::Query { run, event, .. } => {
                if run != q.last_run {
                    return;
                }
                let Some(event) = take_export_rows(q, event) else {
                    return;
                };
                let editor = &view.editor;
                let line_of = |c: usize| editor.line_of_char(c);
                let line_start = |l: usize| editor.line_to_char(l.min(editor.len_lines() - 1));
                let lines = LineMap {
                    line_of: &line_of,
                    line_start: &line_start,
                };
                let finished = q.apply(event, &lines);
                if q.export.is_some() {
                    self.pump_export(tab);
                }
                if finished {
                    self.query_finished(tab);
                }
            }
            DbEvent::Notice { notice, .. } => q.notice(&notice),
            DbEvent::Closed { error, .. } => {
                self.connections.remove(&conn);
                q.conn = None;
                q.running = None;
                q.tx = TxStatus::Idle;
                q.info(match error {
                    Some(e) => format!("Connection lost: {e}"),
                    None => "Connection closed".to_owned(),
                });
            }
            DbEvent::Lookup { key, result, .. } => self.lookup_loaded(key, result),
            DbEvent::Nodes { .. } | DbEvent::Definition { .. } | DbEvent::Model { .. } => {}
        }
    }

    fn query_finished(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let Some(q) = self.tabs[i].query.as_mut() else {
            return;
        };
        let after = q.after_run.take();
        // the window status line replaces "Executing…" with how the run ended (D-043)
        if i == self.active_tab {
            self.status = match q.finished {
                Some(f) if f.failed => format!("{} — see Messages", f.text()),
                Some(f) => f.text(),
                None if q.show_messages => "Query failed — see Messages".to_owned(),
                None => String::new(),
            };
        }
        match after {
            Some(AfterRun::CloseTab) if q.tx == TxStatus::Idle => self.close_tab(tab),
            Some(AfterRun::Bulk) => self.maybe_finish_bulk(),
            _ => {}
        }
    }

    /// F5: the selection, or the whole text, as one request (D-019, D-020).
    fn execute(&mut self) {
        let Some(q) = self.query() else {
            return;
        };
        if q.conn.is_none() {
            let id = self.tabs[self.active_tab].id;
            self.bind_query_tab(id);
            if self.query().is_some_and(|q| q.conn.is_none()) {
                self.status = "Select a database in the tree first".to_owned();
                return;
            }
        }
        let q = self.query().expect("query tab");
        if q.busy() {
            self.status = "A query is already running".to_owned();
            return;
        }
        if q.export.is_some() {
            self.status = "An export is running".to_owned();
            return;
        }
        if !q.connected() {
            self.status = "Still connecting…".to_owned();
            return;
        }
        let editor = &self.tabs[self.active_tab].view.editor;
        let (text, offset) = match editor.selection() {
            Some((a, _)) => (editor.selected_text().unwrap_or_default(), a),
            None => (editor.text(), 0),
        };
        if text.trim().is_empty() {
            return;
        }
        let (sql, plpgsql_lines) = if self.query().is_some_and(|q| q.plpgsql) {
            match crate::query_tab::wrap_plpgsql(&text) {
                Ok((sql, n)) => (sql, Some(n)),
                Err(e) => {
                    self.error(&e);
                    return;
                }
            }
        } else {
            (text, None)
        };
        self.send_query(
            sql,
            Sent {
                offset,
                plpgsql_lines,
            },
            true,
        );
    }

    /// Sends `sql` for the active query tab.
    fn send_query(&mut self, sql: String, sent: Sent, use_modes: bool) {
        self.next_run += 1;
        let run = self.next_run;
        let batch_rows = self.settings.query.batch_rows.max(1) as usize;
        let Some(q) = self.query_mut() else {
            return;
        };
        let Some(conn) = q.conn.as_ref() else {
            return;
        };
        let request = ExecuteRequest {
            run,
            sql,
            batch_rows,
            auto_begin: use_modes && !q.auto_commit,
            auto_rollback: use_modes && q.auto_rollback,
        };
        conn.handle.execute(request);
        q.start_run(run, sent);
        self.status = "Executing…".to_owned();
    }

    /// COMMIT / ROLLBACK in the active query tab (D-019).
    fn send_simple(&mut self, sql: &str, after: Option<AfterRun>) {
        let Some(q) = self.query() else {
            return;
        };
        if !q.connected() || q.busy() {
            return;
        }
        self.send_query(
            sql.to_owned(),
            Sent {
                offset: 0,
                plpgsql_lines: None,
            },
            false,
        );
        if let Some(q) = self.query_mut() {
            q.after_run = after;
        }
    }

    fn finish_transaction(&mut self, tab: u64, after: AfterRun) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        let previous = self.active_tab;
        self.active_tab = i;
        self.send_simple("COMMIT", Some(after));
        self.active_tab = previous.min(self.tabs.len() - 1);
    }

    /// Alt+Shift+C: PostgreSQL's cancel request (D-019).
    fn cancel_query(&mut self) {
        let Some(q) = self.query_mut() else {
            return;
        };
        if let Some(e) = q.export.as_mut() {
            // D-042: stop the export; the writer deletes the file
            e.abort_reason
                .get_or_insert_with(|| "Export cancelled".to_owned());
            let _ = e.writer.send(ToWriter::Abort);
            self.status = "Cancelling export…".to_owned();
            return;
        }
        let (Some(running), Some(conn)) = (q.running.as_mut(), q.conn.as_ref()) else {
            return;
        };
        if running.paused {
            // a paused result: end it, the remaining rows are dropped (D-019)
            conn.handle.discard(q.last_run);
            self.status = "Result closed".to_owned();
            return;
        }
        if let Some(cancel) = &conn.cancel {
            pgc_db::cancel_query(cancel, conn.ssl_mode, conn.ssl_root_cert.clone());
            running
                .cancel_requested
                .get_or_insert_with(std::time::Instant::now);
            self.status = "Cancel requested…".to_owned();
        }
    }

    /// After `cancel_wait_secs` without reaction: offer to close the connection (D-041).
    fn check_cancel_wait(&mut self) {
        let wait =
            std::time::Duration::from_secs(u64::from(self.settings.query.cancel_wait_secs.max(1)));
        let mut ask = Vec::new();
        for tab in &mut self.tabs {
            if let Some(r) = tab.query.as_mut().and_then(|q| q.running.as_mut())
                && !r.asked
                && r.cancel_requested.is_some_and(|t| t.elapsed() >= wait)
            {
                r.asked = true;
                ask.push(tab.id);
            }
        }
        for id in ask {
            self.dialogs.push(
                Dialog::confirm_buttons(
                    DialogKind::CancelWait,
                    "Cancel",
                    "The server did not react to the cancel request.\nClose this query tab's connection?",
                    &[("close", "&Close connection"), ("wait", "&Keep waiting")],
                )
                .with_tag(id),
            );
        }
    }

    fn abort_query_conn(&mut self, tab: u64) {
        let Some(i) = self.tab_index(tab) else {
            return;
        };
        if let Some(q) = self.tabs[i].query.as_mut()
            && let Some(conn) = q.conn.take()
        {
            conn.handle.abort();
            self.connections.remove(&conn.id);
            q.running = None;
            q.tx = TxStatus::Idle;
            q.info("Connection closed (the server did not react to the cancel request)");
        }
    }

    fn fetch_all(&mut self) {
        if let Some(q) = self.query_mut()
            && q.paused()
            && q.export.is_none()
            && let Some(conn) = q.conn.as_ref()
        {
            conn.handle.fetch_all(q.last_run);
            if let Some(rs) = q.results.last_mut() {
                rs.fetching = true;
            }
        }
    }

    /// Asks for the next batch when the grid cursor nears the end of a paused result.
    fn after_grid_move(&mut self) {
        let (h, w) = self.grid_size;
        let Some(q) = self.query_mut() else {
            return;
        };
        let last = q.results.len().saturating_sub(1);
        let active = q.active_result;
        if let Some(rs) = q.results.get_mut(active) {
            rs.ensure_visible(h.saturating_sub(1), w);
            if active == last
                && q.export.is_none()
                && rs.more
                && !rs.fetching
                && rs.row + h.max(1) * 2 >= rs.rows.len()
                && let Some(conn) = q.conn.as_ref()
            {
                rs.fetching = true;
                conn.handle.fetch_more(q.last_run);
            }
        }
    }

    fn active_result_set(&mut self) -> Option<&mut crate::grid::ResultSet> {
        let q = self.query_mut()?;
        if q.show_messages {
            return None;
        }
        let i = q.active_result;
        q.results.get_mut(i)
    }

    /// Keys in the results pane. Returns false if the command is not for it.
    fn results_command(&mut self, cmd: Command) -> bool {
        let page = self.grid_size.0.saturating_sub(1).max(1) as isize;
        let Some(q) = self.query_mut() else {
            return false;
        };
        if q.show_messages {
            let n = q.messages.len();
            let cursor = &mut q.message_cursor;
            match cmd {
                Command::Up => *cursor = cursor.saturating_sub(1),
                Command::Down => *cursor = (*cursor + 1).min(n.saturating_sub(1)),
                Command::Home | Command::DocStart => *cursor = 0,
                Command::End | Command::DocEnd => *cursor = n.saturating_sub(1),
                Command::Copy => {
                    let text = q.messages.get(q.message_cursor).map(|m| m.text.clone());
                    if let Some(text) = text
                        && self.clipboard.copy(&text)
                    {
                        self.effects.push(Effect::Osc52(text));
                    }
                }
                Command::Accept => {
                    // Enter on an error: jump to its line (D-019)
                    if let Some(pos) = q.messages.get(q.message_cursor).and_then(|m| m.jump) {
                        self.focus = Focus::Tabs;
                        self.editor().set_cursor(pos, false);
                    }
                }
                _ => return false,
            }
            return true;
        }
        let i = q.active_result;
        let Some(rs) = q.results.get_mut(i) else {
            return matches!(
                cmd,
                Command::Up | Command::Down | Command::Left | Command::Right
            );
        };
        match cmd {
            Command::Up => rs.move_cursor(-1, 0, 0),
            Command::Down => rs.move_cursor(1, 0, 0),
            Command::Left => rs.move_cursor(0, -1, 0),
            Command::Right => rs.move_cursor(0, 1, 0),
            Command::PageUp => rs.move_cursor(-page, 0, 0),
            Command::PageDown => rs.move_cursor(page, 0, 0),
            Command::Home => rs.move_cursor(0, isize::MIN / 2, 0),
            Command::End => rs.move_cursor(0, isize::MAX / 2, 0),
            Command::DocStart => rs.move_cursor(isize::MIN / 2, isize::MIN / 2, 0),
            Command::DocEnd => rs.move_cursor(isize::MAX / 2, 0, 0),
            Command::SelectAll => rs.all_selected = true,
            Command::Copy => {
                if let Some(text) = rs.copy_text()
                    && self.clipboard.copy(&text)
                {
                    self.effects.push(Effect::Osc52(text));
                }
                return true;
            }
            _ => return false,
        }
        self.after_grid_move();
        true
    }

    fn click_results(&mut self, p: Position, hits: &Hits) {
        let grid = hits.grid;
        let Some(q) = self.query_mut() else {
            return;
        };
        if q.show_messages {
            let i = (p.y - grid.y) as usize;
            if i < q.messages.len() {
                q.message_cursor = hits
                    .message_rows
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(m, _)| *m)
                    .unwrap_or(q.message_cursor);
            }
            return;
        }
        if let Some(rs) = q.results.get_mut(q.active_result)
            && let Some(&(row, col)) = hits
                .grid_cells
                .iter()
                .find(|(_, r)| r.contains(p))
                .map(|(rc, _)| rc)
        {
            rs.row = row;
            rs.col = col;
            rs.all_selected = false;
        }
        self.after_grid_move();
    }

    /// Closes a tab after asking about unsaved text (`ask_save`) and an open
    /// transaction (D-019).
    fn request_close_tab(&mut self, id: u64, ask_save: bool) {
        let Some(i) = self.tab_index(id) else {
            return;
        };
        let tab = &self.tabs[i];
        let Some(q) = tab.query.as_ref() else {
            self.close_tab(id);
            return;
        };
        if ask_save && self.settings.query.ask_save && tab.view.editor.modified() {
            let text = format!("{} has unsaved changes. Save them?", tab.label());
            self.dialogs.push(
                Dialog::confirm_buttons(
                    DialogKind::SaveChanges,
                    "Close query tab",
                    &text,
                    &[("yes", "&Yes"), ("no", "&No"), ("cancel", "&Cancel")],
                )
                .with_tag(id),
            );
        } else if q.tx != TxStatus::Idle && q.connected() {
            self.dialogs.push(
                Dialog::confirm_buttons(
                    DialogKind::CloseTransaction,
                    "Close query tab",
                    "This query tab has an open transaction.",
                    &[
                        ("commit", "Co&mmit"),
                        ("rollback", "&Rollback"),
                        ("cancel", "&Cancel"),
                    ],
                )
                .with_tag(id),
            );
        } else {
            self.close_tab(id);
        }
    }

    /// "Save changes?" → Yes: save to the tab's file or ask for one; `then` runs
    /// after a successful save.
    fn save_before(&mut self, id: u64, then: AfterSave) {
        let Some(i) = self.tab_index(id) else {
            return;
        };
        self.active_tab = i;
        let file = self.tabs[i].query.as_ref().and_then(|q| q.file.clone());
        self.after_save = Some(then);
        match file {
            Some(path) => self.save_to(path),
            None => self.open_file_dialog(true),
        }
    }

    fn close_tab(&mut self, id: u64) {
        let Some(i) = self.tab_index(id) else {
            return;
        };
        if i == 0 {
            return;
        }
        let tab = self.tabs.remove(i);
        self.pending_defs.retain(|_, t| *t != tab.id);
        if let Some(conn) = tab.query.and_then(|q| q.conn) {
            // dropping the connection rolls back an open transaction (D-019)
            conn.handle.abort();
            self.connections.remove(&conn.id);
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        if self.focus == Focus::Results {
            self.focus = Focus::Tabs;
        }
    }

    fn tabs_with_transaction(&self) -> Vec<u64> {
        self.tabs
            .iter()
            .filter(|t| {
                t.query
                    .as_ref()
                    .is_some_and(|q| q.connected() && q.tx != TxStatus::Idle)
            })
            .map(|t| t.id)
            .collect()
    }

    /// F10 / File → Quit (D-019, D-043).
    fn request_quit(&mut self) {
        self.request_bulk(Bulk::Quit);
    }

    /// The tab belongs to what is being quit / disconnected.
    fn in_bulk(b: Bulk, t: &Tab) -> bool {
        match b {
            Bulk::Quit => true,
            Bulk::Disconnect(uid) => {
                t.server == Some(uid)
                    || t.query
                        .as_ref()
                        .and_then(|q| q.binding.as_ref())
                        .is_some_and(|(s, _)| *s == uid)
            }
        }
    }

    /// Quitting or disconnecting a server: asks about unsaved query text, one tab at
    /// a time, then about open transactions; `Cancel` stops (D-019, D-043).
    fn request_bulk(&mut self, b: Bulk) {
        let skip = match &self.bulk {
            Some((current, skip)) if *current == b => skip.clone(),
            _ => Vec::new(),
        };
        self.bulk = Some((b, skip.clone()));
        let title = match b {
            Bulk::Quit => "Quit",
            Bulk::Disconnect(_) => "Disconnect",
        };
        if self.settings.query.ask_save
            && let Some(i) = self.tabs.iter().position(|t| {
                t.query.is_some()
                    && Self::in_bulk(b, t)
                    && t.view.editor.modified()
                    && !skip.contains(&t.id)
            })
        {
            self.active_tab = i;
            let tab = &self.tabs[i];
            let text = format!("{} has unsaved changes. Save them?", tab.label());
            let id = tab.id;
            self.dialogs.push(
                Dialog::confirm_buttons(
                    DialogKind::QuitSaveChanges,
                    title,
                    &text,
                    &[("yes", "&Yes"), ("no", "&No"), ("cancel", "&Cancel")],
                )
                .with_tag(id),
            );
            return;
        }
        let open = self.bulk_transactions(b).len();
        if open == 0 {
            self.bulk = None;
            self.finish_bulk(b);
            return;
        }
        self.bulk = Some((b, Vec::new()));
        let text = if open == 1 {
            "A query tab has an open transaction.".to_owned()
        } else {
            format!("{open} query tabs have open transactions.")
        };
        self.dialogs.push(Dialog::confirm_buttons(
            DialogKind::QuitTransactions,
            title,
            &text,
            &[
                ("commit", "Co&mmit all"),
                ("rollback", "&Rollback all"),
                ("cancel", "&Cancel"),
            ],
        ));
    }

    /// Query tabs of `b` with an open transaction.
    fn bulk_transactions(&self, b: Bulk) -> Vec<u64> {
        let ids = self.tabs_with_transaction();
        self.tabs
            .iter()
            .filter(|t| ids.contains(&t.id) && Self::in_bulk(b, t))
            .map(|t| t.id)
            .collect()
    }

    /// Quits, or closes every tab of the server and its connections (D-043).
    fn finish_bulk(&mut self, b: Bulk) {
        match b {
            Bulk::Quit => self.quit = true,
            Bulk::Disconnect(uid) => {
                let ids: Vec<u64> = self
                    .tabs
                    .iter()
                    .skip(1)
                    .filter(|t| Self::in_bulk(b, t))
                    .map(|t| t.id)
                    .collect();
                for id in ids {
                    self.close_tab(id);
                }
                self.disconnect_server(uid);
                self.status = "Disconnected".to_owned();
            }
        }
    }

    /// Finishes quitting / disconnecting once every COMMIT sent for it has finished.
    fn maybe_finish_bulk(&mut self) {
        let Some(b) = self.bulk_pending else {
            return;
        };
        let waiting = self.tabs.iter().any(|t| {
            Self::in_bulk(b, t)
                && t.query
                    .as_ref()
                    .is_some_and(|q| q.after_run == Some(AfterRun::Bulk) || q.busy())
        });
        if !waiting {
            self.bulk_pending = None;
            self.finish_bulk(b);
        }
    }

    // ---- files (D-014, D-041) -------------------------------------------------------

    fn open_file_dialog(&mut self, save: bool) {
        let dir = self.file_dir.clone();
        let initial = if save {
            self.query()
                .and_then(|q| q.file.as_ref())
                .and_then(|f| f.file_name())
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "query.sql".to_owned())
        } else {
            String::new()
        };
        let entries = list_dir(&dir, "sql");
        let labels: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let marks: Vec<bool> = entries.iter().map(|e| e.2).collect();
        let (kind, title) = if save {
            (DialogKind::SaveFile, "Save SQL file")
        } else {
            (DialogKind::OpenFile, "Open SQL file")
        };
        let mut dialog = Dialog::new(
            kind,
            title,
            70,
            vec![
                Widget::label(&format!("Folder: {}", dir.display())),
                Widget::field("path", "&File name:", &initial, 12),
                Widget::list("files", "", labels, 0, 14).highlighted(marks),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        );
        if !save {
            dialog.focus_on("files");
        }
        self.dialogs.push(dialog);
    }

    fn file_dialog_done(&mut self, dialog: Dialog) {
        let save = dialog.kind == DialogKind::SaveFile;
        let export = dialog.kind == DialogKind::ExportFile;
        let dir = self.file_dir.clone();
        let chosen = if dialog.focused_id() == Some("files") {
            let entries = list_dir(&dir, "sql");
            dialog
                .selected("files")
                .and_then(|i| entries.get(i))
                .map(|e| e.0.trim_end_matches('/').to_owned())
                .unwrap_or_default()
        } else {
            dialog.text("path").unwrap_or("").trim().to_owned()
        };
        if chosen.is_empty() {
            self.dialogs.push(dialog);
            return;
        }
        let path = if chosen == ".." {
            dir.parent().map(Path::to_path_buf).unwrap_or(dir)
        } else {
            dir.join(&chosen)
        };
        if path.is_dir() {
            self.file_dir = path;
            if export {
                let name = dialog.text("path").unwrap_or("").to_owned();
                let options = export_options(&dialog);
                self.open_export_dialog(Some(name), Some(options));
            } else {
                self.open_file_dialog(save);
            }
            return;
        }
        if export {
            let options = export_options(&dialog);
            if path.exists() {
                self.pending_export = Some((path.clone(), options));
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmExportOverwrite,
                    "Export result",
                    &format!("{} exists. Overwrite it?", path.display()),
                ));
            } else {
                self.start_export(path, options);
            }
        } else if save {
            if path.exists() {
                self.pending_save = Some(path.clone());
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmOverwrite,
                    "Save",
                    &format!("{} exists. Overwrite it?", path.display()),
                ));
            } else {
                self.save_to(path);
            }
        } else {
            self.open_path(path);
        }
    }

    // ---- definition export / load (D-012, D-048) --------------------------------------

    /// The database selected in the tree: (server uid, database name).
    fn selected_database(&self) -> Option<(u64, String)> {
        let row = self.tree.selected()?;
        let RowKind::Node(nid) = row.kind else {
            return None;
        };
        let node = self.tree.node(nid).filter(|n| n.is_database())?;
        Some((self.tree.servers[row.server].uid, node.name.clone()))
    }

    fn open_definition_dialog(&mut self, save: bool, initial: &str) {
        let dir = self.file_dir.clone();
        let entries = list_dir(&dir, "pgdef");
        let labels: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let marks: Vec<bool> = entries.iter().map(|e| e.2).collect();
        let database = self
            .definition_target
            .as_ref()
            .map(|t| t.1.clone())
            .unwrap_or_default();
        let (kind, title) = if save {
            (DialogKind::SaveDefinition, "Export database definition")
        } else {
            (DialogKind::OpenDefinition, "Load database definition")
        };
        let mut dialog = Dialog::new(
            kind,
            title,
            70,
            vec![
                Widget::label(&format!("Database: {database}")),
                Widget::label(&format!("Folder: {}", dir.display())),
                Widget::field("path", "&File name:", initial, 12),
                Widget::list("files", "", labels, 0, 14).highlighted(marks),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        );
        if !save {
            dialog.focus_on("files");
        }
        self.dialogs.push(dialog);
    }

    fn definition_dialog_done(&mut self, dialog: Dialog) {
        let save = dialog.kind == DialogKind::SaveDefinition;
        let dir = self.file_dir.clone();
        let chosen = if dialog.focused_id() == Some("files") {
            let entries = list_dir(&dir, "pgdef");
            dialog
                .selected("files")
                .and_then(|i| entries.get(i))
                .map(|e| e.0.trim_end_matches('/').to_owned())
                .unwrap_or_default()
        } else {
            dialog.text("path").unwrap_or("").trim().to_owned()
        };
        if chosen.is_empty() {
            self.dialogs.push(dialog);
            return;
        }
        let path = if chosen == ".." {
            dir.parent().map(Path::to_path_buf).unwrap_or(dir)
        } else {
            dir.join(&chosen)
        };
        if path.is_dir() {
            self.file_dir = path;
            let initial = dialog.text("path").unwrap_or("").to_owned();
            self.open_definition_dialog(save, if save { &initial } else { "" });
            return;
        }
        if save {
            if path.exists() {
                self.pending_definition = Some(path.clone());
                self.dialogs.push(Dialog::confirm(
                    DialogKind::ConfirmDefinitionOverwrite,
                    "Export database definition",
                    &format!("{} exists. Overwrite it?", path.display()),
                ));
            } else {
                self.start_job(JobKind::Export(path));
            }
        } else {
            match pgc_model::file::load(&path) {
                Ok(model) => self.start_job(JobKind::Load(Box::new(model))),
                Err(e) => self.error(&format!("Cannot load {}:\n{e}", path.display())),
            }
        }
    }

    /// Opens a connection to the target database for the job; the structure is read
    /// once it is connected.
    fn start_job(&mut self, kind: JobKind) {
        let Some((uid, database)) = self.definition_target.take() else {
            return;
        };
        let Some(si) = self.tree.index_of(uid) else {
            return;
        };
        let timeout = self.timeout();
        let server = &self.tree.servers[si];
        let password = server
            .password
            .clone()
            .or_else(|| Self::pgpass_password(&server.conn, &database));
        let params = ConnectParams::new(&server.conn, &database, password, timeout);
        self.next_conn += 1;
        let id = self.next_conn;
        let handle = pgc_db::spawn(id, params, self.sink.clone());
        self.connections.insert(id, Target::Structure);
        self.status = format!("Reading the structure of {database}…");
        self.jobs.insert(
            id,
            StructureJob {
                uid,
                database,
                kind,
                handle,
            },
        );
    }

    fn end_job(&mut self, id: ConnId) -> Option<StructureJob> {
        self.connections.remove(&id);
        let job = self.jobs.remove(&id)?;
        job.handle.close();
        Some(job)
    }

    fn on_job_event(&mut self, id: ConnId, event: DbEvent) {
        match event {
            DbEvent::Connected { version, .. } => {
                let Some(job) = self.jobs.get(&id) else {
                    return;
                };
                match pgc_db::model::ModelQueries::render(&self.sql, version) {
                    Ok(q) => job.handle.read_model(0, q),
                    Err(e) => {
                        self.end_job(id);
                        self.error(&format!("Cannot read the structure:\n{e}"));
                    }
                }
            }
            DbEvent::ConnectFailed { error, .. } => {
                if let Some(job) = self.end_job(id) {
                    self.status.clear();
                    self.error(&format!("Cannot connect to {}:\n{error}", job.database));
                }
            }
            DbEvent::Closed { error, .. } => {
                if let Some(job) = self.end_job(id) {
                    self.status.clear();
                    let why =
                        error.map_or("the connection was closed".to_owned(), |e| e.to_string());
                    self.error(&format!(
                        "Reading the structure of {} failed:\n{why}",
                        job.database
                    ));
                }
            }
            DbEvent::Model { result, .. } => {
                let Some(job) = self.end_job(id) else {
                    return;
                };
                self.status.clear();
                match result {
                    Ok(live) => self.job_done(job, *live),
                    Err(e) => self.error(&format!(
                        "Cannot read the structure of {}:\n{e}",
                        job.database
                    )),
                }
            }
            _ => {}
        }
    }

    fn job_done(&mut self, job: StructureJob, live: pgc_model::Database) {
        match job.kind {
            JobKind::Export(path) => match pgc_model::file::save(&live, &path) {
                Ok(()) => {
                    self.status = format!(
                        "Definition of {} exported to {}",
                        job.database,
                        path.display()
                    );
                }
                Err(e) => self.error(&format!("Cannot write {}:\n{e}", path.display())),
            },
            JobKind::Load(want) => match pgc_model::sync::sync_script(&want, &live) {
                None => self.dialogs.push(Dialog::message(
                    "Load database definition",
                    &format!(
                        "The structure of {} already matches the definition file.",
                        job.database
                    ),
                    false,
                )),
                Some(script) => {
                    // review in a query tab of that database; never run automatically
                    let mut view = TextView::new(false, self.tab_width());
                    view.load(script);
                    self.next_query += 1;
                    let id = self.add_tab(TabKind::Query(self.next_query), view);
                    if let Some(q) = self.tab_index(id).and_then(|i| self.tabs[i].query.as_mut()) {
                        q.binding = Some((job.uid, job.database.clone()));
                    }
                    self.bind_query_tab(id);
                    self.status = format!(
                        "Sync script for {}: review it, then run it with F5",
                        job.database
                    );
                }
            },
        }
    }

    /// Where a query tab is connected: `server · user@database` or its state (D-043).
    pub fn tab_connection_label(&self, tab: &Tab) -> String {
        let Some(q) = &tab.query else {
            // object and default tabs: where the object comes from, then read only
            return match tab.server {
                Some(uid) => format!(
                    "{} · read only",
                    self.place_label(uid, tab.database.as_deref())
                ),
                None => "read only".to_owned(),
            };
        };
        let Some((uid, db)) = &q.binding else {
            return "No database – select one in the tree".to_owned();
        };
        let place = self.place_label(*uid, Some(db));
        match q.conn.as_ref().map(|c| c.state) {
            Some(QueryConnState::Connected) => place,
            Some(QueryConnState::Connecting) => format!("{place} – connecting…"),
            None => format!("{place} – not connected"),
        }
    }

    /// `server · user@database` (or `server · user` without a database).
    fn place_label(&self, uid: u64, db: Option<&str>) -> String {
        let (server, user) = self
            .tree
            .servers
            .iter()
            .find(|s| s.uid == uid)
            .map(|s| (s.conn.name.as_str(), s.conn.username.as_str()))
            .unwrap_or(("?", ""));
        match (user.is_empty(), db) {
            (true, Some(db)) => format!("{server} · {db}"),
            (false, Some(db)) => format!("{server} · {user}@{db}"),
            (true, None) => server.to_owned(),
            (false, None) => format!("{server} · {user}"),
        }
    }

    // ---- developer templates (D-013, D-046) -----------------------------------------

    /// F7 / Tools → Generate SQL from developer template: only for a connected, idle
    /// query tab.
    fn open_template_picker(&mut self) {
        let Some(q) = self.query() else {
            self.error("Developer templates insert SQL into a query tab.\nOpen a query tab first.");
            return;
        };
        if !q.connected() {
            self.error("The query tab has no active connection.\nSelect a database in the tree and connect the tab first.");
            return;
        }
        if q.busy() || q.paused() || q.export.is_some() {
            self.error("The query tab's connection is busy.\nWait for the query (or close its result) first.");
            return;
        }
        let Some(version) = self.tab_server_version() else {
            self.error("The server version of the tab is not known yet.");
            return;
        };
        let lib = TemplateLibrary::scan(
            &self.paths.user_templates_dir(),
            &pgc_core::paths::system_templates_dir(),
        );
        if lib.entries.is_empty() {
            self.error("No developer templates found.");
            return;
        }
        let names = lib
            .entries
            .iter()
            .map(
                |e| match e.resolve(version).map(|(text, _)| devtemplate::parse(text)) {
                    Some(Ok(t)) => (t.name.unwrap_or_else(|| e.stem().to_owned()), t.description),
                    Some(Err(err)) => (format!("{} (error)", e.stem()), err),
                    None => (
                        format!("{} (not for this server version)", e.stem()),
                        String::new(),
                    ),
                },
            )
            .collect();
        self.picker = Some(Picker {
            lib,
            names,
            rows: Vec::new(),
            filter: None,
        });
        self.dialogs.push(Dialog::new(
            DialogKind::TemplatePicker,
            "Generate SQL from developer template",
            64,
            vec![
                Widget::field("search", "&Search:", "", 9),
                Widget::list("items", "", Vec::new(), 0, 14),
                Widget::label_id("desc", " "),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        ));
        self.sync_picker();
    }

    /// Server version of the active query tab's server.
    fn tab_server_version(&self) -> Option<pgc_core::ServerVersion> {
        let (uid, _) = self.query()?.binding.as_ref()?;
        self.tree.servers.iter().find(|s| s.uid == *uid)?.version
    }

    /// Picker: rebuilds the tree rows when the search text changed; shows the
    /// description of the selected template.
    fn sync_picker(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if d.kind != DialogKind::TemplatePicker {
            return;
        }
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let filter = d.text("search").unwrap_or("").trim().to_lowercase();
        if p.filter.as_deref() != Some(filter.as_str()) {
            let (rows, labels, marks) = picker_rows(p, &filter);
            p.rows = rows;
            p.filter = Some(filter);
            let first = p.rows.iter().position(Option::is_some).unwrap_or(0);
            d.set_list("items", labels, marks, first);
        }
        let desc = d
            .selected("items")
            .and_then(|i| p.rows.get(i).copied().flatten())
            .map(|e| p.names[e].1.clone())
            .unwrap_or_default();
        d.set_label("desc", if desc.is_empty() { " " } else { &desc });
    }

    fn template_chosen(&mut self, dialog: Dialog) {
        let entry = dialog.selected("items").and_then(|i| {
            self.picker
                .as_ref()
                .and_then(|p| p.rows.get(i).copied().flatten())
        });
        let Some(e) = entry else {
            // a folder row: keep the picker open
            self.dialogs.push(dialog);
            return;
        };
        let Some(version) = self.tab_server_version() else {
            return;
        };
        let picker = self.picker.take().expect("picker");
        let entry = &picker.lib.entries[e];
        let Some((text, origin)) = entry.resolve(version) else {
            self.error("This template has no version for the tab's server.");
            return;
        };
        let file = match devtemplate::parse(text) {
            Ok(f) => f,
            Err(err) => {
                self.error(&format!("Template {origin}:\n{err}"));
                return;
            }
        };
        let Some(q) = self.query() else {
            return;
        };
        let database = q.database().unwrap_or("").to_owned();
        let tab = self.tabs[self.active_tab].id;
        let title = format!("New: {}", picker.names[e].0);
        let ctx = devtemplate::TemplateContext { version, database };
        let mut form = TemplateForm::new(tab, title, file, ctx, self.tab_width());
        self.template_defaults(&mut form);
        self.template = Some(form);
        self.row_choices.clear();
        self.start_lookups();
    }

    /// Choice defaults from the tree selection when it is in the tab's server and
    /// database (D-046).
    fn template_defaults(&self, form: &mut TemplateForm) {
        use crate::objects::ObjType as T;
        use devtemplate::Source as S;
        let Some((uid, db)) = self.query().and_then(|q| q.binding.clone()) else {
            return;
        };
        let Some(RowKind::Node(nid)) = self.tree.selected().map(|r| r.kind) else {
            return;
        };
        let Some((server, database, objects)) = self.tree.object_path(nid) else {
            return;
        };
        if self.tree.servers[server].uid != uid {
            return;
        }
        let same_db = database.as_deref() == Some(db.as_str());
        let schema = objects
            .iter()
            .find(|o| o.0 == T::Schema)
            .map(|o| o.1.clone());
        let nearest = |kinds: &[T]| {
            objects
                .iter()
                .rev()
                .find(|o| kinds.contains(&o.0))
                .map(|o| o.1.clone())
        };
        // functions and procedures: the tree label is `name(args)` (D-050)
        let nearest_label = |kinds: &[T]| {
            objects
                .iter()
                .rev()
                .find(|o| kinds.contains(&o.0))
                .map(|o| o.2.clone())
        };
        let mut wants = Vec::new();
        for f in &form.fields {
            let Some(source) = f.param.source else {
                continue;
            };
            let qualified =
                |name: Option<String>| Some(QualName::new(schema.clone()?, name?).to_string());
            let label = match source {
                S::Roles => objects
                    .last()
                    .filter(|o| o.0 == T::Role)
                    .map(|o| o.1.clone()),
                _ if !same_db => None,
                S::Schemas => schema.clone(),
                S::Tables => qualified(nearest(&[T::Table, T::ForeignTable, T::Partition])),
                S::Views => qualified(nearest(&[T::View])),
                S::MaterializedViews => qualified(nearest(&[T::MatView])),
                S::Sequences => qualified(nearest(&[T::Sequence])),
                S::Functions => qualified(nearest_label(&[T::Function, T::TriggerFunction])),
                S::Procedures => qualified(nearest_label(&[T::Procedure])),
                S::Types => qualified(nearest(&[T::Type, T::Domain])),
                S::UserTypes => qualified(nearest(&[T::Type])),
                S::Domains => qualified(nearest(&[T::Domain])),
                S::Indexes => qualified(nearest(&[T::Index])),
                S::Triggers => nearest(&[T::Trigger]).and_then(|name| {
                    let table = nearest(&[T::Table, T::Partition, T::View])?;
                    Some(
                        QualName {
                            table: Some(table),
                            ..QualName::new(schema.clone()?, name)
                        }
                        .to_string(),
                    )
                }),
            };
            if let Some(label) = label {
                wants.push((f.param.name.clone(), label));
            }
        }
        for (name, label) in wants {
            form.want(&name, label);
        }
    }

    /// Asks the tab's connection for every live choice list of the form.
    fn start_lookups(&mut self) {
        let Some(form) = &self.template else {
            return;
        };
        let mut wanted = Vec::new();
        for f in &form.fields {
            match f.param.kind {
                ParamType::Choice if f.param.options.is_empty() => {
                    wanted.push((
                        LookupTarget::Field(f.param.name.clone(), shape(&f.param)),
                        sql_of(&f.param),
                    ));
                }
                ParamType::List => {
                    for sub in &f.param.fields {
                        if sub.kind == ParamType::Choice && sub.options.is_empty() {
                            wanted.push((
                                LookupTarget::Row(
                                    f.param.name.clone(),
                                    sub.name.clone(),
                                    shape(sub),
                                ),
                                sql_of(sub),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        let Some(handle) = self
            .query()
            .and_then(|q| q.conn.as_ref())
            .map(|c| c.handle.clone())
        else {
            return;
        };
        for (target, sql) in wanted {
            self.next_key += 1;
            self.template_lookups.insert(self.next_key, target);
            handle.lookup(self.next_key, sql);
        }
    }

    fn lookup_loaded(&mut self, key: u64, result: Result<Vec<Vec<String>>, DbError>) {
        let Some(target) = self.template_lookups.remove(&key) else {
            return;
        };
        let result = result.map_err(|e| e.to_string());
        match target {
            LookupTarget::Field(name, shape) => {
                if let Some(form) = self.template.as_mut() {
                    form.set_choices(
                        &name,
                        result.map(|rows| crate::template_dialog::choices_from_rows(rows, shape)),
                    );
                }
            }
            LookupTarget::Row(list, field, shape) => {
                if let Ok(rows) = result {
                    self.row_choices.insert(
                        (list, field),
                        crate::template_dialog::choices_from_rows(rows, shape),
                    );
                }
            }
            LookupTarget::Names {
                tab,
                generation,
                need,
            } => self.names_loaded(tab, generation, need, result),
            LookupTarget::Generate { tab, what, shown } => {
                self.generate_loaded(tab, what, &shown, result)
            }
        }
    }

    fn form_event(&mut self, event: FormEvent) {
        match event {
            FormEvent::None => {}
            FormEvent::Cancel => {
                self.template = None;
                self.template_lookups.clear();
            }
            FormEvent::Ok => {
                let Some(form) = &self.template else {
                    return;
                };
                match form.result() {
                    Ok(sql) => {
                        let tab = form.tab;
                        self.template = None;
                        self.template_lookups.clear();
                        if let Some(i) = self.tab_index(tab) {
                            self.active_tab = i;
                            self.focus = Focus::Tabs;
                            self.edit(Action::Insert(sql));
                            self.status = "Template inserted".to_owned();
                        }
                    }
                    Err(e) => self.error(&e),
                }
            }
            FormEvent::AddRow(field) => self.open_row_dialog(field, None),
            FormEvent::EditRow(field, row) => self.open_row_dialog(field, Some(row)),
        }
    }

    /// The small dialog with the fields of one row of a `list` parameter.
    fn open_row_dialog(&mut self, field: usize, row: Option<usize>) {
        let Some(form) = &self.template else {
            return;
        };
        let param = form.fields[field].param.clone();
        let current = row.and_then(|r| match &form.fields[field].value {
            crate::template_dialog::FieldValue::List { rows, .. } => rows.get(r).cloned(),
            _ => None,
        });
        let lw = param
            .fields
            .iter()
            .map(|f| f.label().chars().count() as u16 + 2)
            .max()
            .unwrap_or(8)
            .min(24);
        let mut widgets = Vec::new();
        for sub in &param.fields {
            let id = crate::dialog::intern(&sub.name);
            let label = format!("{}{}:", sub.label(), if sub.required { "*" } else { "" });
            let value = current.as_ref().and_then(|r| r.get(&sub.name));
            match sub.kind {
                ParamType::Bool => {
                    let checked = match value {
                        Some(ParamValue::Bool(b)) => *b,
                        _ => sub.default_bool(),
                    };
                    widgets.push(Widget::checkbox(id, sub.label(), checked));
                }
                ParamType::Choice => {
                    let items = self.row_items(&param.name, sub);
                    let text = match value {
                        Some(ParamValue::Text(s)) => s.clone(),
                        Some(ParamValue::Object(q)) => q.to_string(),
                        _ => sub.default_text(),
                    };
                    match items {
                        Some(items) => {
                            let mut labels: Vec<String> = vec![String::new()];
                            labels.extend(items.iter().map(|c| c.label.clone()));
                            let selected = labels.iter().position(|l| *l == text).unwrap_or(0);
                            widgets.push(Widget::combo(id, &label, labels, selected, lw));
                        }
                        None => widgets.push(Widget::field(id, &label, &text, lw)),
                    }
                }
                _ => {
                    let text = match value {
                        Some(ParamValue::Text(s)) => s.clone(),
                        _ => sub.default_text(),
                    };
                    widgets.push(Widget::field(id, &label, &text, lw));
                }
            }
        }
        widgets.push(Widget::button("ok", "&OK", true));
        widgets.push(Widget::button("cancel", "&Cancel", false));
        let title = if row.is_some() { "Edit row" } else { "Add row" };
        self.row_edit = Some((field, row));
        self.dialogs.push(Dialog::new(
            DialogKind::TemplateRow,
            &format!("{}: {title}", param.label()),
            60,
            widgets,
        ));
    }

    /// Items for a choice field of a list row: fixed options or the loaded list;
    /// `None` = not loaded (a text input is used instead).
    fn row_items(&self, list: &str, sub: &devtemplate::Param) -> Option<Vec<Choice>> {
        if !sub.options.is_empty() {
            return Some(
                sub.options
                    .iter()
                    .map(|o| Choice {
                        label: o.clone(),
                        value: ParamValue::Text(o.clone()),
                    })
                    .collect(),
            );
        }
        self.row_choices
            .get(&(list.to_owned(), sub.name.clone()))
            .cloned()
    }

    fn row_done(&mut self, dialog: Dialog) {
        let Some((field, row)) = self.row_edit else {
            return;
        };
        let Some(param) = self
            .template
            .as_ref()
            .map(|f| f.fields[field].param.clone())
        else {
            return;
        };
        let mut values = std::collections::BTreeMap::new();
        for sub in &param.fields {
            let v = match sub.kind {
                ParamType::Bool => ParamValue::Bool(dialog.checked(&sub.name)),
                ParamType::Choice => match self.row_items(&param.name, sub) {
                    Some(items) => match dialog.selected(&sub.name) {
                        Some(i) if i > 0 => items[i - 1].value.clone(),
                        _ => ParamValue::Text(String::new()),
                    },
                    None => {
                        ParamValue::Text(dialog.text(&sub.name).unwrap_or("").trim().to_owned())
                    }
                },
                _ => ParamValue::Text(dialog.text(&sub.name).unwrap_or("").trim().to_owned()),
            };
            values.insert(sub.name.clone(), v);
        }
        if let Err(e) = devtemplate::validate(&param.fields, &values) {
            self.dialogs.push(dialog);
            self.error(&e);
            return;
        }
        self.row_edit = None;
        let Some(form) = self.template.as_mut() else {
            return;
        };
        if let Some(rows) = form.list_rows_mut(field) {
            let at = match row {
                Some(r) if r < rows.len() => {
                    rows[r] = values;
                    r
                }
                _ => {
                    rows.push(values);
                    rows.len() - 1
                }
            };
            form.list_select(field, at);
        }
        form.refresh();
    }

    /// Show the outcome of installation steps (D-047) in one message.
    fn show_steps(&mut self, title: &str, steps: Vec<setup::StepOutcome>) {
        let failed = steps.iter().any(|s| !s.ok);
        let text = steps
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.dialogs.push(Dialog::message(title, &text, failed));
    }

    /// Settings → Show templates folder: where templates are read from. Over SSH the
    /// folders are on the machine running pgtc, so they are shown, not opened.
    fn show_templates_folder(&mut self) {
        let user = self.paths.user_templates_dir();
        let system = pgc_core::paths::system_templates_dir();
        let state = |p: &Path| if p.is_dir() { "" } else { "  (does not exist)" };
        let text = format!(
            "User templates (they win over the built-in ones):\n{}{}\n\nBuilt-in templates:\n{}{}{}",
            user.display(),
            state(&user),
            system.display(),
            state(&system),
            if system.is_dir() {
                ""
            } else {
                "\n(the templates embedded in pgtc are used)"
            }
        );
        self.dialogs
            .push(Dialog::message("Templates folders", &text, false));
    }

    // ---- result export (D-021, D-042) --------------------------------------------------

    /// Where the writer threads report to (set by the event loop).
    pub fn set_export_sink(&mut self, sink: ExportSink) {
        self.export_sink = Some(sink);
    }

    /// The export dialog; `name` / `options` keep the user's input when the dialog is
    /// reopened for another folder.
    fn open_export_dialog(&mut self, name: Option<String>, options: Option<ExportOptions>) {
        let Some(q) = self.query() else {
            return;
        };
        let set = q.active_result + 1;
        let o = options.unwrap_or(ExportOptions {
            format: self.settings.export.format,
            separator: self.settings.export.csv_separator,
            bom: self.settings.export.csv_bom,
        });
        let name = name.unwrap_or_else(|| format!("result_{set}.{}", o.format.extension()));
        let dir = self.file_dir.clone();
        let entries = list_dir(&dir, o.format.extension());
        let labels: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let marks: Vec<bool> = entries.iter().map(|e| e.2).collect();
        let formats = ExportFormat::ALL
            .iter()
            .map(|f| f.label().to_owned())
            .collect();
        let format_index = ExportFormat::ALL
            .iter()
            .position(|f| *f == o.format)
            .unwrap_or(0);
        let separators = vec![
            ", (comma)".to_owned(),
            "; (semicolon)".to_owned(),
            "Tab".to_owned(),
        ];
        let separator_index = match o.separator {
            CsvSeparator::Comma => 0,
            CsvSeparator::Semicolon => 1,
            CsvSeparator::Tab => 2,
        };
        self.dialogs.push(Dialog::new(
            DialogKind::ExportFile,
            "Export result",
            70,
            vec![
                Widget::label(&format!("Folder: {}", dir.display())),
                Widget::field("path", "&File name:", &name, 16),
                Widget::list("files", "", labels, 0, 12).highlighted(marks),
                Widget::combo("format", "For&mat:", formats, format_index, 16),
                Widget::combo(
                    "separator",
                    "CSV &separator:",
                    separators,
                    separator_index,
                    16,
                ),
                Widget::checkbox("bom", "CSV with UTF-8 &BOM", o.bom),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        ));
    }

    /// The file name's extension follows the chosen format (D-042).
    fn sync_export_dialog(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if d.kind != DialogKind::ExportFile {
            return;
        }
        let format = export_options(d).format;
        let name = d.text("path").unwrap_or("").to_owned();
        let path = Path::new(&name);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let known = ExportFormat::ALL.iter().any(|f| f.extension() == ext);
        if known && ext != format.extension() {
            let renamed = path.with_extension(format.extension());
            d.set_text("path", &renamed.to_string_lossy());
        }
    }

    fn start_export(&mut self, path: std::path::PathBuf, options: ExportOptions) {
        self.settings.export.format = options.format;
        self.settings.export.csv_separator = options.separator;
        self.settings.export.csv_bom = options.bom;
        self.save_settings();
        if let Some(dir) = path.parent() {
            self.file_dir = dir.to_path_buf();
        }
        let Some(sink) = self.export_sink.clone() else {
            self.error("Export is not available.");
            return;
        };
        self.next_export += 1;
        let id = self.next_export;
        let tab = self.tabs[self.active_tab].id;
        let Some(q) = self.query_mut() else {
            return;
        };
        let set = q.active_result;
        let Some(rs) = q.results.get(set) else {
            return;
        };
        let writer = crate::export::spawn(id, path.clone(), options, rs.columns.clone(), sink);
        q.export = Some(ExportRun {
            id,
            set,
            path: path.clone(),
            writer,
            next_loaded: 0,
            writing: false,
            awaiting_rows: false,
            from_stream: false,
            finishing: false,
            written: 0,
            abort_reason: None,
        });
        self.status = format!("Exporting to {}…", path.display());
        self.pump_export(tab);
    }

    /// Hands the next batch to the writer: first the rows in the grid, then rows taken
    /// from the paused stream, then `Finish` (D-021). One batch at a time.
    fn pump_export(&mut self, tab: u64) {
        let batch = self.settings.query.batch_rows.max(1) as usize;
        let Some(q) = self
            .tab_index(tab)
            .and_then(|i| self.tabs[i].query.as_mut())
        else {
            return;
        };
        let last = q.results.len().saturating_sub(1);
        let run = q.last_run;
        let Some(e) = q.export.as_mut() else {
            return;
        };
        if e.writing || e.awaiting_rows || e.finishing || e.abort_reason.is_some() {
            return;
        }
        let Some(rs) = q.results.get_mut(e.set) else {
            return;
        };
        if e.next_loaded < rs.rows.len() {
            let end = (e.next_loaded + batch).min(rs.rows.len());
            let rows = rs.rows[e.next_loaded..end].to_vec();
            e.next_loaded = end;
            e.writing = true;
            let _ = e.writer.send(ToWriter::Rows(rows));
        } else if e.set == last && rs.more {
            if rs.fetching {
                // a grid fetch is on its way: its rows land in the grid first
                return;
            }
            if let Some(conn) = q.conn.as_ref() {
                rs.fetching = true;
                e.awaiting_rows = true;
                e.from_stream = true;
                conn.handle.fetch_more(run);
            }
        } else {
            e.finishing = true;
            let _ = e.writer.send(ToWriter::Finish);
        }
    }

    fn on_export_event(&mut self, event: ExportEvent) {
        let id = match &event {
            ExportEvent::Written { id, .. }
            | ExportEvent::Done { id, .. }
            | ExportEvent::Failed { id, .. }
            | ExportEvent::Aborted { id } => *id,
        };
        let Some(i) = self.tabs.iter().position(|t| {
            t.query
                .as_ref()
                .and_then(|q| q.export.as_ref())
                .is_some_and(|e| e.id == id)
        }) else {
            return;
        };
        let tab = self.tabs[i].id;
        let active = i == self.active_tab;
        let q = self.tabs[i].query.as_mut().expect("query tab");
        if let ExportEvent::Written { rows, .. } = event {
            if let Some(e) = q.export.as_mut() {
                e.writing = false;
                e.written = rows;
            }
            self.pump_export(tab);
            return;
        }
        let e = q.export.take().expect("export");
        // its batch may still be on the way: not for the grid
        q.drop_rows = e.awaiting_rows;
        // rows were taken from a still paused result: end it (D-042)
        let last = q.results.len().saturating_sub(1);
        let paused = q.results.get(e.set).is_some_and(|rs| rs.more) && e.set == last;
        let discard = |q: &QueryState| {
            if let Some(conn) = q.conn.as_ref() {
                conn.handle.discard(q.last_run);
            }
        };
        match event {
            ExportEvent::Done { rows, .. } => {
                if e.from_stream
                    && let Some(rs) = q.results.get_mut(e.set)
                {
                    rs.exported_total = Some(rows);
                }
                let text = format!(
                    "Exported {} rows to {}",
                    crate::grid::thousands(rows as usize),
                    e.path.display()
                );
                q.info(text.clone());
                if active {
                    self.status = text;
                }
            }
            ExportEvent::Failed { error, .. } => {
                if e.from_stream && paused {
                    discard(q);
                }
                q.info(format!("Export failed; {} was deleted.", e.path.display()));
                if active {
                    self.status = "Export failed".to_owned();
                }
                self.error(&format!(
                    "{error}

The incomplete file was deleted."
                ));
            }
            ExportEvent::Aborted { .. } => {
                if e.from_stream && paused {
                    discard(q);
                }
                let reason = e
                    .abort_reason
                    .unwrap_or_else(|| "Export cancelled".to_owned());
                let text = format!("{reason}; {} was deleted.", e.path.display());
                q.info(text.clone());
                if active {
                    self.status = text;
                }
            }
            ExportEvent::Written { .. } => {}
        }
    }

    fn open_path(&mut self, path: std::path::PathBuf) {
        let text = match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(_) => {
                    self.error(&format!("{} is not a UTF-8 text file.", path.display()));
                    return;
                }
            },
            Err(e) => {
                self.error(&format!("Cannot open {}:\n{e}", path.display()));
                return;
            }
        };
        let mut view = TextView::new(false, self.tab_width());
        view.load(text);
        self.next_query += 1;
        let id = self.add_tab(TabKind::Query(self.next_query), view);
        if let Some(q) = self.tabs.last_mut().and_then(|t| t.query.as_mut()) {
            q.file = Some(path.clone());
        }
        self.bind_query_tab(id);
        self.status = format!("Opened {}", path.display());
    }

    fn save_to(&mut self, path: std::path::PathBuf) {
        let text = self.tabs[self.active_tab].view.editor.text();
        match std::fs::write(&path, text) {
            Ok(()) => {
                self.tabs[self.active_tab].view.editor.set_modified(false);
                if let Some(q) = self.query_mut() {
                    q.file = Some(path.clone());
                }
                if let Some(dir) = path.parent() {
                    self.file_dir = dir.to_path_buf();
                }
                self.status = format!("Saved {}", path.display());
                let id = self.tabs[self.active_tab].id;
                match self.after_save.take() {
                    Some(AfterSave::Close(tab)) if tab == id => self.request_close_tab(id, false),
                    Some(AfterSave::Bulk) => {
                        if let Some((b, _)) = self.bulk {
                            self.request_bulk(b);
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => self.error(&format!("Cannot save {}:\n{e}", path.display())),
        }
    }

    // ---- editor (D-015, D-038) ------------------------------------------------------

    fn tab_width(&self) -> usize {
        self.settings.editor.tab_width.max(1) as usize
    }

    fn editor(&mut self) -> &mut Editor {
        &mut self.tabs[self.active_tab].view.editor
    }

    /// Applies an editing action to the active tab's editor.
    fn edit(&mut self, action: Action) {
        if !self.editor().apply(action) {
            self.status = "Read-only view".to_owned();
        }
    }

    /// Copy (or cut) the selection to the clipboard (D-023).
    fn copy(&mut self, cut: bool) {
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
    fn open_find(&mut self) {
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

    fn find_dialog_done(&mut self, dialog: Dialog, button: &str) {
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
    fn find_again(&mut self, forward: bool) {
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

    fn replace_confirm_done(&mut self, button: &str) {
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

    // ---- connections ----    // ---- connections --------------------------------------------------------------

    fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(
            self.settings.connection.connect_timeout_secs.max(1),
        ))
    }

    fn pgpass_password(conn: &SavedConnection, database: &str) -> Option<String> {
        let path = paths::pgpass_file()?;
        pgpass::read_password(
            &path,
            PgPassKey {
                host: &conn.host,
                port: conn.port,
                database,
                username: &conn.username,
            },
        )
    }

    fn connect_server(&mut self, uid: u64) {
        let timeout = self.timeout();
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        if node.state != ConnState::Disconnected {
            return;
        }
        let password = node
            .password
            .clone()
            .or_else(|| Self::pgpass_password(&node.conn, &node.conn.database));
        // Kept for the session: per-database connections reuse it (D-035).
        node.password = password.clone();
        let params = ConnectParams::new(&node.conn, &node.conn.database, password, timeout);
        self.next_conn += 1;
        let id = self.next_conn;
        node.state = ConnState::Connecting;
        node.handle = Some(pgc_db::spawn(id, params, self.sink.clone()));
        self.status = format!("Connecting to {}…", node.conn.name);
        self.connections.insert(id, Target::Server(uid));
    }

    fn connect_database(&mut self, nid: u64) {
        let timeout = self.timeout();
        let Some(si) = self.tree.server_of(nid) else {
            return;
        };
        let s = &self.tree.servers[si];
        let (uid, conn, password) = (s.uid, s.conn.clone(), s.password.clone());
        let Some(node) = self.tree.node_mut(nid) else {
            return;
        };
        let name = node.name.clone();
        let password = password.or_else(|| Self::pgpass_password(&conn, &name));
        let params = ConnectParams::new(&conn, &name, password, timeout);
        self.next_conn += 1;
        let id = self.next_conn;
        node.conn = Some(DbConn {
            state: ConnState::Connecting,
            handle: Some(pgc_db::spawn(id, params, self.sink.clone())),
        });
        self.connections.insert(id, Target::Database(uid, nid));
    }

    fn disconnect_server(&mut self, uid: u64) {
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        for handle in node.reset() {
            handle.close();
            self.connections.remove(&handle.id);
        }
    }

    fn on_db(&mut self, event: DbEvent) {
        let id = match &event {
            DbEvent::Connected { id, .. }
            | DbEvent::ConnectFailed { id, .. }
            | DbEvent::Nodes { id, .. }
            | DbEvent::Definition { id, .. }
            | DbEvent::Query { id, .. }
            | DbEvent::Notice { id, .. }
            | DbEvent::Lookup { id, .. }
            | DbEvent::Model { id, .. }
            | DbEvent::Closed { id, .. } => *id,
        };
        // Events of connections closed on request are no longer interesting.
        let Some(target) = self.connections.get(&id).cloned() else {
            return;
        };
        match (target, event) {
            (Target::Query(tab), event) => self.on_query_event(id, tab, event),
            (Target::Structure, event) => self.on_job_event(id, event),
            (
                _,
                DbEvent::Query { .. }
                | DbEvent::Notice { .. }
                | DbEvent::Lookup { .. }
                | DbEvent::Model { .. },
            ) => {}
            (_, DbEvent::Nodes { key, result, .. }) => {
                let Some(nid) = self.pending.remove(&key) else {
                    return;
                };
                match result {
                    Ok(rows) => self.tree.set_children(nid, rows),
                    Err(e) => {
                        let label = self.tree.node(nid).map(|n| n.label.clone());
                        if let Some(n) = self.tree.node_mut(nid) {
                            n.loading = false;
                            n.expanded = false;
                        }
                        if let Some(label) = label {
                            self.error(&format!("Cannot load {label}:\n{e}"));
                        }
                    }
                }
            }
            (_, DbEvent::Definition { key, result, .. }) => self.definition_loaded(key, result),
            (Target::Server(uid), DbEvent::Connected { version, .. }) => {
                self.server_connected(uid, version);
            }
            (Target::Server(uid), DbEvent::ConnectFailed { error, .. }) => {
                self.connections.remove(&id);
                self.server_failed(uid, error);
            }
            (Target::Server(uid), DbEvent::Closed { error, .. }) => {
                self.connections.remove(&id);
                if let Some(node) = self.tree.by_uid(uid) {
                    let name = node.conn.name.clone();
                    for handle in node.reset() {
                        handle.close();
                        self.connections.remove(&handle.id);
                    }
                    self.status = match error {
                        Some(e) => format!("Connection to {name} lost: {e}"),
                        None => format!("Disconnected from {name}"),
                    };
                }
            }
            (Target::Database(_, nid), event) => {
                let Some(name) = self.tree.node(nid).map(|n| n.name.clone()) else {
                    // The database disappeared from the tree (refresh); its connection
                    // was dropped with the node.
                    self.connections.remove(&id);
                    return;
                };
                match event {
                    DbEvent::Connected { .. } => {
                        let groups = self.tree.groups(&ObjType::Database.children(false));
                        if let Some(n) = self.tree.node_mut(nid) {
                            if let Some(c) = &mut n.conn {
                                c.state = ConnState::Connected;
                            }
                            n.children = Some(groups);
                        }
                    }
                    DbEvent::ConnectFailed { error, .. } => {
                        self.connections.remove(&id);
                        self.database_reset(nid);
                        self.error(&format!("Cannot connect to database {name}:\n{error}"));
                    }
                    DbEvent::Closed { error, .. } => {
                        self.connections.remove(&id);
                        self.database_reset(nid);
                        if let Some(e) = error {
                            self.status = format!("Connection to database {name} lost: {e}");
                        }
                    }
                    DbEvent::Nodes { .. }
                    | DbEvent::Definition { .. }
                    | DbEvent::Query { .. }
                    | DbEvent::Notice { .. }
                    | DbEvent::Lookup { .. }
                    | DbEvent::Model { .. } => {}
                }
            }
        }
    }

    fn database_reset(&mut self, nid: u64) {
        if let Some(n) = self.tree.node_mut(nid) {
            n.conn = Some(DbConn {
                state: ConnState::Disconnected,
                handle: None,
            });
            n.expanded = false;
            n.children = None;
        }
    }

    fn server_connected(&mut self, uid: u64, version: pgc_core::ServerVersion) {
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        node.state = ConnState::Connected;
        node.version = Some(version);
        let name = node.conn.name.clone();
        let mut status = format!("Connected to {name} (PostgreSQL {version})");
        if version.is_unverified() {
            status = format!(
                "Connected to {name} — PostgreSQL {} – not yet verified with pgtc",
                version.major()
            );
        }
        // D-035: write .pgpass only after a successful connect.
        if node.save_password
            && let Some(password) = node.password.clone()
        {
            node.save_password = false;
            let conn = node.conn.clone();
            if let Err(e) = Self::save_pgpass(&conn, &password) {
                tracing::error!("saving password: {e}");
                status = format!("{status}; password not saved: {e}");
            } else {
                status = format!("{status}; password saved");
            }
        }
        self.status = status;
        // Groups appear; Databases is opened at once.
        self.tree.server_connected(uid);
        if let Some(databases) = self
            .tree
            .by_uid(uid)
            .and_then(|s| s.children.first())
            .map(|n| n.nid)
        {
            self.expand_node(databases);
        }
    }

    fn save_pgpass(conn: &SavedConnection, password: &str) -> Result<(), DbError> {
        let path = paths::pgpass_file()
            .ok_or_else(|| DbError::Other("cannot locate the .pgpass file".to_owned()))?;
        pgpass::save_password(
            &path,
            PgPassKey {
                host: &conn.host,
                port: conn.port,
                database: &conn.database,
                username: &conn.username,
            },
            password,
        )
    }

    fn server_failed(&mut self, uid: u64, error: DbError) {
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        node.handle = None;
        node.state = ConnState::Disconnected;
        let name = node.conn.name.clone();
        match error {
            DbError::PasswordRequired if node.never_prompt => {
                self.status = format!("Cannot connect to {name}");
                self.error(&format!(
                    "Cannot connect to {name}:\nthe server requires a password and -w was given"
                ));
            }
            DbError::PasswordRequired => self.open_login(uid, None),
            DbError::AuthFailed(msg) => {
                node.password = None;
                self.open_login(uid, Some(msg));
            }
            DbError::Other(msg) => {
                self.status = format!("Cannot connect to {name}");
                self.error(&format!("Cannot connect to {name}:\n{msg}"));
            }
        }
    }

    // ---- dialogs ------------------------------------------------------------------

    fn error(&mut self, text: &str) {
        self.dialogs.push(Dialog::message("Error", text, true));
    }

    fn dialog_event(&mut self, event: DialogEvent) {
        match event {
            DialogEvent::None => {}
            DialogEvent::Cancelled => {
                self.dialogs.pop();
            }
            DialogEvent::Button(id) => {
                let Some(dialog) = self.dialogs.pop() else {
                    return;
                };
                match (dialog.kind, id) {
                    (DialogKind::Settings, "ok") => self.apply_settings(dialog),
                    (DialogKind::PsqlCommand, "ok") => {
                        let text = dialog.text("cmd").unwrap_or("").to_owned();
                        self.run_psql(dialog, &text);
                    }
                    (DialogKind::Connection, "ok") => self.apply_connection(dialog),
                    (DialogKind::Login, "ok") => self.apply_login(&dialog),
                    (DialogKind::ConfirmDelete, "yes") => self.delete_server(dialog.tag),
                    (DialogKind::Find, "cancel") => {}
                    (DialogKind::Find, button) => self.find_dialog_done(dialog, button),
                    (DialogKind::ReplaceConfirm, button) => self.replace_confirm_done(button),
                    (DialogKind::SaveChanges, "yes") => {
                        self.save_before(dialog.tag, AfterSave::Close(dialog.tag));
                    }
                    (DialogKind::QuitSaveChanges, "yes") => {
                        self.save_before(dialog.tag, AfterSave::Bulk);
                    }
                    (DialogKind::QuitSaveChanges, "no") => {
                        if let Some((b, skip)) = self.bulk.as_mut() {
                            skip.push(dialog.tag);
                            let b = *b;
                            self.request_bulk(b);
                        }
                    }
                    (DialogKind::SaveChanges, "no") => self.request_close_tab(dialog.tag, false),
                    (DialogKind::CloseTransaction, "commit") => {
                        self.finish_transaction(dialog.tag, AfterRun::CloseTab);
                    }
                    (DialogKind::CloseTransaction, "rollback") => self.close_tab(dialog.tag),
                    (DialogKind::QuitTransactions, "commit") => {
                        if let Some((b, _)) = self.bulk.take() {
                            self.bulk_pending = Some(b);
                            for id in self.bulk_transactions(b) {
                                self.finish_transaction(id, AfterRun::Bulk);
                            }
                            self.maybe_finish_bulk();
                        }
                    }
                    (DialogKind::QuitTransactions, "rollback") => {
                        // closing the connections rolls the transactions back
                        if let Some((b, _)) = self.bulk.take() {
                            self.finish_bulk(b);
                        }
                    }
                    (DialogKind::CancelWait, "close") => self.abort_query_conn(dialog.tag),
                    (DialogKind::OpenFile | DialogKind::SaveFile, "ok") => {
                        self.file_dialog_done(dialog);
                    }
                    (DialogKind::ExportFile, "ok") => self.file_dialog_done(dialog),
                    (DialogKind::SaveDefinition | DialogKind::OpenDefinition, "ok") => {
                        self.definition_dialog_done(dialog);
                    }
                    (DialogKind::ConfirmDefinitionOverwrite, "yes") => {
                        if let Some(path) = self.pending_definition.take() {
                            self.start_job(JobKind::Export(path));
                        }
                    }
                    (DialogKind::TemplatePicker, "ok") => self.template_chosen(dialog),
                    (DialogKind::TemplateRow, "ok") => self.row_done(dialog),
                    (DialogKind::ConfirmExportOverwrite, "yes") => {
                        if let Some((path, options)) = self.pending_export.take() {
                            self.start_export(path, options);
                        }
                    }
                    (DialogKind::ConfirmOverwrite, "yes") => {
                        if let Some(path) = self.pending_save.take() {
                            self.save_to(path);
                        }
                    }
                    _ => {}
                }
            }
        }
        // a save for closing / quitting that was cancelled or failed: the tab stays
        // open, quitting stops
        let saving = self
            .dialogs
            .iter()
            .any(|d| matches!(d.kind, DialogKind::SaveFile | DialogKind::ConfirmOverwrite));
        if !saving {
            self.after_save = None;
        }
        if !saving
            && !self.dialogs.iter().any(|d| {
                matches!(
                    d.kind,
                    DialogKind::QuitSaveChanges | DialogKind::QuitTransactions
                )
            })
        {
            self.bulk = None;
        }
    }

    /// Settings dialog (D-051): one column, groups separated by an empty row.
    fn open_settings(&mut self) {
        let names = self.themes.names();
        let selected = self
            .themes
            .index_of(&self.settings.ui.color_scheme)
            .unwrap_or(0);
        let s = &self.settings;
        let timeout = s.connection.connect_timeout_secs.to_string();
        let batch = s.query.batch_rows.to_string();
        let wait = s.query.cancel_wait_secs.to_string();
        self.dialogs.push(Dialog::new(
            DialogKind::Settings,
            "Settings",
            50,
            vec![
                Widget::checkbox("icons", "Show &icons (needs a Nerd Font)", s.ui.icons),
                Widget::label(&format!("    Sample: {}", crate::icons::sample())),
                Widget::label("    Boxes or ? here: no Nerd Font."),
                Widget::combo("scheme", "Colour &scheme:", names, selected, 22),
                Widget::checkbox("ascii", "ASCII &frames (+ - |)", s.ui.ascii_frames),
                Widget::label(""),
                Widget::field("timeout", "Connect &timeout (s):", &timeout, 22),
                Widget::checkbox(
                    "system",
                    "Show s&ystem objects in the tree",
                    s.ui.show_system_objects,
                ),
                Widget::label(""),
                Widget::field("batch", "&Rows per batch:", &batch, 22),
                Widget::field("wait", "Cancel &wait (s):", &wait, 22),
                Widget::checkbox(
                    "auto_commit",
                    "Auto co&mmit in new query tabs",
                    s.query.auto_commit,
                ),
                Widget::checkbox(
                    "auto_rollback",
                    "Auto roll&back on error in new tabs",
                    s.query.auto_rollback,
                ),
                Widget::checkbox(
                    "ask_save",
                    "&Ask to save query tabs when closing",
                    s.query.ask_save,
                ),
                Widget::label(""),
                Widget::checkbox("clip_system", "System c&lipboard", s.clipboard.system),
                Widget::checkbox(
                    "clip_osc52",
                    "OSC &52 clipboard (terminal, SSH)",
                    s.clipboard.osc52,
                ),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        ));
    }

    fn apply_settings(&mut self, dialog: Dialog) {
        // numbers first: a wrong one keeps the dialog open (D-051)
        let number = |id: &str, max: u32| -> Option<u32> {
            match dialog.text(id).unwrap_or("").trim().parse::<u32>() {
                Ok(n) if (1..=max).contains(&n) => Some(n),
                _ => None,
            }
        };
        let checks = [
            (
                "timeout",
                3600,
                "Connect timeout must be a number of seconds from 1 to 3600.",
            ),
            (
                "batch",
                1_000_000,
                "Rows per batch must be a number from 1 to 1000000.",
            ),
            (
                "wait",
                3600,
                "Cancel wait must be a number of seconds from 1 to 3600.",
            ),
        ];
        let mut values = [0u32; 3];
        for (i, (id, max, text)) in checks.iter().enumerate() {
            match number(id, *max) {
                Some(n) => values[i] = n,
                None => {
                    self.dialogs.push(dialog);
                    self.dialogs.last_mut().expect("just pushed").focus_on(id);
                    self.error(text);
                    return;
                }
            }
        }
        let [timeout, batch, wait] = values;
        self.settings.connection.connect_timeout_secs = timeout;
        self.settings.query.batch_rows = batch;
        self.settings.query.cancel_wait_secs = wait;
        self.settings.query.auto_commit = dialog.checked("auto_commit");
        self.settings.query.auto_rollback = dialog.checked("auto_rollback");
        self.settings.ui.icons = dialog.checked("icons");
        self.settings.ui.ascii_frames = dialog.checked("ascii");
        self.settings.query.ask_save = dialog.checked("ask_save");
        let clipboard = pgc_core::settings::ClipboardSettings {
            system: dialog.checked("clip_system"),
            osc52: dialog.checked("clip_osc52"),
        };
        if clipboard != self.settings.clipboard {
            self.clipboard.apply(&clipboard);
            self.settings.clipboard = clipboard;
        }
        if dialog.checked("system") != self.settings.ui.show_system_objects {
            // reloads the loaded parts of every tree (D-036)
            self.toggle_system_objects();
        }
        if let Some(i) = dialog.selected("scheme") {
            let names = self.themes.names();
            self.settings.ui.color_scheme = names[i].clone();
        }
        self.theme = self.current_theme();
        if self.save_settings() {
            self.status = "Settings saved".to_owned();
        }
    }

    /// The chosen colour scheme with the frame style of the settings (D-051).
    fn current_theme(&self) -> Theme {
        let mut theme = self.themes.get(&self.settings.ui.color_scheme).clone();
        theme.ascii = self.settings.ui.ascii_frames;
        theme
    }

    /// Saves `settings.toml`; shows an error dialog on failure.
    fn save_settings(&mut self) -> bool {
        let file = self.paths.settings_file();
        match self.settings.save(&file) {
            Ok(()) => true,
            Err(e) => {
                tracing::error!("saving settings: {e:#}");
                self.error(&format!("Cannot save {}:\n{e}", file.display()));
                false
            }
        }
    }

    fn persist_servers(&mut self) {
        self.settings.servers = self.tree.saved();
        self.save_settings();
    }

    /// New (`uid` = None) or Edit connection dialog (D-035).
    fn open_connection_dialog(&mut self, uid: Option<u64>) {
        let conn = uid
            .and_then(|u| self.tree.by_uid(u).map(|n| n.conn.clone()))
            .unwrap_or_default();
        let name = if uid.is_some() {
            conn.name.as_str()
        } else {
            ""
        };
        let modes: Vec<String> = SslMode::ALL.iter().map(|m| m.name().to_owned()).collect();
        let mode = SslMode::ALL
            .iter()
            .position(|m| *m == conn.ssl_mode)
            .unwrap_or(1);
        let lw = CONN_LABEL_WIDTH;
        let title = if uid.is_some() {
            "Edit connection"
        } else {
            "New connection"
        };
        let dialog = Dialog::new(
            DialogKind::Connection,
            title,
            64,
            vec![
                Widget::field("name", "&Name:", name, lw),
                Widget::label(""),
                Widget::field("host", "&Host:", &conn.host, lw),
                Widget::field("port", "&Port:", &conn.port.to_string(), lw),
                Widget::field("database", "Maintenance &DB:", &conn.database, lw),
                Widget::field("user", "&User name:", &conn.username, lw),
                Widget::field("password", "Pass&word:", "", lw).masked(),
                Widget::checkbox("save", "&Save password", false),
                Widget::label(""),
                Widget::combo("ssl", "SS&L mode:", modes, mode, lw),
                Widget::field(
                    "rootcert",
                    "Root cer&t. file:",
                    conn.ssl_root_cert.as_deref().unwrap_or(""),
                    lw,
                ),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        )
        .with_tag(uid.unwrap_or(0));
        self.dialogs.push(dialog);
    }

    fn apply_connection(&mut self, dialog: Dialog) {
        let text = |id: &str| dialog.text(id).unwrap_or("").trim().to_owned();
        let port = match text("port").parse::<u16>() {
            Ok(p) if p > 0 => p,
            _ => {
                let mut dialog = dialog;
                dialog.focus_on("port");
                self.dialogs.push(dialog);
                self.error("Port must be a number from 1 to 65535.");
                return;
            }
        };
        let host = text("host");
        let username = text("user");
        if host.is_empty() || username.is_empty() {
            let mut dialog = dialog;
            dialog.focus_on(if host.is_empty() { "host" } else { "user" });
            self.dialogs.push(dialog);
            self.error("Host and user name are required.");
            return;
        }
        let mut database = text("database");
        if database.is_empty() {
            database = "postgres".to_owned();
        }
        let mut name = text("name");
        if name.is_empty() {
            name = format!("{username}@{host}:{port}");
        }
        let rootcert = text("rootcert");
        let conn = SavedConnection {
            name,
            host,
            port,
            database,
            username,
            ssl_mode: dialog
                .selected("ssl")
                .map_or(SslMode::Prefer, |i| SslMode::ALL[i]),
            ssl_root_cert: (!rootcert.is_empty()).then_some(rootcert),
        };
        // The password is not trimmed: spaces may be part of it.
        let password = dialog
            .text("password")
            .filter(|p| !p.is_empty())
            .map(str::to_owned);
        let save = dialog.checked("save");

        if dialog.tag == 0 {
            let uid = self.tree.add(conn);
            if let Some(node) = self.tree.by_uid(uid) {
                node.password = password;
                node.save_password = save;
            }
            self.persist_servers();
            self.tree.select_server(uid);
            self.focus = Focus::Tree;
            self.connect_server(uid);
        } else if let Some(node) = self.tree.by_uid(dialog.tag) {
            let connected = node.state != ConnState::Disconnected;
            node.conn = conn;
            if password.is_some() {
                node.password = password;
            }
            node.save_password = save;
            self.persist_servers();
            self.status = if connected {
                "Connection saved; changes apply on the next connect".to_owned()
            } else {
                "Connection saved".to_owned()
            };
        }
    }

    fn open_login(&mut self, uid: u64, error: Option<String>) {
        let Some(node) = self.tree.by_uid(uid) else {
            return;
        };
        let mut text = format!(
            "Connection: {}\nServer:     {}:{}\nDatabase:   {}\nUser name:  {}",
            node.conn.name, node.conn.host, node.conn.port, node.conn.database, node.conn.username
        );
        if let Some(e) = &error {
            text = format!("{e}\n\n{text}");
        }
        self.dialogs.push(
            Dialog::new(
                DialogKind::Login,
                "Login",
                60,
                vec![
                    Widget::label(&text),
                    Widget::label(""),
                    Widget::field("password", "&Password:", "", 11).masked(),
                    Widget::checkbox("save", "&Save password", false),
                    Widget::button("ok", "&OK", true),
                    Widget::button("cancel", "&Cancel", false),
                ],
            )
            .with_tag(uid),
        );
        self.status = "Password required".to_owned();
    }

    fn apply_login(&mut self, dialog: &Dialog) {
        let uid = dialog.tag;
        let password = dialog.text("password").unwrap_or("").to_owned();
        let save = dialog.checked("save");
        if let Some(node) = self.tree.by_uid(uid) {
            node.password = Some(password);
            node.save_password |= save;
        }
        self.connect_server(uid);
    }

    fn confirm_delete(&mut self) {
        let Some(node) = self.tree.selected_server() else {
            return;
        };
        let text = format!("Delete connection \"{}\"?", node.conn.name);
        let uid = node.uid;
        self.dialogs.push(
            Dialog::confirm(DialogKind::ConfirmDelete, "Delete connection", &text).with_tag(uid),
        );
    }

    fn delete_server(&mut self, uid: u64) {
        self.disconnect_server(uid);
        if let Some(node) = self.tree.remove(uid) {
            self.status = format!("Connection \"{}\" deleted", node.conn.name);
        }
        self.persist_servers();
    }

    // ---- mouse --------------------------------------------------------------------

    fn on_mouse(&mut self, m: MouseEvent, hits: &Hits) {
        // any mouse action closes the autocomplete list (D-052)
        if !matches!(m.kind, MouseEventKind::Moved) {
            self.completion = None;
        }
        let p = Position::new(m.column, m.row);
        if self.template.is_some() && self.dialogs.is_empty() {
            // an open choice list takes every click and the wheel
            if let Some(form) = self.template.as_mut()
                && form.popup_mouse(m.kind, p)
            {
                return;
            }
            if m.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some((stop, area)) = hits.template.iter().find(|(_, r)| r.contains(p))
                && let Some(form) = self.template.as_mut()
            {
                let event = form.click(*stop, p.y, *area);
                self.form_event(event);
            }
            return;
        }
        // the wheel scrolls an open combo list of a dialog
        if matches!(
            m.kind,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
        ) && let Some(d) = self.dialogs.last_mut()
        {
            let delta = if m.kind == MouseEventKind::ScrollDown {
                3
            } else {
                -3
            };
            // an open combo list first, else a Help window's text
            if !d.combo_wheel(delta) && !d.text_wheel(delta) {
                d.list_wheel(delta);
            }
            return;
        }
        // the border between the tree and the tabs is dragged to resize them (D-045)
        match m.kind {
            MouseEventKind::Down(MouseButton::Left)
                if !self.modal() && hits.pane_border.contains(p) =>
            {
                self.pane_drag = true;
                return;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.pane_drag => {
                let wanted = (p.x + 1).saturating_sub(hits.main.x);
                self.settings.ui.tree_width =
                    Some(crate::view::tree_width(Some(wanted), hits.main.width));
                return;
            }
            MouseEventKind::Up(_) if self.pane_drag => {
                self.pane_drag = false;
                self.save_settings();
                return;
            }
            _ => {}
        }
        // the results bar is the drag handle of the editor / results split (D-019)
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.split_drag = !self.modal() && hits.split.contains(p);
            }
            MouseEventKind::Drag(MouseButton::Left) if self.split_drag => {
                self.drag_split(p.y, hits.split_area);
                return;
            }
            MouseEventKind::Up(_) => self.split_drag = false,
            _ => {}
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left)
                if !self.modal() && self.editor_area.contains(p) =>
            {
                self.focus = Focus::Tabs;
                self.click_editor(p, false);
            }
            MouseEventKind::Drag(MouseButton::Left)
                if !self.modal() && self.focus == Focus::Tabs && self.editor_area.contains(p) =>
            {
                self.click_editor(p, true);
            }
            MouseEventKind::ScrollDown if !self.modal() && self.editor_area.contains(p) => {
                self.editor().scroll_lines(3);
            }
            MouseEventKind::ScrollUp if !self.modal() && self.editor_area.contains(p) => {
                self.editor().scroll_lines(-3);
            }
            MouseEventKind::Down(MouseButton::Left)
                if !self.modal() && hits.result_bar.iter().any(|(_, r)| r.contains(p)) =>
            {
                if let Some((item, _)) = hits.result_bar.iter().find(|(_, r)| r.contains(p)) {
                    let item = *item;
                    self.focus = Focus::Results;
                    self.toggle_query(|q| {
                        match item {
                            crate::view::BarItem::Results => q.show_messages = false,
                            crate::view::BarItem::Messages => q.show_messages = true,
                            crate::view::BarItem::Set(i) => {
                                q.show_messages = false;
                                q.active_result = i;
                            }
                        }
                        String::new()
                    });
                }
            }
            MouseEventKind::Down(MouseButton::Left) if !self.modal() && hits.grid.contains(p) => {
                self.focus = Focus::Results;
                self.click_results(p, hits);
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if !self.modal() && hits.grid.contains(p) =>
            {
                let d = if m.kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if let Some(rs) = self.active_result_set() {
                    rs.move_cursor(d, 0, 0);
                }
                self.after_grid_move();
            }
            MouseEventKind::Down(MouseButton::Left) => self.left_click(p, hits),
            MouseEventKind::Down(MouseButton::Right) if !self.modal() && hits.grid.contains(p) => {
                self.focus = Focus::Results;
                self.click_results(p, hits);
                self.open_results_menu((p.x, p.y + 1));
            }
            MouseEventKind::Down(MouseButton::Right) if !self.modal() && hits.tree.contains(p) => {
                self.focus = Focus::Tree;
                if let Some(i) = self.tree_row_at(p, hits) {
                    self.tree.cursor = i;
                }
                self.run(Command::ContextMenu);
            }
            _ => {}
        }
    }

    /// Moves the editor / results split so that the results bar is on row `y`.
    fn drag_split(&mut self, y: u16, area: Rect) {
        if area.height < 8 {
            return;
        }
        let results_h = area.bottom().saturating_sub(y).clamp(4, area.height - 3);
        // rounded up: the view rounds down, so the bar lands exactly on `y`
        let percent = (results_h as u32 * 100).div_ceil(area.height as u32) as u16;
        if let Some(q) = self.query_mut() {
            q.results_percent = percent.clamp(15, 85);
        }
    }

    /// Mouse in the editor: place the cursor (`select`: extend the selection).
    fn click_editor(&mut self, p: Position, select: bool) {
        let line_numbers = self.settings.editor.line_numbers;
        let area = self.editor_area;
        let editor = self.editor();
        let gutter = pgc_editor::gutter_width(editor, line_numbers);
        let line = editor.top + (p.y - area.y) as usize;
        if line >= editor.len_lines() {
            let end = editor.line_to_char(editor.len_lines() - 1)
                + editor.line(editor.len_lines() - 1).chars().count();
            editor.set_cursor(end, select);
            return;
        }
        let col = editor.left + p.x.saturating_sub(area.x + gutter) as usize;
        let pos = editor.pos_at_col(line, col);
        editor.set_cursor(pos, select);
    }

    /// Index of the tree row at `p`, if any.
    fn tree_row_at(&self, p: Position, hits: &Hits) -> Option<usize> {
        let first = hits.tree.y + 1;
        let last = hits.tree.bottom().saturating_sub(1);
        if p.y < first || p.y >= last {
            return None;
        }
        let i = self.tree.offset + (p.y - first) as usize;
        (i < self.tree.rows().len()).then_some(i)
    }

    fn left_click(&mut self, p: Position, hits: &Hits) {
        let fkey = hits
            .fkeys
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(k, _)| *k);

        if let Some(dialog) = self.dialogs.last_mut() {
            if let Some((outer, _)) = &hits.dialog
                && dialog.combo_click(*outer, hits.screen, p.x, p.y)
            {
                // the click went to (or closed) an open combo list
            } else if let Some((_, areas)) = &hits.dialog
                && let Some(i) = areas.iter().position(|a| a.contains(p))
            {
                let event = dialog.click(i, areas[i], p.x, p.y);
                self.dialog_event(event);
            } else if let Some(k) = fkey {
                self.dispatch(KeyEvent::new(k.code, k.mods));
            }
            return;
        }

        if let Some(popup) = self.popup.take() {
            if let Some((i, _)) = hits.popup_items.iter().find(|(_, r)| r.contains(p))
                && let Some(MenuEntry::Item { cmd, .. }) = popup.entries.get(*i)
                && self.is_enabled(*cmd)
            {
                self.run(*cmd);
            } else if hits.popup.is_some_and(|r: Rect| r.contains(p)) {
                self.popup = Some(popup);
            }
            return;
        }

        let title = hits.menu.titles.iter().position(|r| r.contains(p));
        if let Some(state) = self.menu {
            if let Some(i) = title {
                if i == state.menu {
                    self.menu = None;
                } else {
                    self.open_menu(i);
                }
            } else if let Some((i, _)) = hits.menu.items.iter().find(|(_, r)| r.contains(p)) {
                if let Some(c) = menu::entry_command(&self.menus, state.menu, *i)
                    && self.is_enabled(c)
                {
                    self.menu = None;
                    self.run(c);
                }
            } else if !hits.menu.dropdown.is_some_and(|r: Rect| r.contains(p)) {
                self.menu = None;
            }
            return;
        }

        if let Some(i) = title {
            self.open_menu(i);
        } else if let Some(&(i, _)) = hits.tab_close.iter().find(|(_, r)| r.contains(p)) {
            // the tab's close mark: as Ctrl+W, with its questions (D-043)
            if let Some(id) = self.tabs.get(i).map(|t| t.id) {
                self.active_tab = i;
                self.focus = Focus::Tabs;
                self.request_close_tab(id, true);
            }
        } else if let Some(i) = hits.tabs.iter().position(|r| r.contains(p)) {
            self.active_tab = i;
            self.focus = Focus::Tabs;
        } else if let Some(k) = fkey {
            self.dispatch(KeyEvent::new(k.code, k.mods));
        } else if hits.tree.contains(p) {
            self.focus = Focus::Tree;
            if let Some(i) = self.tree_row_at(p, hits) {
                if self.tree.cursor == i {
                    self.toggle();
                } else {
                    self.tree.cursor = i;
                }
            }
        } else if hits.right.contains(p) {
            self.focus = Focus::Tabs;
        }
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Entries of `dir` for the file dialog: `(label, is_dir, highlight)`; `../` first,
/// then directories, then files; `*.sql` highlighted (D-041).
/// The template picker (D-046): the library, (name, description) per entry, the rows
/// shown (`None` = folder row) and the filter they were built for.
struct Picker {
    lib: TemplateLibrary,
    names: Vec<(String, String)>,
    rows: Vec<Option<usize>>,
    filter: Option<String>,
}

/// What a template lookup fills: a choice field, or a choice field of list rows
/// (list name, field name); how the rows become values.
#[derive(Debug, Clone)]
enum LookupTarget {
    Field(String, devtemplate::Shape),
    Row(String, String, devtemplate::Shape),
    /// Columns for a generate command of query tab `tab` (D-054).
    Generate {
        tab: u64,
        what: crate::psql::Gen,
        shown: String,
    },
    /// Autocomplete names of query tab `tab` (D-052).
    Names {
        tab: u64,
        generation: u64,
        need: crate::completion::Need,
    },
}

fn shape(p: &devtemplate::Param) -> devtemplate::Shape {
    p.source
        .map_or(devtemplate::Shape::Name, devtemplate::Source::shape)
}

fn sql_of(p: &devtemplate::Param) -> String {
    match (p.source, &p.query) {
        (Some(s), _) => s.sql().to_owned(),
        (None, Some(q)) => q.clone(),
        (None, None) => "SELECT NULL WHERE false".to_owned(),
    }
}

/// Picker rows: folders (indented) with their templates; with a filter only the
/// matching templates and their folders. Returns rows, labels, highlight marks.
fn picker_rows(p: &Picker, filter: &str) -> (Vec<Option<usize>>, Vec<String>, Vec<bool>) {
    let mut rows = Vec::new();
    let mut labels = Vec::new();
    let mut marks = Vec::new();
    let mut open: Vec<String> = Vec::new();
    for (i, e) in p.lib.entries.iter().enumerate() {
        let (name, desc) = &p.names[i];
        if !filter.is_empty()
            && !name.to_lowercase().contains(filter)
            && !desc.to_lowercase().contains(filter)
            && !e.rel.to_lowercase().contains(filter)
        {
            continue;
        }
        let folders: Vec<String> = e.folders().into_iter().map(str::to_owned).collect();
        let common = open
            .iter()
            .zip(&folders)
            .take_while(|(a, b)| a == b)
            .count();
        for (depth, f) in folders.iter().enumerate().skip(common) {
            rows.push(None);
            labels.push(format!("{}▾ {f}", "  ".repeat(depth)));
            marks.push(true);
        }
        open = folders;
        rows.push(Some(i));
        labels.push(format!("{}  {name}", "  ".repeat(open.len())));
        marks.push(false);
    }
    (rows, labels, marks)
}

/// Format, separator and BOM chosen in the export dialog.
fn export_options(d: &Dialog) -> ExportOptions {
    ExportOptions {
        format: ExportFormat::ALL[d.selected("format").unwrap_or(0).min(2)],
        separator: match d.selected("separator") {
            Some(1) => CsvSeparator::Semicolon,
            Some(2) => CsvSeparator::Tab,
            _ => CsvSeparator::Comma,
        },
        bom: d.checked("bom"),
    }
}

/// Rows the export asked for go to its writer, not to the grid (D-042). Returns the
/// event when it is for the grid / messages.
fn take_export_rows(q: &mut QueryState, event: QueryEvent) -> Option<QueryEvent> {
    let last = q.results.len().saturating_sub(1);
    let ended = q.export.is_none() && q.drop_rows;
    let export = q
        .export
        .as_mut()
        .filter(|e| e.awaiting_rows && e.set == last);
    if export.is_none() && !ended {
        return Some(event);
    }
    match event {
        QueryEvent::Rows { rows, more } => {
            match export {
                Some(e) => {
                    e.awaiting_rows = false;
                    if e.abort_reason.is_none() {
                        e.writing = true;
                        let _ = e.writer.send(ToWriter::Rows(rows));
                    }
                }
                None => q.drop_rows = false,
            }
            if let Some(rs) = q.results.last_mut() {
                rs.more = more;
                rs.fetching = false;
            }
            if let Some(r) = q.running.as_mut() {
                r.paused = more;
            }
            None
        }
        QueryEvent::Error(_) | QueryEvent::Finished { .. } => {
            // the rows will not come: the file is incomplete
            q.drop_rows = false;
            if let Some(e) = export {
                e.awaiting_rows = false;
                e.abort_reason
                    .get_or_insert_with(|| "The query failed during the export".to_owned());
                let _ = e.writer.send(ToWriter::Abort);
            }
            Some(event)
        }
        other => Some(other),
    }
}

/// Directory listing for the file dialogs: (name, is directory, highlighted); files
/// with the extension `ext` are highlighted.
fn list_dir(dir: &Path, ext: &str) -> Vec<(String, bool, bool)> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                dirs.push(format!("{name}/"));
            } else {
                files.push(name);
            }
        }
    }
    dirs.sort_by_key(|d| d.to_lowercase());
    files.sort_by_key(|f| f.to_lowercase());
    let mut out = Vec::new();
    if dir.parent().is_some() {
        out.push(("../".to_owned(), true, false));
    }
    out.extend(dirs.into_iter().map(|d| (d, true, false)));
    out.extend(files.into_iter().map(|f| {
        let marked = f.to_lowercase().ends_with(&format!(".{ext}"));
        (f, false, marked)
    }));
    out
}

/// Keymap command → editor action (D-038).
fn editor_action(cmd: Command) -> Option<Action> {
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
        _ => return None,
    })
}

/// What follows a save asked for by "Save changes?" (D-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterSave {
    Close(u64),
    /// Continue quitting / disconnecting.
    Bulk,
}

/// Closing many tabs at once (D-043).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bulk {
    Quit,
    /// Disconnect the server with this uid.
    Disconnect(u64),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
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
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
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
                let Ok(e) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
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

    #[test]
    fn psql_window_lists_every_command() {
        let (mut a, _d) = app();
        a.screen = Rect::new(0, 0, 100, 40);
        let d = a.psql_dialog("");
        a.dialogs.push(d);
        let rows = crate::psql::list_rows();
        let screen = screen_text(&a);
        assert!(
            screen.contains("\\dt") && screen.contains("list tables"),
            "{screen}"
        );
        assert!(screen.contains(" pgtc "), "{screen}");
        assert!(screen.contains("\\gtd"), "the whole list fits: {screen}");
        // Down: the next command goes into the input (with a space for its argument)
        key(&mut a, "Down");
        assert_eq!(a.dialogs[0].text("cmd"), Some("\\l "));
        // typing moves the list to the command; the list stays complete
        a.dialogs[0].set_text("cmd", "");
        a.update(Msg::Tick, &Hits::default());
        typing(&mut a, "\\gtu");
        let gtu = rows
            .iter()
            .position(|r| r.is_some_and(|(c, _, _)| c == "\\gtu"))
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
        assert_eq!(a.dialogs[0].text("cmd"), Some("\\gts "));
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
                let Ok(e) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
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
                let Ok(e) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
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
                let Ok(e) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
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
                let Ok(e) =
                    tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv()).await
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
                let Ok(m) =
                    tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv()).await
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
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
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

    /// Writes HTML previews of two real screens per built-in colour scheme to the file
    /// named by `PGTC_PREVIEW_OUT` (Windows Terminal "Campbell" palette). Not part of
    /// the normal test run.
    #[tokio::test]
    #[ignore]
    async fn scheme_preview_html() {
        let Ok(out) = std::env::var("PGTC_PREVIEW_OUT") else {
            return;
        };
        let palette = &crate::theme_check::PALETTES[0];
        let mut html = String::new();
        for name in ThemeSet::built_in().names() {
            let mut screens = Vec::new();
            for screen in 0..2 {
                let (mut a, _d) = app();
                a.settings.ui.color_scheme = name.clone();
                a.theme = a.themes.get(&name).clone();
                // tree: a connected server with databases, a second one disconnected
                let uid = add_server(&mut a, "production");
                add_server(&mut a, "staging");
                fake_connecting(&mut a, uid);
                a.server_connected(uid, pgc_core::ServerVersion(170002));
                let dbs = a.tree.servers[0].children[0].nid;
                let row = |oid, name: &str| pgc_db::NodeRow {
                    oid,
                    name: name.into(),
                    label: None,
                    partitioned: false,
                };
                a.tree
                    .set_children(dbs, vec![row(1, "billing"), row(2, "postgres")]);
                a.tree.node_mut(dbs).unwrap().loading = false;
                // query tab with SQL and a paused result
                a.new_query_tab(None);
                typing(&mut a, "-- revenue per customer, October");
                for line in [
                    "SELECT c.name, sum(o.total) AS revenue, max(o.note) AS note",
                    "FROM orders o JOIN customers c ON c.id = o.customer_id",
                    "WHERE o.created_at >= '2026-10-01' AND o.status <> 'void'",
                    "GROUP BY c.name ORDER BY 2 DESC LIMIT 50;",
                ] {
                    key(&mut a, "Enter");
                    typing(&mut a, line);
                }
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
                let col = |n: &str, t| pgc_db::Column {
                    name: n.into(),
                    type_oid: t,
                };
                q.apply(
                    pgc_db::QueryEvent::ResultStart {
                        columns: vec![col("name", 25), col("revenue", 1700), col("note", 25)],
                        title: None,
                    },
                    &lines,
                );
                let r = |n: &str, v: &str, note: Option<&str>| {
                    vec![Some(n.into()), Some(v.into()), note.map(Into::into)]
                };
                q.apply(
                    pgc_db::QueryEvent::Rows {
                        rows: vec![
                            r(
                                "Acme d.o.o.",
                                "48210.50",
                                Some(
                                    "priority
client",
                                ),
                            ),
                            r("Northwind", "31877.00", None),
                            r("Čakovec Tools", "12904.75", Some("new")),
                            r("Globex", "9120.00", None),
                        ],
                        more: true,
                    },
                    &lines,
                );
                if screen == 0 {
                    a.focus = Focus::Results;
                    if let Some(rs) = a.active_result_set() {
                        rs.move_cursor(1, 1, 0);
                    }
                    key(&mut a, "F9"); // File menu
                    key(&mut a, "Down");
                    key(&mut a, "Down");
                } else {
                    a.focus = Focus::Tree;
                    a.tree.cursor = 2;
                    a.run(Command::Settings);
                    key(&mut a, "Alt+S");
                    key(&mut a, "Space"); // open the colour scheme list
                }
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
                let mut hits = Hits::default();
                terminal
                    .draw(|f| crate::view::render(f, &a, &mut hits))
                    .unwrap();
                let buf = terminal.backend().buffer();
                let mut pre = String::new();
                for y in 0..30 {
                    let mut run_style = String::new();
                    let mut run = String::new();
                    for x in 0..110 {
                        let cell = &buf[(x, y)];
                        let (fg, bg) = crate::theme_check::shown(cell.style(), palette);
                        let m = cell.modifier;
                        let css = format!(
                            "color:#{:02x}{:02x}{:02x};background:#{:02x}{:02x}{:02x}{}{}{}",
                            fg.0,
                            fg.1,
                            fg.2,
                            bg.0,
                            bg.1,
                            bg.2,
                            if m.contains(ratatui::style::Modifier::BOLD) {
                                ";font-weight:700"
                            } else {
                                ""
                            },
                            if m.contains(ratatui::style::Modifier::ITALIC) {
                                ";font-style:italic"
                            } else {
                                ""
                            },
                            if m.contains(ratatui::style::Modifier::UNDERLINED) {
                                ";text-decoration:underline"
                            } else {
                                ""
                            },
                        );
                        if css != run_style && !run.is_empty() {
                            pre.push_str(&format!("<span style=\"{run_style}\">{run}</span>"));
                            run.clear();
                        }
                        run_style = css;
                        let sym = cell.symbol();
                        let sym = if sym.is_empty() { " " } else { sym };
                        run.push_str(
                            &sym.replace('&', "&amp;")
                                .replace('<', "&lt;")
                                .replace('>', "&gt;"),
                        );
                    }
                    pre.push_str(&format!(
                        "<span style=\"{run_style}\">{run}</span>
"
                    ));
                }
                screens.push(pre);
            }
            html.push_str(&format!(
                "<!--scheme {name}-->
{}
<!--screen-->
{}
<!--end-->
",
                screens[0], screens[1]
            ));
        }
        std::fs::write(out, html).unwrap();
    }

    /// Writes the README screenshots (SVG, Dracula scheme) into the folder named by
    /// `PGTC_SCREENSHOT_DIR`: real screens drawn by pgtc's renderer with made-up data.
    /// Not part of the normal test run.
    #[tokio::test]
    #[ignore]
    async fn readme_screenshots() {
        let Ok(out) = std::env::var("PGTC_SCREENSHOT_DIR") else {
            return;
        };
        const W: u16 = 120;
        const H: u16 = 34;
        // a connected server with databases, a query tab with SQL and a result
        let scene = || {
            let (mut a, d) = app();
            a.settings.ui.color_scheme = "Dracula".into();
            a.theme = a.themes.get("Dracula").clone();
            a.screen = Rect::new(0, 0, W, H);
            let uid = a.tree.add(SavedConnection {
                name: "production".into(),
                username: "app".into(),
                ..SavedConnection::default()
            });
            add_server(&mut a, "staging");
            fake_connecting(&mut a, uid);
            a.server_connected(uid, pgc_core::ServerVersion(170002));
            let dbs = a.tree.servers[0].children[0].nid;
            let row = |oid, name: &str| pgc_db::NodeRow {
                oid,
                name: name.into(),
                label: None,
                partitioned: false,
            };
            a.tree.set_children(
                dbs,
                vec![row(1, "billing"), row(2, "postgres"), row(3, "shop")],
            );
            a.tree.node_mut(dbs).unwrap().loading = false;
            a.new_query_tab(None);
            // a query tab connected to "shop" (no real connection behind it)
            let q = a.query_mut().unwrap();
            q.binding = Some((uid, "shop".into()));
            q.conn = Some(crate::query_tab::QueryConn {
                id: 99,
                database: "shop".into(),
                handle: pgc_db::ConnectionHandle::detached(99),
                cancel: None,
                state: crate::query_tab::QueryConnState::Connected,
                ssl_mode: pgc_core::settings::SslMode::default(),
                ssl_root_cert: None,
            });
            typing(&mut a, "-- revenue per customer, October");
            for line in [
                "SELECT c.name, sum(o.total) AS revenue, count(*) AS orders",
                "FROM orders o",
                "JOIN customers c ON c.id = o.customer_id",
                "WHERE o.created_at >= '2026-10-01' AND o.status <> 'void'",
                "GROUP BY c.name",
                "ORDER BY revenue DESC;",
            ] {
                key(&mut a, "Enter");
                typing(&mut a, line);
            }
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
            let col = |n: &str, t| pgc_db::Column {
                name: n.into(),
                type_oid: t,
            };
            q.apply(
                pgc_db::QueryEvent::ResultStart {
                    columns: vec![col("name", 25), col("revenue", 1700), col("orders", 20)],
                    title: None,
                },
                &lines,
            );
            let r =
                |n: &str, v: &str, c: &str| vec![Some(n.into()), Some(v.into()), Some(c.into())];
            q.apply(
                pgc_db::QueryEvent::Rows {
                    rows: vec![
                        r("Acme d.o.o.", "48210.50", "112"),
                        r("Northwind Traders", "31877.00", "87"),
                        r("Čakovec Tools", "12904.75", "41"),
                        r("Globex", "9120.00", "30"),
                        r("Initech", "4410.20", "12"),
                    ],
                    more: false,
                },
                &lines,
            );
            q.apply(
                pgc_db::QueryEvent::Command {
                    tag: "SELECT 5".into(),
                },
                &lines,
            );
            q.apply(
                pgc_db::QueryEvent::Finished {
                    elapsed: std::time::Duration::from_millis(38),
                    status: TxStatus::Idle,
                },
                &lines,
            );
            a.tree.cursor = 2;
            a.focus = Focus::Tabs;
            (a, d)
        };
        let svg = |a: &App| -> String {
            let palette = &crate::theme_check::PALETTES[0];
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
            let mut hits = Hits::default();
            terminal
                .draw(|f| crate::view::render(f, a, &mut hits))
                .unwrap();
            let buf = terminal.backend().buffer();
            let (cw, ch) = (9.0_f32, 19.0_f32);
            let hex = |c: (u8, u8, u8)| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
            let mut out = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" \
                 viewBox=\"0 0 {} {}\" font-family=\"Cascadia Mono, Consolas, 'DejaVu Sans Mono', Menlo, monospace\" \
                 font-size=\"15\">\n",
                W as f32 * cw,
                H as f32 * ch,
                W as f32 * cw,
                H as f32 * ch
            );
            for y in 0..H {
                // backgrounds as runs of one colour
                let mut x = 0;
                while x < W {
                    let (_, bg) = crate::theme_check::shown(buf[(x, y)].style(), palette);
                    let start = x;
                    while x < W && crate::theme_check::shown(buf[(x, y)].style(), palette).1 == bg {
                        x += 1;
                    }
                    out.push_str(&format!(
                        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
                        start as f32 * cw,
                        y as f32 * ch,
                        (x - start) as f32 * cw + 0.5,
                        // overlap the next row: no hairline gaps when scaled
                        ch + 0.5,
                        hex(bg)
                    ));
                }
                // every character at its own column, so the grid stays exact
                for x in 0..W {
                    let cell = &buf[(x, y)];
                    let sym = cell.symbol();
                    if sym.trim().is_empty() {
                        continue;
                    }
                    let (fg, _) = crate::theme_check::shown(cell.style(), palette);
                    let m = cell.modifier;
                    let bold = if m.contains(ratatui::style::Modifier::BOLD) {
                        " font-weight=\"700\""
                    } else {
                        ""
                    };
                    let under = if m.contains(ratatui::style::Modifier::UNDERLINED) {
                        " text-decoration=\"underline\""
                    } else {
                        ""
                    };
                    let sym = sym
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;");
                    out.push_str(&format!(
                        "<text x=\"{}\" y=\"{}\" fill=\"{}\"{bold}{under}>{sym}</text>\n",
                        x as f32 * cw,
                        y as f32 * ch + ch * 0.75,
                        hex(fg)
                    ));
                }
            }
            out.push_str("</svg>\n");
            out
        };
        let dir = std::path::Path::new(&out);
        std::fs::create_dir_all(dir).unwrap();
        // 1. query tab: editor and result grid
        let (a, _d) = scene();
        std::fs::write(dir.join("query.svg"), svg(&a)).unwrap();
        // 2. autocomplete after "c."
        let (mut a, _d) = scene();
        a.editor().set_text("SELECT c. FROM customers c");
        a.editor().set_cursor(9, false);
        a.query_mut().unwrap().names.columns.insert(
            (None, "customers".into()),
            vec![
                "id".into(),
                "name".into(),
                "email".into(),
                "country".into(),
                "created_at".into(),
            ],
        );
        a.query_mut().unwrap().names.objects = Some(Vec::new());
        let analysis = pgc_editor::complete::analyze(&a.tabs[a.active_tab].view.editor).unwrap();
        let built = crate::completion::build(
            &analysis,
            &a.query().unwrap().names,
            pgc_editor::lexer::keywords(),
        );
        let mut popup = crate::completion::Popup {
            tab: a.tabs[a.active_tab].id,
            analysis,
            items: Vec::new(),
            loading: false,
            selected: 0,
            offset: 0,
            auto: false,
        };
        popup.set_items(built.items, false);
        a.completion = Some(popup);
        std::fs::write(dir.join("autocomplete.svg"), svg(&a)).unwrap();
        // 3. the psql command window (F4)
        let (mut a, _d) = scene();
        let d = a.psql_dialog("");
        a.dialogs.push(d);
        key(&mut a, "Down");
        key(&mut a, "Down");
        key(&mut a, "Down");
        std::fs::write(dir.join("psql.svg"), svg(&a)).unwrap();
        // 4. the Query menu
        let (mut a, _d) = scene();
        key(&mut a, "F9");
        key(&mut a, "Right");
        key(&mut a, "Right");
        std::fs::write(dir.join("menu.svg"), svg(&a)).unwrap();
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
    fn context_menu_runs_commands() {
        let (mut a, _d) = app();
        add_server(&mut a, "a");
        key(&mut a, "F2");
        assert!(a.popup.is_some());
        key(&mut a, "e"); // Edit connection…
        assert!(a.popup.is_none());
        assert_eq!(a.dialogs[0].kind, DialogKind::Connection);
    }
}
