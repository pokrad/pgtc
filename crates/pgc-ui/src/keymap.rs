//! Keymap with contexts (D-016, D-032): built-in mc-style defaults, overridden by
//! `keymap.toml` in the user's config directory.
//!
//! ```toml
//! [tree]
//! refresh_tree = "Ctrl+R"            # one key
//! quick_search = ["Alt+S", "Ctrl+S"] # several keys
//! drop_object  = []                  # unbound
//! ```
//!
//! Lookup: the current context first, then `[global]`.

use std::collections::HashMap;

use crate::keys::KeyCombo;

/// Keymap sections (D-016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Global,
    Tree,
    Editor,
    Results,
    /// The server dashboard tab (D-057).
    Dashboard,
    /// The SQL profiler tab (D-058).
    Profiler,
    /// The database statistics tab (D-062).
    DbStats,
    Dialog,
}

impl Context {
    pub const ALL: [Context; 8] = [
        Context::Global,
        Context::Tree,
        Context::Editor,
        Context::Results,
        Context::Dashboard,
        Context::Profiler,
        Context::DbStats,
        Context::Dialog,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Context::Global => "global",
            Context::Tree => "tree",
            Context::Editor => "editor",
            Context::Results => "results",
            Context::Dashboard => "dashboard",
            Context::Profiler => "profiler",
            Context::DbStats => "db_stats",
            Context::Dialog => "dialog",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// Groups of the key lists, in their order (D-065).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    General,
    Actions,
    File,
    Moving,
    Selecting,
    Editing,
    Clipboard,
    Find,
    Results,
    Windows,
}

impl Group {
    /// Subheading in the key lists.
    pub fn title(self) -> &'static str {
        match self {
            Group::General => "General",
            Group::Actions => "Actions",
            Group::File => "File",
            Group::Moving => "Moving",
            Group::Selecting => "Selecting",
            Group::Editing => "Editing",
            Group::Clipboard => "Clipboard and undo",
            Group::Find => "Find",
            Group::Results => "Results",
            Group::Windows => "Windows and tabs",
        }
    }
}

macro_rules! commands {
    ($($variant:ident = $name:literal, $desc:literal, $group:ident;)*) => {
        /// Everything the user can trigger by key or menu.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Command { $($variant),* }

        impl Command {
            pub const ALL: &'static [Command] = &[$(Command::$variant),*];

            /// Name used in `keymap.toml`.
            pub fn name(self) -> &'static str {
                match self { $(Command::$variant => $name),* }
            }

            /// Short description (Help → Keyboard shortcuts, `F1`).
            pub fn description(self) -> &'static str {
                match self { $(Command::$variant => $desc),* }
            }

            /// Group in the key lists (D-065).
            pub fn group(self) -> Group {
                match self { $(Command::$variant => Group::$group),* }
            }
        }
    };
}

commands! {
    // global
    Help = "help", "The keys that work now", General;
    Menu = "menu", "Open the menu bar", General;
    Quit = "quit", "Quit pgtc", General;
    SwitchPanel = "switch_panel", "Switch between the object tree and the right frame", Windows;
    NextTab = "next_tab", "Next tab", Windows;
    PrevTab = "prev_tab", "Previous tab", Windows;
    CloseTab = "close_tab", "Close the tab", Windows;
    // direct keys for the frames (D-060)
    FocusTree = "focus_tree", "Focus the object tree", Windows;
    FocusRight = "focus_right", "Focus the right frame (the part last used)", Windows;
    FocusLower = "focus_lower", "Lower part of the right frame (results, messages, statement pane)", Windows;
    FocusUpper = "focus_upper", "Upper part of the right frame (editor, list)", Windows;
    // tree
    ContextMenu = "context_menu", "Context menu", Actions;
    ShowInNewTab = "show_in_new_tab", "Show the object in a new tab", Actions;
    DropObject = "drop_object", "Delete the saved connection", Actions;
    RefreshTree = "refresh_tree", "Refresh the object tree", Actions;
    ToggleSystemObjects = "toggle_system_objects", "Show / hide system objects", Actions;
    QuickSearch = "quick_search", "Quick search", Find;
    // query tab
    Execute = "execute", "Execute the selection or the whole script", Actions;
    CancelQuery = "cancel_query", "Cancel the running query", Actions;
    NewFromTemplate = "new_from_template", "Generate SQL from developer template", Actions;
    ToggleResults = "toggle_results", "Show / hide the results (statement pane in dashboard / profiler)", Results;
    // navigation
    Up = "up", "Up", Moving;
    Down = "down", "Down", Moving;
    Left = "left", "Left / collapse", Moving;
    Right = "right", "Right / expand", Moving;
    PageUp = "page_up", "Page up", Moving;
    PageDown = "page_down", "Page down", Moving;
    Home = "home", "Start of line / first row", Moving;
    End = "end", "End of line / last row", Moving;
    DocStart = "doc_start", "Start of the text", Moving;
    DocEnd = "doc_end", "End of the text", Moving;
    // editor (D-015, D-038)
    WordLeft = "word_left", "Word left", Moving;
    WordRight = "word_right", "Word right", Moving;
    SelectLeft = "select_left", "Select left", Selecting;
    SelectRight = "select_right", "Select right", Selecting;
    SelectUp = "select_up", "Select up", Selecting;
    SelectDown = "select_down", "Select down", Selecting;
    SelectWordLeft = "select_word_left", "Select word left", Selecting;
    SelectWordRight = "select_word_right", "Select word right", Selecting;
    SelectHome = "select_home", "Select to start of line", Selecting;
    SelectEnd = "select_end", "Select to end of line", Selecting;
    SelectPageUp = "select_page_up", "Select page up", Selecting;
    SelectPageDown = "select_page_down", "Select page down", Selecting;
    SelectDocStart = "select_doc_start", "Select to start of the text", Selecting;
    SelectDocEnd = "select_doc_end", "Select to end of the text", Selecting;
    SelectAll = "select_all", "Select all", Selecting;
    Copy = "copy", "Copy", Clipboard;
    Cut = "cut", "Cut", Clipboard;
    Paste = "paste", "Paste", Clipboard;
    Undo = "undo", "Undo", Clipboard;
    Redo = "redo", "Redo", Clipboard;
    Indent = "indent", "Indent", Editing;
    Unindent = "unindent", "Unindent", Editing;
    Newline = "newline", "New line", Editing;
    DeleteWordLeft = "delete_word_left", "Delete word left", Editing;
    DeleteWordRight = "delete_word_right", "Delete word right", Editing;
    DeleteLine = "delete_line", "Delete the line (the selected lines)", Editing;
    Find = "find", "Find / replace", Find;
    Complete = "complete", "Autocomplete: names and keywords", Actions;
    Psql = "psql_command", "psql-style command line (dt, d, gti, ...)", Actions;
    FindNext = "find_next", "Find next", Find;
    FindPrev = "find_prev", "Find previous", Find;
    // query tab (D-019, D-020, D-041)
    FetchAll = "fetch_all", "Fetch all rows", Results;
    ExportResult = "export_result", "Export the result", Results;
    Commit = "commit", "Commit", Actions;
    Rollback = "rollback", "Rollback", Actions;
    ToggleAutoCommit = "toggle_auto_commit", "Auto commit on / off", Actions;
    ToggleAutoRollback = "toggle_auto_rollback", "Auto rollback on error on / off", Actions;
    TogglePlpgsql = "toggle_plpgsql", "PL/pgSQL mode on / off", Actions;
    EditInQueryTab = "edit_in_query_tab", "Edit the shown definition (or statement) in a new query tab", Actions;
    FindObject = "find_object", "Find an object below the selected tree row", Find;
    SelectTopRows = "select_top_rows", "Select the top 100 rows of the table or view in a new query tab", Actions;
    ShowMessages = "show_messages", "Switch between results and messages", Results;
    NextResult = "next_result", "Next result set", Results;
    PrevResult = "prev_result", "Previous result set", Results;
    NextResultTab = "next_result_tab", "Next result set, then Messages", Results;
    PrevResultTab = "prev_result_tab", "Previous result set or Messages", Results;
    ResultsSmaller = "results_smaller", "Make the results (statement pane) smaller", Results;
    ResultsLarger = "results_larger", "Make the results (statement pane) larger", Results;
    // server dashboard (D-057)
    Dashboard = "dashboard", "Server dashboard of the selected server or database", Actions;
    DashboardRefresh = "dashboard_refresh", "Refresh the dashboard now (connects again when needed)", Actions;
    DashboardPause = "dashboard_pause", "Pause / resume the dashboard refresh", Actions;
    DashboardInterval = "dashboard_interval", "Dashboard refresh interval (1, 2, 5, 10, 30 s)", Actions;
    HideIdleSessions = "hide_idle_sessions", "Hide / show idle sessions", Actions;
    NextList = "next_list", "Next dashboard list", Moving;
    PrevList = "prev_list", "Previous dashboard list", Moving;
    CancelBackend = "cancel_backend", "Cancel the query of the selected session", Actions;
    TerminateBackend = "terminate_backend", "Terminate the selected session", Actions;
    ShowRow = "show_row", "Show the whole row (full query text)", Actions;
    // SQL profiler (D-058)
    SqlProfiler = "sql_profiler", "SQL profiler: every statement on the server", Actions;
    ProfilerStartStop = "profiler_start_stop", "Start / stop the profiler", Actions;
    ProfilerPause = "profiler_pause", "Pause / resume reading the server log", Actions;
    ProfilerFilter = "profiler_filter", "Filter the profiler entries", Actions;
    ProfilerClear = "profiler_clear", "Clear the profiler entries", Actions;
    // database statistics (D-062)
    DbStats = "db_stats", "Statistics of the selected database", Actions;
    DbStatsRefresh = "db_stats_refresh", "Read the database statistics again (connects again when needed)", Actions;
    // dialogs and menus
    NextField = "next_field", "Next field", Moving;
    PrevField = "prev_field", "Previous field", Moving;
    Accept = "accept", "Open / choose / OK", Actions;
    DialogOk = "dialog_ok", "OK from any field (where the terminal reports Ctrl+Enter)", Actions;
    Cancel = "cancel", "Cancel / close", Actions;
    Toggle = "toggle", "Toggle the checkbox", Actions;
    Backspace = "backspace", "Delete left", Editing;
    Delete = "delete", "Delete right", Editing;
    // menu commands (no default key)
    NewConnection = "new_connection", "New connection", General;
    EditConnection = "edit_connection", "Edit the connection", General;
    DeleteConnection = "delete_connection", "Delete the connection", General;
    Connect = "connect", "Connect", General;
    Disconnect = "disconnect", "Disconnect", General;
    NewQuery = "new_query", "New query tab", File;
    OpenFile = "open_file", "Open SQL file", File;
    SaveFile = "save_file", "Save SQL file", File;
    SaveFileAs = "save_file_as", "Save SQL file as", File;
    ExportDefinition = "export_definition", "Export database definition", File;
    LoadDefinition = "load_definition", "Load database definition", File;
    Settings = "settings", "Settings", General;
    RunSetup = "run_setup", "Run setup", General;
    InstallTemplates = "install_templates", "Install templates", General;
    ShowTemplatesFolder = "show_templates_folder", "Show templates folder", General;
    KeyboardShortcuts = "keyboard_shortcuts", "All keyboard shortcuts", General;
    ThirdPartyLicences = "third_party_licences", "Third-party licences", General;
    About = "about", "About pgtc", General;
}

impl Command {
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.name() == name)
    }
}

/// Built-in defaults (mc style, D-016, D-032).
const DEFAULTS: &[(Context, Command, &[&str])] = &[
    (Context::Global, Command::Help, &["F1"]),
    (Context::Global, Command::Menu, &["F9"]),
    (Context::Global, Command::NewQuery, &["Ctrl+N"]),
    (Context::Global, Command::Quit, &["F10"]),
    // Tab is only for the editor (indent) and dialogs (fields), D-016
    (Context::Global, Command::SwitchPanel, &["Ctrl+Tab", "F6"]),
    (Context::Global, Command::NextTab, &["Alt+Right"]),
    (Context::Global, Command::PrevTab, &["Alt+Left"]),
    (Context::Global, Command::CloseTab, &["Ctrl+W"]),
    // D-060: the Ctrl keys only where the terminal reports them
    (Context::Global, Command::FocusTree, &["Alt+1", "Ctrl+1"]),
    (Context::Global, Command::FocusRight, &["Alt+2", "Ctrl+2"]),
    (
        Context::Global,
        Command::FocusLower,
        &["Alt+Down", "Ctrl+Shift+Down"],
    ),
    (
        Context::Global,
        Command::FocusUpper,
        &["Alt+Up", "Ctrl+Shift+Up"],
    ),
    (Context::Tree, Command::Up, &["Up"]),
    (Context::Tree, Command::Down, &["Down"]),
    (Context::Tree, Command::Left, &["Left"]),
    (Context::Tree, Command::Right, &["Right"]),
    (Context::Tree, Command::PageUp, &["PageUp"]),
    (Context::Tree, Command::PageDown, &["PageDown"]),
    (Context::Tree, Command::Home, &["Home"]),
    (Context::Tree, Command::End, &["End"]),
    (Context::Tree, Command::Accept, &["Enter"]),
    (Context::Tree, Command::ContextMenu, &["F2"]),
    (Context::Tree, Command::ShowInNewTab, &["F3"]),
    (Context::Tree, Command::DropObject, &["F8"]),
    (Context::Tree, Command::RefreshTree, &["Ctrl+R"]),
    (Context::Tree, Command::QuickSearch, &["Alt+S", "Ctrl+S"]),
    (Context::Tree, Command::EditInQueryTab, &["Ctrl+E"]),
    (Context::Tree, Command::FindObject, &["Ctrl+F"]),
    (Context::Tree, Command::SelectTopRows, &["Ctrl+B"]),
    (Context::Tree, Command::Dashboard, &["Ctrl+D"]),
    (Context::Tree, Command::SqlProfiler, &["Ctrl+P"]),
    // Ctrl+I is Tab in most terminals, Alt+S quick search (D-062)
    (Context::Tree, Command::DbStats, &["Alt+I"]),
    (Context::Editor, Command::EditInQueryTab, &["Ctrl+E"]),
    (Context::Editor, Command::Execute, &["F5"]),
    (Context::Editor, Command::NewFromTemplate, &["F7"]),
    (Context::Editor, Command::CancelQuery, &["Alt+Shift+C"]),
    (Context::Editor, Command::ToggleResults, &["Ctrl+R"]),
    (Context::Editor, Command::Up, &["Up"]),
    (Context::Editor, Command::Down, &["Down"]),
    (Context::Editor, Command::Left, &["Left"]),
    (Context::Editor, Command::Right, &["Right"]),
    (Context::Editor, Command::PageUp, &["PageUp"]),
    (Context::Editor, Command::PageDown, &["PageDown"]),
    (Context::Editor, Command::Home, &["Home"]),
    (Context::Editor, Command::End, &["End"]),
    (Context::Editor, Command::DocStart, &["Ctrl+Home"]),
    (Context::Editor, Command::DocEnd, &["Ctrl+End"]),
    (Context::Editor, Command::WordLeft, &["Ctrl+Left"]),
    (Context::Editor, Command::WordRight, &["Ctrl+Right"]),
    (Context::Editor, Command::SelectLeft, &["Shift+Left"]),
    (Context::Editor, Command::SelectRight, &["Shift+Right"]),
    (Context::Editor, Command::SelectUp, &["Shift+Up"]),
    (Context::Editor, Command::SelectDown, &["Shift+Down"]),
    (
        Context::Editor,
        Command::SelectWordLeft,
        &["Ctrl+Shift+Left"],
    ),
    (
        Context::Editor,
        Command::SelectWordRight,
        &["Ctrl+Shift+Right"],
    ),
    (Context::Editor, Command::SelectHome, &["Shift+Home"]),
    (Context::Editor, Command::SelectEnd, &["Shift+End"]),
    (Context::Editor, Command::SelectPageUp, &["Shift+PageUp"]),
    (
        Context::Editor,
        Command::SelectPageDown,
        &["Shift+PageDown"],
    ),
    // second keys: Windows Terminal scrolls with Ctrl+Shift+Home / End (D-053)
    (
        Context::Editor,
        Command::SelectDocStart,
        &["Ctrl+Shift+Home", "Alt+Shift+Home"],
    ),
    (
        Context::Editor,
        Command::SelectDocEnd,
        &["Ctrl+Shift+End", "Alt+Shift+End"],
    ),
    (Context::Editor, Command::SelectAll, &["Ctrl+A"]),
    (Context::Editor, Command::Copy, &["Ctrl+C", "Ctrl+Insert"]),
    (Context::Editor, Command::Cut, &["Ctrl+X", "Shift+Delete"]),
    (Context::Editor, Command::Paste, &["Ctrl+V", "Shift+Insert"]),
    (Context::Editor, Command::Undo, &["Ctrl+Z"]),
    (
        Context::Editor,
        Command::Redo,
        &["Ctrl+Shift+Z", "Alt+Shift+Z"],
    ),
    (Context::Editor, Command::DeleteLine, &["Ctrl+Y"]),
    (Context::Editor, Command::Indent, &["Tab"]),
    (Context::Editor, Command::Unindent, &["Shift+Tab"]),
    (Context::Editor, Command::Newline, &["Enter"]),
    (Context::Editor, Command::Backspace, &["Backspace"]),
    (Context::Editor, Command::Delete, &["Delete"]),
    (
        Context::Editor,
        Command::DeleteWordLeft,
        &["Ctrl+Backspace"],
    ),
    (Context::Editor, Command::DeleteWordRight, &["Ctrl+Delete"]),
    (Context::Editor, Command::Find, &["Ctrl+F"]),
    (Context::Editor, Command::Complete, &["Ctrl+Space", "Alt+/"]),
    (Context::Editor, Command::Psql, &["F4"]),
    (Context::Editor, Command::SaveFile, &["Ctrl+S"]),
    (Context::Results, Command::SaveFile, &["Ctrl+S"]),
    (Context::Editor, Command::TogglePlpgsql, &["Ctrl+P"]),
    (Context::Results, Command::TogglePlpgsql, &["Ctrl+P"]),
    (Context::Results, Command::Psql, &["F4"]),
    (Context::Editor, Command::FindNext, &["F3"]),
    // Shift+F3 can be lost on some Linux terminals (D-053)
    (Context::Editor, Command::FindPrev, &["Shift+F3", "Alt+F3"]),
    (Context::Results, Command::CancelQuery, &["Alt+Shift+C"]),
    (Context::Results, Command::ToggleResults, &["Ctrl+R"]),
    (Context::Results, Command::Execute, &["F5"]),
    (Context::Results, Command::Up, &["Up"]),
    (Context::Results, Command::Down, &["Down"]),
    (Context::Results, Command::Left, &["Left"]),
    (Context::Results, Command::Right, &["Right"]),
    (Context::Results, Command::PageUp, &["PageUp"]),
    (Context::Results, Command::PageDown, &["PageDown"]),
    (Context::Results, Command::Home, &["Home"]),
    (Context::Results, Command::End, &["End"]),
    (Context::Results, Command::DocStart, &["Ctrl+Home"]),
    (Context::Results, Command::DocEnd, &["Ctrl+End"]),
    (Context::Results, Command::Copy, &["Ctrl+C", "Ctrl+Insert"]),
    (Context::Results, Command::SelectAll, &["Ctrl+A"]),
    (Context::Results, Command::Accept, &["Enter"]),
    (Context::Results, Command::FetchAll, &["Alt+End"]),
    (Context::Results, Command::ContextMenu, &["F2"]),
    (Context::Results, Command::ShowMessages, &["Alt+M"]),
    // D-060: the tabs nearest to the focus
    (Context::Results, Command::NextResultTab, &["Alt+Right"]),
    (Context::Results, Command::PrevResultTab, &["Alt+Left"]),
    (Context::Dashboard, Command::NextList, &["Alt+Right"]),
    (Context::Dashboard, Command::PrevList, &["Alt+Left"]),
    // GNOME Terminal / xfce switch their tabs with Ctrl+PgUp / PgDn (D-053)
    (
        Context::Results,
        Command::NextResult,
        &["Ctrl+PageDown", "Alt+PageDown"],
    ),
    (
        Context::Results,
        Command::PrevResult,
        &["Ctrl+PageUp", "Alt+PageUp"],
    ),
    (Context::Results, Command::ResultsSmaller, &["Alt+Shift+Up"]),
    (
        Context::Results,
        Command::ResultsLarger,
        &["Alt+Shift+Down"],
    ),
    (Context::Editor, Command::FetchAll, &["Alt+End"]),
    (Context::Editor, Command::ShowMessages, &["Alt+M"]),
    (
        Context::Editor,
        Command::NextResult,
        &["Ctrl+PageDown", "Alt+PageDown"],
    ),
    (
        Context::Editor,
        Command::PrevResult,
        &["Ctrl+PageUp", "Alt+PageUp"],
    ),
    (Context::Editor, Command::ResultsSmaller, &["Alt+Shift+Up"]),
    (Context::Editor, Command::ResultsLarger, &["Alt+Shift+Down"]),
    (Context::Dashboard, Command::DashboardRefresh, &["F5"]),
    (Context::Dashboard, Command::DashboardPause, &["Ctrl+P"]),
    (Context::Dashboard, Command::DashboardInterval, &["Ctrl+T"]),
    (Context::Dashboard, Command::HideIdleSessions, &["Alt+I"]),
    (
        Context::Dashboard,
        Command::NextList,
        &["Ctrl+PageDown", "Alt+PageDown"],
    ),
    (
        Context::Dashboard,
        Command::PrevList,
        &["Ctrl+PageUp", "Alt+PageUp"],
    ),
    (Context::Dashboard, Command::CancelBackend, &["Alt+Shift+C"]),
    (Context::Dashboard, Command::TerminateBackend, &["F8"]),
    (Context::Dashboard, Command::ShowRow, &["Enter"]),
    (Context::Dashboard, Command::ContextMenu, &["F2"]),
    (
        Context::Dashboard,
        Command::Copy,
        &["Ctrl+C", "Ctrl+Insert"],
    ),
    (Context::Dashboard, Command::SelectAll, &["Ctrl+A"]),
    (Context::Dashboard, Command::Up, &["Up"]),
    (Context::Dashboard, Command::Down, &["Down"]),
    (Context::Dashboard, Command::Left, &["Left"]),
    (Context::Dashboard, Command::Right, &["Right"]),
    (Context::Dashboard, Command::PageUp, &["PageUp"]),
    (Context::Dashboard, Command::PageDown, &["PageDown"]),
    (Context::Dashboard, Command::Home, &["Home"]),
    (Context::Dashboard, Command::End, &["End"]),
    (Context::Dashboard, Command::DocStart, &["Ctrl+Home"]),
    (Context::Dashboard, Command::DocEnd, &["Ctrl+End"]),
    // the statement pane (D-059)
    (Context::Dashboard, Command::ToggleResults, &["Ctrl+R"]),
    (
        Context::Dashboard,
        Command::ResultsSmaller,
        &["Alt+Shift+Up"],
    ),
    (
        Context::Dashboard,
        Command::ResultsLarger,
        &["Alt+Shift+Down"],
    ),
    (Context::Dashboard, Command::EditInQueryTab, &["Ctrl+E"]),
    (Context::Profiler, Command::ProfilerStartStop, &["F5"]),
    (Context::Profiler, Command::ProfilerPause, &["Ctrl+P"]),
    (Context::Profiler, Command::ProfilerFilter, &["Ctrl+F"]),
    (Context::Profiler, Command::ProfilerClear, &["Ctrl+L"]),
    (Context::Profiler, Command::ShowRow, &["Enter"]),
    (Context::Profiler, Command::ContextMenu, &["F2"]),
    (Context::Profiler, Command::Copy, &["Ctrl+C", "Ctrl+Insert"]),
    (Context::Profiler, Command::SelectAll, &["Ctrl+A"]),
    (Context::Profiler, Command::Up, &["Up"]),
    (Context::Profiler, Command::Down, &["Down"]),
    (Context::Profiler, Command::Left, &["Left"]),
    (Context::Profiler, Command::Right, &["Right"]),
    (Context::Profiler, Command::PageUp, &["PageUp"]),
    (Context::Profiler, Command::PageDown, &["PageDown"]),
    (Context::Profiler, Command::Home, &["Home"]),
    (Context::Profiler, Command::End, &["End"]),
    (Context::Profiler, Command::DocStart, &["Ctrl+Home"]),
    (Context::Profiler, Command::DocEnd, &["Ctrl+End"]),
    // the statement pane (D-059)
    (Context::Profiler, Command::ToggleResults, &["Ctrl+R"]),
    (
        Context::Profiler,
        Command::ResultsSmaller,
        &["Alt+Shift+Up"],
    ),
    (
        Context::Profiler,
        Command::ResultsLarger,
        &["Alt+Shift+Down"],
    ),
    (Context::Profiler, Command::EditInQueryTab, &["Ctrl+E"]),
    (Context::DbStats, Command::DbStatsRefresh, &["F5"]),
    (Context::DbStats, Command::ContextMenu, &["F2"]),
    (Context::DbStats, Command::Copy, &["Ctrl+C", "Ctrl+Insert"]),
    (Context::DbStats, Command::SelectAll, &["Ctrl+A"]),
    (Context::DbStats, Command::Up, &["Up"]),
    (Context::DbStats, Command::Down, &["Down"]),
    (Context::DbStats, Command::Left, &["Left"]),
    (Context::DbStats, Command::Right, &["Right"]),
    (Context::DbStats, Command::PageUp, &["PageUp"]),
    (Context::DbStats, Command::PageDown, &["PageDown"]),
    (Context::DbStats, Command::Home, &["Home"]),
    (Context::DbStats, Command::End, &["End"]),
    (Context::DbStats, Command::DocStart, &["Ctrl+Home"]),
    (Context::DbStats, Command::DocEnd, &["Ctrl+End"]),
    (Context::Dialog, Command::NextField, &["Tab"]),
    (Context::Dialog, Command::PrevField, &["Shift+Tab"]),
    (Context::Dialog, Command::Up, &["Up"]),
    (Context::Dialog, Command::Down, &["Down"]),
    (Context::Dialog, Command::Left, &["Left"]),
    (Context::Dialog, Command::Right, &["Right"]),
    (Context::Dialog, Command::PageUp, &["PageUp"]),
    (Context::Dialog, Command::PageDown, &["PageDown"]),
    (Context::Dialog, Command::Home, &["Home"]),
    (Context::Dialog, Command::End, &["End"]),
    (Context::Dialog, Command::Accept, &["Enter"]),
    (Context::Dialog, Command::DialogOk, &["Ctrl+Enter"]),
    (Context::Dialog, Command::Cancel, &["Esc", "F10"]),
    (Context::Dialog, Command::Toggle, &["Space"]),
    (Context::Dialog, Command::Backspace, &["Backspace"]),
    (Context::Dialog, Command::Delete, &["Delete"]),
];

/// Key bindings per context.
#[derive(Debug, Clone)]
pub struct Keymap {
    maps: HashMap<Context, HashMap<KeyCombo, Command>>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut keymap = Self {
            maps: HashMap::new(),
        };
        for (ctx, cmd, keys) in DEFAULTS {
            for key in *keys {
                let combo = KeyCombo::parse(key).expect("built-in key names are valid");
                keymap.bind(*ctx, combo, *cmd);
            }
        }
        keymap
    }
}

impl Keymap {
    fn bind(&mut self, ctx: Context, key: KeyCombo, cmd: Command) {
        self.maps.entry(ctx).or_default().insert(key, cmd);
    }

    fn unbind_command(&mut self, ctx: Context, cmd: Command) {
        if let Some(map) = self.maps.get_mut(&ctx) {
            map.retain(|_, c| *c != cmd);
        }
    }

    /// Command for `key` in `ctx`, falling back to `[global]`.
    pub fn lookup(&self, ctx: Context, key: KeyCombo) -> Option<Command> {
        self.lookup_only(ctx, key)
            .or_else(|| self.lookup_only(Context::Global, key))
    }

    /// Command for `key` in `ctx` only.
    pub fn lookup_only(&self, ctx: Context, key: KeyCombo) -> Option<Command> {
        self.maps.get(&ctx).and_then(|m| m.get(&key)).copied()
    }

    /// All keys bound to `cmd` in `ctx`, sorted for stable display.
    pub fn keys_for(&self, ctx: Context, cmd: Command) -> Vec<KeyCombo> {
        let mut keys: Vec<KeyCombo> = self
            .maps
            .get(&ctx)
            .into_iter()
            .flatten()
            .filter(|(_, c)| **c == cmd)
            .map(|(k, _)| *k)
            .collect();
        keys.sort_by_key(|k| k.to_string());
        keys
    }

    /// First key for `cmd`, searching `contexts` in order (for menu shortcuts).
    pub fn shortcut(&self, contexts: &[Context], cmd: Command) -> Option<KeyCombo> {
        contexts
            .iter()
            .find_map(|ctx| self.keys_for(*ctx, cmd).into_iter().next())
    }

    /// Applies `keymap.toml` text. Returns warnings for entries that were skipped.
    pub fn apply_overrides(&mut self, text: &str) -> Result<Vec<String>, String> {
        let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut warnings = Vec::new();
        for (section, value) in &table {
            let Some(ctx) = Context::from_name(section) else {
                warnings.push(format!("unknown section [{section}]"));
                continue;
            };
            let Some(entries) = value.as_table() else {
                warnings.push(format!("[{section}] is not a table"));
                continue;
            };
            for (name, keys) in entries {
                let Some(cmd) = Command::from_name(name) else {
                    warnings.push(format!("[{section}] unknown command '{name}'"));
                    continue;
                };
                let names: Vec<&str> = match keys {
                    toml::Value::String(s) => vec![s.as_str()],
                    toml::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
                    _ => {
                        warnings.push(format!("[{section}] {name}: expected key name(s)"));
                        continue;
                    }
                };
                let mut combos = Vec::new();
                for n in names {
                    match KeyCombo::parse(n) {
                        Ok(c) => combos.push(c),
                        Err(e) => warnings.push(format!("[{section}] {name}: {e}")),
                    }
                }
                self.unbind_command(ctx, cmd);
                for combo in combos {
                    self.bind(ctx, combo, cmd);
                }
            }
        }
        Ok(warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> KeyCombo {
        KeyCombo::parse(s).unwrap()
    }

    #[test]
    fn command_names_are_unique() {
        for (i, a) in Command::ALL.iter().enumerate() {
            for b in &Command::ALL[i + 1..] {
                assert_ne!(a.name(), b.name());
            }
        }
    }

    #[test]
    fn context_key_wins_over_global() {
        let km = Keymap::default();
        assert_eq!(
            km.lookup(Context::Dialog, k("Tab")),
            Some(Command::NextField)
        );
        // Tab does nothing in the tree and the results; Ctrl+Tab / F6 switch panels
        assert_eq!(km.lookup(Context::Tree, k("Tab")), None);
        assert_eq!(km.lookup(Context::Results, k("Tab")), None);
        for ctx in [Context::Tree, Context::Editor, Context::Results] {
            for key in ["Ctrl+Tab", "F6"] {
                assert_eq!(km.lookup(ctx, k(key)), Some(Command::SwitchPanel), "{key}");
            }
        }
        assert_eq!(
            km.lookup(Context::Tree, k("Ctrl+R")),
            Some(Command::RefreshTree)
        );
        assert_eq!(
            km.lookup(Context::Editor, k("Ctrl+R")),
            Some(Command::ToggleResults)
        );
        assert_eq!(km.lookup(Context::Editor, k("F1")), Some(Command::Help));
        assert_eq!(km.lookup(Context::Dialog, k("F10")), Some(Command::Cancel));
        // D-038: Tab indents in the editor; F6 / Ctrl+Tab leave it
        assert_eq!(km.lookup(Context::Editor, k("Tab")), Some(Command::Indent));
        assert_eq!(
            km.lookup(Context::Editor, k("F6")),
            Some(Command::SwitchPanel)
        );
        assert_eq!(
            km.lookup(Context::Editor, k("Ctrl+Tab")),
            Some(Command::SwitchPanel)
        );
        assert_eq!(
            km.lookup(Context::Editor, k("Ctrl+Shift+Z")),
            Some(Command::Redo)
        );
    }

    #[test]
    fn dashboard_and_profiler_keys_in_the_tree() {
        // D-057, D-058: Ctrl+P keeps its meaning outside the tree
        let km = Keymap::default();
        assert_eq!(
            km.lookup(Context::Tree, k("Ctrl+D")),
            Some(Command::Dashboard)
        );
        assert_eq!(
            km.lookup(Context::Tree, k("Ctrl+P")),
            Some(Command::SqlProfiler)
        );
        assert_eq!(
            km.lookup(Context::Editor, k("Ctrl+P")),
            Some(Command::TogglePlpgsql)
        );
        assert_eq!(
            km.lookup(Context::Dashboard, k("Ctrl+P")),
            Some(Command::DashboardPause)
        );
        // D-062: Alt+I in the tree; Alt+S stays quick search
        assert_eq!(km.lookup(Context::Tree, k("Alt+I")), Some(Command::DbStats));
        assert_eq!(
            km.lookup(Context::Tree, k("Alt+S")),
            Some(Command::QuickSearch)
        );
        assert_eq!(
            km.lookup(Context::DbStats, k("F5")),
            Some(Command::DbStatsRefresh)
        );
    }

    #[test]
    fn second_keys_for_keys_terminals_keep() {
        // D-053
        let km = Keymap::default();
        let ed = Context::Editor;
        assert_eq!(
            km.lookup(ed, k("Alt+Shift+Home")),
            Some(Command::SelectDocStart)
        );
        assert_eq!(
            km.lookup(ed, k("Alt+Shift+End")),
            Some(Command::SelectDocEnd)
        );
        assert_eq!(km.lookup(ed, k("Alt+F3")), Some(Command::FindPrev));
        for ctx in [Context::Editor, Context::Results] {
            assert_eq!(km.lookup(ctx, k("Alt+PageDown")), Some(Command::NextResult));
            assert_eq!(km.lookup(ctx, k("Alt+PageUp")), Some(Command::PrevResult));
        }
        assert_eq!(km.lookup(ed, k("Ctrl+Space")), Some(Command::Complete));
        assert_eq!(km.lookup(ed, k("Alt+/")), Some(Command::Complete));
    }

    #[test]
    fn overrides_replace_and_unbind() {
        let mut km = Keymap::default();
        let warnings = km
            .apply_overrides(
                r#"
                [tree]
                refresh_tree = "F6"
                drop_object = []
                [global]
                next_tab = ["Ctrl+PageDown", "Alt+Right"]
                "#,
            )
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            km.lookup(Context::Tree, k("F6")),
            Some(Command::RefreshTree)
        );
        assert_eq!(km.lookup(Context::Tree, k("Ctrl+R")), None);
        assert_eq!(km.lookup(Context::Tree, k("F8")), None);
        assert_eq!(
            km.lookup(Context::Tree, k("Ctrl+PgDn")),
            Some(Command::NextTab)
        );
    }

    #[test]
    fn bad_entries_are_reported() {
        let mut km = Keymap::default();
        let w = km
            .apply_overrides("[nope]\na = 1\n[tree]\nfoo = \"F1\"\nhelp = \"Hyper+X\"\n")
            .unwrap();
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(km.apply_overrides("[tree").is_err());
    }
}
