//! Central application state and the `update` function (Elm style). Input events,
//! database events and timers arrive as [`Msg`]s; `update` is the only place that
//! mutates state.
//!
//! `impl App` is spread over the submodules by area (tree pane, query tabs, closing,
//! ...); this file has the state, the message loop and the command dispatch.

mod autocomplete;
mod closing;
mod command_line;
mod connections;
mod dashboard;
mod definition;
mod editing;
mod files;
mod find_object;
mod focus;
mod lookups;
mod mouse;
mod object_view;
mod profiler;
mod query;
mod result_export;
mod results;
mod settings_dialog;
mod statement_pane;
mod templates;
#[cfg(test)]
mod tests;
mod top_rows;
mod tree_pane;

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pgc_core::paths::Paths;
use pgc_core::settings::Settings;
use pgc_db::conninfo::Resolved;
use pgc_db::{ConnId, DbEvent, EventSink};
use pgc_editor::{Action, Editor, SearchOptions};
use pgc_sql::SqlCatalog;
use ratatui::layout::Rect;

use self::closing::{AfterSave, Bulk};
use self::definition::{JobKind, StructureJob};
use self::editing::editor_action;
use self::lookups::LookupTarget;
use self::templates::Picker;
use crate::clipboard::Clipboard;
use crate::dialog::{Dialog, DialogEvent, DialogKind};
use crate::export::{ExportEvent, ExportOptions, ExportSink};
use crate::keymap::{Command, Context, Keymap};
use crate::keys::KeyCombo;
use crate::menu::{self, MenuDef, MenuEvent, MenuState, Popup};
use crate::query_tab::{AfterRun, QueryState};
use crate::template_dialog::{Choice, TemplateForm};
use crate::theme::{Theme, ThemeSet};
use crate::tree::{ConnState, RowKind, Tree};
use crate::view::Hits;

/// Messages processed by [`App::update`].
#[derive(Debug)]
pub enum Msg {
    Input(Event),
    /// The `Esc` + digit window with this sequence number expired (D-032).
    EscTimeout(u64),
    /// The tree cursor rested long enough: load the default tab (D-037).
    DefinitionTimer(u64),
    /// Typing in Find object paused: search (D-055).
    FindTimer(u64),
    /// Time for another slice of the editors' background lexer scan (D-038).
    ScanTick,
    /// Periodic tick while a query runs (elapsed time, cancel wait, D-041).
    Tick,
    Db(DbEvent),
    /// Time for the next refresh of tab `.0` (dashboard, profiler) if `.1` is its
    /// latest timer (D-057, D-058).
    TabTimer(u64, u64),
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
    /// Send `Msg::FindTimer(seq)` after the same delay (D-055).
    StartFindTimer(u64),
    /// Send `Msg::ScanTick` soon.
    ScanTick,
    /// Send `Msg::Tick` after the tick interval.
    Tick,
    /// Put this text into the terminal's clipboard with OSC 52 (D-023).
    Osc52(String),
    /// Send `Msg::TabTimer(tab, seq)` after `after` (D-057, D-058).
    StartTabTimer {
        tab: u64,
        seq: u64,
        after: std::time::Duration,
    },
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
    /// A server dashboard (D-057).
    Dashboard,
    /// An SQL profiler (D-058).
    Profiler,
}

/// Content of a tab: an editor (D-038), read-only for object views (D-037).
#[derive(Debug)]
pub struct TextView {
    /// Shown as the pane title (object name).
    pub title: String,
    pub editor: Editor,
    pub loading: bool,
    /// The text is an object's definition (not an info text or an error), so it can
    /// be edited in a new query tab (D-049).
    pub definition: bool,
    /// Last text loaded with `set_text` (identical text is not reloaded).
    shown: String,
}

impl TextView {
    pub fn new(read_only: bool, tab_width: usize) -> Self {
        Self {
            title: String::new(),
            editor: Editor::new("", read_only, tab_width),
            loading: false,
            definition: false,
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
        self.definition = false;
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
    /// Dashboard tabs (D-057).
    pub dashboard: Option<crate::dashboard::Dashboard>,
    /// Profiler tabs (D-058).
    pub profiler: Option<crate::profiler::Profiler>,
    /// The lower part (results, statement pane) had the focus last (`Alt+2`, D-060).
    pub lower_focused: bool,
}

impl Tab {
    /// Label in the tab strip.
    pub fn label(&self) -> String {
        match &self.kind {
            TabKind::Default => "Default".to_owned(),
            TabKind::Object => self.view.title.clone(),
            TabKind::Dashboard => self
                .dashboard
                .as_ref()
                .map(|d| d.label.clone())
                .unwrap_or_default(),
            TabKind::Profiler => self
                .profiler
                .as_ref()
                .map(|p| p.label.clone())
                .unwrap_or_default(),
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
    /// Dashboard tab id (D-057).
    Dashboard(u64),
    /// Profiler tab id (D-058).
    Profiler(u64),
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
    /// The open Find object dialog (D-055).
    find: Option<find_object::FindState>,
    find_seq: u64,
    /// The tree opening down to a found object (D-055).
    reveal: Option<find_object::Reveal>,
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
    /// their way (key → what they fill; also the lookups of autocomplete, D-052, and
    /// of the generate commands, D-054) and the row being edited (list field, row).
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
    /// Quitting waits for profilers to put the logging settings back (D-058).
    quit_after_restore: bool,
    esc_seq: u64,
    esc_pending: bool,
}

const ESC: KeyEvent = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
/// Lines lexed per background scan tick (D-038).
const SCAN_BUDGET_LINES: usize = 100_000;

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
                dashboard: None,
                profiler: None,
                lower_focused: false,
            }],
            active_tab: 0,
            status,
            search: None,
            completion: None,
            find: None,
            find_seq: 0,
            reveal: None,
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
            quit_after_restore: false,
            esc_seq: 0,
            esc_pending: false,
        };
        if let Some(resolved) = startup {
            app.startup_connection(resolved);
        }
        app
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
            // dashboard / profiler: the list has the results focus, the statement pane
            // the editor focus (D-059)
            Focus::Results if self.tabs[self.active_tab].dashboard.is_some() => {
                Some(Context::Dashboard)
            }
            Focus::Results if self.tabs[self.active_tab].profiler.is_some() => {
                Some(Context::Profiler)
            }
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
            Command::FocusTree
            | Command::FocusRight
            | Command::FocusLower
            | Command::FocusUpper => true,
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
            Command::FindObject => self.find_scope().is_ok(),
            Command::EditInQueryTab if self.list_tab() && self.focus != Focus::Tree => {
                self.can_edit_statement()
            }
            Command::EditInQueryTab => self.edit_source().is_ok(),
            Command::ToggleResults | Command::ResultsSmaller | Command::ResultsLarger
                if self.list_tab() =>
            {
                true
            }
            Command::SelectTopRows => self.top_rows_source().is_ok(),
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
            Command::NextResultTab | Command::PrevResultTab => self.query().is_some(),
            Command::Psql => self
                .query()
                .is_some_and(|q| q.connected() && !q.busy() && q.export.is_none()),
            Command::Dashboard => self.dashboard_target().is_some(),
            Command::DashboardRefresh
            | Command::DashboardPause
            | Command::DashboardInterval
            | Command::HideIdleSessions
            | Command::NextList
            | Command::PrevList
            | Command::CancelBackend
            | Command::TerminateBackend
            | Command::ShowRow
                if self.dashboard().is_some() =>
            {
                self.dashboard_enabled(cmd)
            }
            Command::SqlProfiler => self.dashboard_target().is_some(),
            Command::ProfilerStartStop
            | Command::ProfilerPause
            | Command::ProfilerFilter
            | Command::ProfilerClear
            | Command::ShowRow => self.profiler_enabled(cmd),
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
                    Context::Dashboard,
                    Context::Profiler,
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
            Msg::FindTimer(seq) => {
                if seq == self.find_seq {
                    self.find_search();
                }
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
                // a waiting search, the tree opening to a found object (D-055)
                self.after_tree_event();
                Vec::new()
            }
            Msg::Export(event) => {
                self.on_export_event(event);
                Vec::new()
            }
            Msg::TabTimer(tab, seq) => {
                self.tab_timer(tab, seq);
                Vec::new()
            }
        };
        self.sync_export_dialog();
        self.sync_picker();
        self.sync_psql_dialog();
        self.sync_find_dialog();
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
        if self.focus == Focus::Results
            && !self.list_tab()
            && self.query().is_none_or(|q| !q.results_visible)
        {
            self.focus = Focus::Tabs;
        }
        // the statement pane follows the list cursor (D-059)
        self.sync_pane();
        self.fix_list_focus();
        self.remember_part();
        // the list belongs to the editor of the active query tab, or to the focused
        // `sql` box of that tab's template form (D-052)
        let keep = match self.completion.as_ref() {
            None => true,
            Some(p) if p.in_form => {
                self.completion_in_form() && self.template.as_ref().is_some_and(|f| f.tab == p.tab)
            }
            Some(p) => {
                p.tab == self.tabs[self.active_tab].id && self.focus == Focus::Tabs && !self.modal()
            }
        };
        if !keep {
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
        // what the terminal sent and what it became (PGTC_LOG=pgc_ui=trace)
        tracing::trace!(
            code = ?ev.code,
            modifiers = ?ev.modifiers,
            kind = ?ev.kind,
            key = %combo,
            focus = ?self.focus,
            context = ?self.context(),
            ?cmd,
            "key"
        );

        if let Some(dialog) = self.dialogs.last_mut() {
            if cmd == Some(Command::Help) {
                self.run(Command::Help);
                return;
            }
            // F4 list: arrows and pages move in the list while typing in the input
            // Find object: arrows and pages move in the list while typing (D-055)
            if dialog.kind == DialogKind::FindObject {
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
                    return;
                }
            }
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

        if self.template.is_some() {
            // an SQL box gets the editor's keys (D-046) and autocomplete (D-052)
            let in_sql = self.completion_in_form();
            let editor_cmd = if in_sql {
                self.keymap.lookup_only(Context::Editor, combo)
            } else {
                None
            };
            if in_sql {
                if self.completion.is_some() && self.completion_key(&ev, editor_cmd) {
                    return;
                }
                if editor_cmd == Some(Command::Complete) {
                    self.open_completion(true);
                    return;
                }
            }
            let typed = match ev.code {
                KeyCode::Char(c)
                    if in_sql
                        && !ev
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    Some(c)
                }
                _ => None,
            };
            let Some(form) = self.template.as_mut() else {
                return;
            };
            let event = form.handle_key(cmd, editor_cmd.and_then(editor_action), &ev);
            self.form_event(event);
            if let Some(c) = typed
                && self.completion_in_form()
            {
                self.after_typing(c);
            }
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
        let in_dashboard = in_results && self.dashboard().is_some();
        let in_profiler = in_results && self.profiler().is_some();
        match cmd {
            Command::Quit => self.request_quit(),
            Command::Menu => self.open_menu(0),
            // direct keys for the frames (D-060)
            Command::FocusTree => self.focus = Focus::Tree,
            Command::FocusRight => self.focus_right(),
            Command::FocusLower => self.focus_lower(),
            Command::FocusUpper => self.focus_upper(),
            // F6 / Ctrl+Tab: only the tree ↔ the right frame (its part last used, D-060)
            Command::SwitchPanel if self.focus == Focus::Tree => self.focus_right(),
            Command::SwitchPanel => self.focus = Focus::Tree,
            c if in_results && self.results_command(c) => {}
            c if in_dashboard && self.dashboard_command(c) => {}
            c if in_profiler && self.profiler_command(c) => {}
            Command::Dashboard => self.open_dashboard(),
            Command::SqlProfiler => self.open_profiler(),
            // the statement pane of a dashboard / profiler tab (D-059)
            Command::ToggleResults if self.list_tab() => self.toggle_pane(),
            Command::ResultsSmaller if self.list_tab() => self.resize_pane(-5),
            Command::ResultsLarger if self.list_tab() => self.resize_pane(5),
            Command::EditInQueryTab if self.list_tab() && !in_tree => {
                self.edit_statement_in_query_tab();
            }
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
            // Alt+Left / Right in the results: result sets, then Messages (D-060)
            Command::NextResultTab | Command::PrevResultTab => {
                let next = cmd == Command::NextResultTab;
                self.toggle_query(|q| {
                    q.results_visible = true;
                    // positions: the result sets 0..k, then Messages (k); without a
                    // result set the (empty) Results view is one position
                    let n = q.results.len();
                    let k = n.max(1);
                    let at = if q.show_messages {
                        k
                    } else {
                        q.active_result.min(k - 1)
                    };
                    let to = if next {
                        (at + 1) % (k + 1)
                    } else {
                        (at + k) % (k + 1)
                    };
                    q.show_messages = to == k;
                    if to < n {
                        q.active_result = to;
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
            Command::InstallTemplates => self.start_setup(false),
            Command::RunSetup => self.start_setup(true),
            Command::ShowTemplatesFolder => self.show_templates_folder(),
            Command::Help => self.open_shortcuts(true),
            Command::KeyboardShortcuts => self.open_shortcuts(false),
            Command::ThirdPartyLicences => self.open_licences(),
            Command::About => self.open_about(),
            Command::RefreshTree => self.refresh(),
            Command::ShowInNewTab => self.show_in_new_tab(),
            Command::EditInQueryTab => self.edit_in_query_tab(),
            Command::SelectTopRows => self.select_top_rows(),
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
            Command::FindObject if in_tree => self.open_find_object(),
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

    // ---- tabs --------------------------------------------------------------------

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
            dashboard: None,
            profiler: None,
            lower_focused: false,
        });
        self.active_tab = self.tabs.len() - 1;
        self.focus = Focus::Tabs;
        id
    }

    fn tab_mut(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

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

    // ---- dialogs ------------------------------------------------------------------

    fn error(&mut self, text: &str) {
        self.dialogs.push(Dialog::message("Error", text, true));
    }

    fn dialog_event(&mut self, event: DialogEvent) {
        match event {
            DialogEvent::None => {}
            DialogEvent::Cancelled => {
                if let Some(d) = self.dialogs.pop()
                    && d.kind == DialogKind::ConfirmProfilerSettings
                {
                    self.profiler_answer(d.tag, false);
                }
            }
            DialogEvent::Button(id) => {
                let Some(dialog) = self.dialogs.pop() else {
                    return;
                };
                match (dialog.kind, id) {
                    (DialogKind::Settings, "ok") => self.apply_settings(dialog),
                    (DialogKind::FindObject, "ok") => self.find_chosen(dialog),
                    (DialogKind::PsqlCommand, "ok") => {
                        let text = dialog.text("cmd").unwrap_or("").to_owned();
                        self.run_psql(dialog, &text);
                    }
                    (DialogKind::Connection, "ok") => self.apply_connection(dialog),
                    (DialogKind::Login, "ok") => self.apply_login(&dialog),
                    // as disconnecting: questions first, then the server's tabs close
                    (DialogKind::ConfirmDelete, "yes") => {
                        self.request_bulk(Bulk::Delete(dialog.tag));
                    }
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
                    (DialogKind::ConfirmCancelBackend, "yes") => {
                        self.send_signal(dialog.tag, crate::dashboard::Signal::Cancel);
                    }
                    (DialogKind::ConfirmProfilerSettings, button) => {
                        self.profiler_answer(dialog.tag, button == "yes");
                    }
                    (DialogKind::ProfilerFilter, "ok") => self.apply_profiler_filter(&dialog),
                    (DialogKind::ConfirmTerminateBackend, "yes") => {
                        self.send_signal(dialog.tag, crate::dashboard::Signal::Terminate);
                    }
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
                    // No: the other setup steps still run (templates are kept)
                    (DialogKind::ConfirmTemplatesOverwrite, b) => {
                        self.finish_setup(dialog.tag == 1, b == "yes");
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
}

/// A request on the query tab's connection would end its result (D-019, D-046, D-054).
const CONNECTION_BUSY: &str =
    "The query tab's connection is busy.\nWait for the query (or close its result) first.";

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}
