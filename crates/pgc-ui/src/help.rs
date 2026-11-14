//! Help menu (D-014, D-016): keyboard shortcuts from the active keymap, third-party
//! licences, About pgtc. The `F1` window (the keys that work now, D-063) is built in
//! `app/key_help.rs` from the parts here.

use crate::app::App;
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::keymap::{Command, Context, Group, Keymap};
use crate::keys::KeyCombo;

/// Licence text of the pgAdmin SQL templates (D-010).
const PGADMIN_LICENCE: &str = include_str!("../../../THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt");
/// Licences of the Rust crates pgtc is built from, generated with `cargo about`
/// (`THIRD_PARTY_LICENSES/README.md`). Lines starting with `## ` are headings.
const CRATE_LICENCES: &str = include_str!("../../../THIRD_PARTY_LICENSES/rust-crates.txt");

const TEXT_ID: &str = "text";
/// Licence lines are wrapped to this width (some licence files have one line per
/// paragraph).
const WRAP: usize = 80;

/// Word-wraps `line` to `width` columns, keeping its indentation on every part.
pub(crate) fn wrap(line: &str, width: usize) -> Vec<String> {
    let line = line.trim_end();
    if line.chars().count() <= width {
        return vec![line.to_owned()];
    }
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    let mut out = Vec::new();
    let mut cur = indent.clone();
    for word in line.split_whitespace() {
        let len = cur.chars().count();
        if len > indent.chars().count() && len + 1 + word.chars().count() > width {
            out.push(std::mem::replace(&mut cur, indent.clone()));
        }
        if cur.chars().count() > indent.chars().count() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    out.push(cur);
    out
}

/// Sections of the shortcuts window, in order.
pub(crate) const SECTIONS: [(Context, &str); 8] = [
    (Context::Global, "Everywhere"),
    (Context::Tree, "Object tree"),
    (Context::Editor, "Editor and object view"),
    (Context::Results, "Results"),
    (Context::Dashboard, "Server dashboard"),
    (Context::Profiler, "SQL profiler"),
    (Context::DbStats, "Database statistics"),
    (Context::Dialog, "Dialogs and lists"),
];

/// Text of a Help window: lines and heading marks.
pub struct Lines {
    pub lines: Vec<String>,
    pub headers: Vec<bool>,
}

impl Lines {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            headers: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, line: String, header: bool) {
        self.lines.push(line);
        self.headers.push(header);
    }
}

/// Rows of one section of a keys window: the key(s), what they do, and the group
/// (D-065).
pub(crate) type KeyRows = Vec<(String, &'static str, Group)>;

/// A section with more rows than this (and more than one group) shows the group names
/// as subheadings (D-065).
const SUBHEADINGS_FROM: usize = 8;

/// Key rows of the commands, group by group (D-065); `keys` gives
/// the keys shown for a command (none: left out).
pub(crate) fn command_rows(keys: impl Fn(Command) -> Vec<KeyCombo>) -> KeyRows {
    let mut rows: KeyRows = Command::ALL
        .iter()
        .filter_map(|&cmd| {
            let k = keys(cmd);
            (!k.is_empty()).then(|| (key_names(&k), cmd.description(), cmd.group()))
        })
        .collect();
    // stable: the table's order inside a group
    rows.sort_by_key(|r| r.2);
    rows
}

/// Keys that are not commands of the keymap.
pub(crate) const ESC_DIGIT: (&str, &str) = (
    "Esc, digit",
    "F1 … F10 (Esc 1 … Esc 0), for terminals without F-keys",
);
pub(crate) const MENU_LETTER: (&str, &str) = ("F9, letter", "Menu with that underlined letter");
pub(crate) const FIELD_LETTER: (&str, &str) =
    ("Alt+letter", "Dialog field with that underlined letter");

/// The keys of one row, as shown: `Ctrl+C, Ctrl+Insert`.
pub(crate) fn key_names(keys: &[KeyCombo]) -> String {
    let names: Vec<String> = keys.iter().map(ToString::to_string).collect();
    names.join(", ")
}

/// Titled sections of key rows as window text; sections without rows are left out.
pub(crate) fn key_sections(sections: &[(&str, KeyRows)]) -> Lines {
    let width = sections
        .iter()
        .flat_map(|(_, rows)| rows)
        .map(|(k, _, _)| k.chars().count())
        .max()
        .unwrap_or(0)
        .min(24);
    let mut out = Lines::new();
    for (title, rows) in sections {
        if rows.is_empty() {
            continue;
        }
        if !out.lines.is_empty() {
            out.push(String::new(), false);
        }
        out.push((*title).to_owned(), true);
        let sub = rows.len() > SUBHEADINGS_FROM && rows.iter().any(|r| r.2 != rows[0].2);
        let mut group = None;
        for (keys, desc, g) in rows {
            if sub && group != Some(*g) {
                out.push(format!("  {}", g.title()), true);
                group = Some(*g);
            }
            let indent = if sub { "    " } else { "  " };
            out.push(format!("{indent}{keys:<width$}  {desc}"), false);
        }
    }
    out
}

/// Every bound key of `km`, grouped by context (D-016).
pub fn shortcut_lines(km: &Keymap, keymap_file: &str) -> Lines {
    let mut sections: Vec<(&str, KeyRows)> = SECTIONS
        .iter()
        .map(|&(ctx, title)| (title, command_rows(|cmd| km.keys_for(ctx, cmd))))
        .collect();
    let other = [ESC_DIGIT, MENU_LETTER, FIELD_LETTER]
        .iter()
        .map(|&(k, d)| (k.to_owned(), d, Group::General))
        .collect();
    sections.push(("Other keys", other));
    let mut out = key_sections(&sections);
    out.push(String::new(), false);
    // keys a terminal may keep for itself (D-053)
    out.push("If a key does not work".to_owned(), true);
    for l in [
        "  Some terminals keep keys for themselves (e.g. Ctrl+Tab, Shift+PgUp,",
        "  Ctrl+PgUp / PgDn, Ctrl+Shift+Home / End, Alt+Shift+arrows). Most commands",
        "  have a second key above, and most are also in the menus (F9).",
        "  macOS Terminal / iTerm2: turn on \"Use Option as Meta key\" for Alt keys.",
        "  Inside GNU screen, Ctrl+A goes to screen.",
        "  Any key can be changed in keymap.toml (below).",
    ] {
        out.push(l.to_owned(), false);
    }
    out.push(String::new(), false);
    out.push("Keys are changed in".to_owned(), true);
    out.push(format!("  {keymap_file}"), false);
    out
}

/// Text of the licences window: the pgAdmin licence, then the crate licences.
pub fn licence_lines() -> Lines {
    let mut out = Lines::new();
    out.push(
        "pgAdmin 4 SQL templates (PostgreSQL Licence)".to_owned(),
        true,
    );
    out.push(String::new(), false);
    // what was taken from pgAdmin, like the "Used by" lists of the crates (D-010)
    for l in [
        "Used by:",
        "  pgtc's catalog queries (the embedded sql/ folder), adapted from the SQL",
        "  templates of pgAdmin 4: the object tree (nodes.sql), object properties and",
        "  definitions. Each adapted file names its pgAdmin source.",
    ] {
        out.push(l.to_owned(), false);
    }
    out.push(String::new(), false);
    for l in PGADMIN_LICENCE.lines() {
        for part in wrap(l, WRAP) {
            out.push(part, false);
        }
    }
    out.push(String::new(), false);
    for l in CRATE_LICENCES.lines() {
        match l.strip_prefix("## ") {
            Some(h) => out.push(h.to_owned(), true),
            None => {
                for part in wrap(l, WRAP) {
                    out.push(part, false);
                }
            }
        }
    }
    out
}

/// A Help window: scrolling text and a Close button.
pub(crate) fn viewer(title: &str, lines: Lines) -> Dialog {
    let longest = lines
        .lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    // padding and frames (6) + the scroll-mark column
    let width = longest.max(title.chars().count() + 4) as u16 + 7;
    Dialog::new(
        DialogKind::Viewer,
        title,
        width,
        vec![
            Widget::text(TEXT_ID, lines.lines, lines.headers),
            Widget::button("ok", "&Close", true),
        ],
    )
}

impl App {
    /// Help → Keyboard shortcuts: every key of the active keymap.
    pub(crate) fn open_shortcuts(&mut self) {
        if self.viewer_open() {
            return;
        }
        let file = self.paths.keymap_file().display().to_string();
        let mut d = viewer("Keyboard shortcuts", shortcut_lines(&self.keymap, &file));
        d.fit_text(self.screen);
        self.dialogs.push(d);
    }

    /// A Help window is on top (`F1` does not stack a second one).
    pub(crate) fn viewer_open(&self) -> bool {
        self.dialogs
            .last()
            .is_some_and(|d| d.kind == DialogKind::Viewer)
    }

    /// Help → Third-party licences (D-017).
    pub(crate) fn open_licences(&mut self) {
        let mut d = viewer("Third-party licences", licence_lines());
        d.fit_text(self.screen);
        self.dialogs.push(d);
    }

    /// Help → About pgtc.
    pub(crate) fn open_about(&mut self) {
        let text = format!(
            "pgtc — Postgres terminal commander\n\
             Version {}\n\
             \n\
             Copyright (c) 2026, Darko Prenosil\n\
             Licensed under the PostgreSQL Licence.\n\
             \n\
             Third-party licences: Help → Third-party licences",
            env!("CARGO_PKG_VERSION")
        );
        self.dialogs
            .push(Dialog::message("About pgtc", &text, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_come_from_the_keymap() {
        let mut km = Keymap::default();
        let l = shortcut_lines(&km, "keymap.toml");
        let text = l.lines.join("\n");
        assert!(text.contains("Everywhere\n"), "{text}");
        assert!(
            l.lines.iter().any(|r| r.starts_with("    F5 ")
                && r.ends_with("Execute the selection or the whole script")),
            "{text}"
        );
        assert!(text.contains("Ctrl+C, Ctrl+Insert"));
        let tree = l.lines.iter().position(|x| x == "Object tree").unwrap();
        assert!(l.headers[tree]);
        // 8 contexts, Other keys, If a key does not work, Keys are changed in
        let top = l
            .lines
            .iter()
            .zip(&l.headers)
            .filter(|(t, h)| **h && !t.starts_with(' '))
            .count();
        assert_eq!(top, 11, "{text}");
        // larger sections in groups with subheadings, in the groups' order (D-065)
        let at = |s: &str| l.lines.iter().skip(tree).position(|x| x == s).unwrap();
        assert!(at("  Actions") < at("  Moving"), "{text}");
        assert!(at("  Moving") < at("  Editing"), "{text}");
        assert!(
            l.lines.iter().any(|r| r.starts_with("    Ctrl+Y ")
                && r.ends_with("Delete the line (the selected lines)"))
        );
        assert!(text.contains("Alt+Shift+Z, Ctrl+Shift+Z"), "{text}");
        // small sections are only ordered: Other keys has no subheadings
        let other = l.lines.iter().position(|x| x == "Other keys").unwrap();
        assert!(l.lines[other + 1].starts_with("  Esc, digit"), "{text}");
        assert!(text.ends_with("  keymap.toml"));
        assert!(
            text.contains(
                "If a key does not work
"
            ),
            "{text}"
        );
        assert!(text.contains("Use Option as Meta key"));

        km.apply_overrides("[editor]\nexecute = \"Ctrl+E\"\n")
            .unwrap();
        let text = shortcut_lines(&km, "k").lines.join("\n");
        assert!(text.contains("  Ctrl+E "), "{text}");
    }

    #[test]
    fn long_lines_are_wrapped_with_their_indent() {
        assert_eq!(wrap("short", 10), vec!["short"]);
        assert_eq!(
            wrap("  aaa bbb ccc ddd", 10),
            vec!["  aaa bbb", "  ccc ddd"]
        );
        assert_eq!(wrap("abcdefghijkl x", 5), vec!["abcdefghijkl", "x"]);
    }

    #[test]
    fn licences_include_pgadmin_and_the_crates() {
        let l = licence_lines();
        assert!(l.headers[0]);
        let text = l.lines.join("\n");
        assert!(text.contains("pgAdmin"));
        assert!(
            text.contains("Used by:\n  pgtc's catalog queries"),
            "{text}"
        );
        assert!(text.contains("ratatui"), "crate list missing");
        // long words (URLs) may stay longer than the wrap width
        assert!(l.lines.iter().all(|x| x.chars().count() <= WRAP + 40));
        assert!(
            !text.contains("\n## "),
            "headings are marked, not shown with ##"
        );
    }
}
