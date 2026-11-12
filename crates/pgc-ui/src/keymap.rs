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
    Dialog,
}

impl Context {
    pub const ALL: [Context; 5] = [
        Context::Global,
        Context::Tree,
        Context::Editor,
        Context::Results,
        Context::Dialog,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Context::Global => "global",
            Context::Tree => "tree",
            Context::Editor => "editor",
            Context::Results => "results",
            Context::Dialog => "dialog",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }
}

macro_rules! commands {
    ($($variant:ident = $name:literal, $desc:literal;)*) => {
        /// Everything the user can trigger by key or menu.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Command { $($variant),* }

        impl Command {
            pub const ALL: &'static [Command] = &[$(Command::$variant),*];

            /// Name used in `keymap.toml`.
            pub fn name(self) -> &'static str {
                match self { $(Command::$variant => $name),* }
            }

            /// Short description (Help → Keyboard shortcuts).
            pub fn description(self) -> &'static str {
                match self { $(Command::$variant => $desc),* }
            }
        }
    };
}

commands! {
    // global
    Help = "help", "Keyboard shortcuts of the focused frame";
    Menu = "menu", "Open the menu bar";
    Quit = "quit", "Quit pgtc";
    SwitchPanel = "switch_panel", "Switch between the panels";
    NextTab = "next_tab", "Next tab";
    PrevTab = "prev_tab", "Previous tab";
    CloseTab = "close_tab", "Close the tab";
    // tree
    ContextMenu = "context_menu", "Context menu";
    ShowInNewTab = "show_in_new_tab", "Show the object in a new tab";
    NewFromTemplate = "new_from_template", "Generate SQL from developer template";
    DropObject = "drop_object", "Delete the saved connection";
    RefreshTree = "refresh_tree", "Refresh the object tree";
    ToggleSystemObjects = "toggle_system_objects", "Show / hide system objects";
    QuickSearch = "quick_search", "Quick search";
    // query tab
    Execute = "execute", "Execute the selection or the whole script";
    CancelQuery = "cancel_query", "Cancel the running query";
    ToggleResults = "toggle_results", "Show / hide the results";
    // navigation
    Up = "up", "Up";
    Down = "down", "Down";
    Left = "left", "Left / collapse";
    Right = "right", "Right / expand";
    PageUp = "page_up", "Page up";
    PageDown = "page_down", "Page down";
    Home = "home", "Start of line / first row";
    End = "end", "End of line / last row";
    DocStart = "doc_start", "Start of the text";
    DocEnd = "doc_end", "End of the text";
    // editor (D-015, D-038)
    WordLeft = "word_left", "Word left";
    WordRight = "word_right", "Word right";
    SelectLeft = "select_left", "Select left";
    SelectRight = "select_right", "Select right";
    SelectUp = "select_up", "Select up";
    SelectDown = "select_down", "Select down";
    SelectWordLeft = "select_word_left", "Select word left";
    SelectWordRight = "select_word_right", "Select word right";
    SelectHome = "select_home", "Select to start of line";
    SelectEnd = "select_end", "Select to end of line";
    SelectPageUp = "select_page_up", "Select page up";
    SelectPageDown = "select_page_down", "Select page down";
    SelectDocStart = "select_doc_start", "Select to start of the text";
    SelectDocEnd = "select_doc_end", "Select to end of the text";
    SelectAll = "select_all", "Select all";
    Copy = "copy", "Copy";
    Cut = "cut", "Cut";
    Paste = "paste", "Paste";
    Undo = "undo", "Undo";
    Redo = "redo", "Redo";
    Indent = "indent", "Indent";
    Unindent = "unindent", "Unindent";
    Newline = "newline", "New line";
    DeleteWordLeft = "delete_word_left", "Delete word left";
    DeleteWordRight = "delete_word_right", "Delete word right";
    Find = "find", "Find / replace";
    Complete = "complete", "Autocomplete: names and keywords";
    Psql = "psql_command", "psql-style command line (dt, d, gti, ...)";
    FindNext = "find_next", "Find next";
    FindPrev = "find_prev", "Find previous";
    // query tab (D-019, D-020, D-041)
    FetchAll = "fetch_all", "Fetch all rows";
    ExportResult = "export_result", "Export the result";
    Commit = "commit", "Commit";
    Rollback = "rollback", "Rollback";
    ToggleAutoCommit = "toggle_auto_commit", "Auto commit on / off";
    ToggleAutoRollback = "toggle_auto_rollback", "Auto rollback on error on / off";
    TogglePlpgsql = "toggle_plpgsql", "PL/pgSQL mode on / off";
    ShowMessages = "show_messages", "Switch between results and messages";
    NextResult = "next_result", "Next result set";
    PrevResult = "prev_result", "Previous result set";
    ResultsSmaller = "results_smaller", "Make the results smaller";
    ResultsLarger = "results_larger", "Make the results larger";
    // dialogs and menus
    NextField = "next_field", "Next field";
    PrevField = "prev_field", "Previous field";
    Accept = "accept", "Open / choose / OK";
    Cancel = "cancel", "Cancel / close";
    Toggle = "toggle", "Toggle the checkbox";
    Backspace = "backspace", "Delete left";
    Delete = "delete", "Delete right";
    // menu commands (no default key)
    NewConnection = "new_connection", "New connection";
    EditConnection = "edit_connection", "Edit the connection";
    DeleteConnection = "delete_connection", "Delete the connection";
    Connect = "connect", "Connect";
    Disconnect = "disconnect", "Disconnect";
    NewQuery = "new_query", "New query tab";
    OpenFile = "open_file", "Open SQL file";
    SaveFile = "save_file", "Save SQL file";
    ExportDefinition = "export_definition", "Export database definition";
    LoadDefinition = "load_definition", "Load database definition";
    Settings = "settings", "Settings";
    RunSetup = "run_setup", "Run setup";
    InstallTemplates = "install_templates", "Install templates";
    ShowTemplatesFolder = "show_templates_folder", "Show templates folder";
    KeyboardShortcuts = "keyboard_shortcuts", "Keyboard shortcuts";
    ThirdPartyLicences = "third_party_licences", "Third-party licences";
    About = "about", "About pgtc";
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
    (Context::Editor, Command::Redo, &["Ctrl+Y", "Ctrl+Shift+Z"]),
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
