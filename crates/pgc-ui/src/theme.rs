//! Colour schemes (D-014). Colours are only ever taken from theme roles.
//!
//! A scheme is a TOML file with mc-like sections; each role is a style string:
//!
//! ```toml
//! name = "My scheme"
//! [core]
//! normal = "lightgray on blue"
//! selected = "black on cyan bold"
//! ```
//!
//! Colours: `default`, the 16 names (`black`, `red`, ..., `lightgray`, `darkgray`,
//! `lightred`, ..., `white`), `#rrggbb` or a palette index `0`..`255`.
//! Modifiers: `bold`, `dim`, `italic`, `underlined`, `reversed`.
//! Roles missing in a scheme are taken from **Classic**.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

macro_rules! roles {
    ($($variant:ident = $name:literal;)*) => {
        /// Theme roles.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Role { $($variant),* }

        impl Role {
            pub const ALL: &'static [Role] = &[$(Role::$variant),*];

            /// `section.key` name in scheme files.
            pub fn name(self) -> &'static str {
                match self { $(Role::$variant => $name),* }
            }
        }
    };
}

roles! {
    CoreNormal = "core.normal";
    CoreBorder = "core.border";
    CoreTitle = "core.title";
    CoreTitleFocused = "core.title_focused";
    CoreSelected = "core.selected";
    CoreSelectedUnfocused = "core.selected_unfocused";
    CoreHint = "core.hint";
    MenuNormal = "menu.normal";
    MenuHotkey = "menu.hotkey";
    MenuSelected = "menu.selected";
    MenuSelectedHotkey = "menu.selected_hotkey";
    MenuDisabled = "menu.disabled";
    DialogNormal = "dialog.normal";
    DialogFocus = "dialog.focus";
    DialogHotkey = "dialog.hotkey";
    DialogFocusHotkey = "dialog.focus_hotkey";
    DialogTitle = "dialog.title";
    DialogDisabled = "dialog.disabled";
    DialogInput = "dialog.input";
    DialogInputFocus = "dialog.input_focus";
    DialogShadow = "dialog.shadow";
    ErrorNormal = "error.normal";
    ErrorTitle = "error.title";
    TabsBar = "tabs.bar";
    TabsActive = "tabs.active";
    TabsActiveUnfocused = "tabs.active_unfocused";
    TabsInactive = "tabs.inactive";
    StatusNormal = "status.normal";
    FkeyNumber = "fkeybar.number";
    FkeyLabel = "fkeybar.label";
    EditorText = "editor.text";
    EditorKeyword = "editor.keyword";
    EditorString = "editor.string";
    EditorNumber = "editor.number";
    EditorComment = "editor.comment";
    EditorQuotedIdent = "editor.quoted_ident";
    EditorOperator = "editor.operator";
    EditorSelection = "editor.selection";
    EditorLineNumber = "editor.line_number";
    GridHeader = "grid.header";
    GridNull = "grid.null";
    GridCursor = "grid.cursor";
    GridRowNumber = "grid.row_number";
    MessagesError = "messages.error";
    MessagesNotice = "messages.notice";
    MessagesInfo = "messages.info";
    DashboardGraph = "dashboard.graph";
    DashboardBlocked = "dashboard.blocked";
}

/// Junction characters of a single frame, from [`Theme::plain_junctions`].
#[derive(Debug, Clone, Copy)]
pub struct Junctions {
    /// An inner horizontal line meets the left frame (`├`).
    pub left: &'static str,
    /// An inner horizontal line meets the right frame (`┤`).
    pub right: &'static str,
    /// An inner vertical line meets the top frame (`┬`).
    pub top: &'static str,
    /// An inner vertical line meets a horizontal line below it (`┴`).
    pub bottom: &'static str,
}

/// A resolved colour scheme.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    styles: Vec<Style>,
    /// ASCII frames (D-051): not part of a scheme file; set from the settings.
    pub ascii: bool,
}

impl Theme {
    /// Window frames (double lines, D-014).
    pub fn double_border(&self) -> ratatui::symbols::border::Set<'static> {
        if self.ascii {
            ratatui::symbols::border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "=",
                horizontal_bottom: "=",
            }
        } else {
            ratatui::symbols::border::DOUBLE
        }
    }

    /// Dialog, menu and combo-list frames (single lines).
    pub fn plain_border(&self) -> ratatui::symbols::border::Set<'static> {
        if self.ascii {
            ratatui::symbols::border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            }
        } else {
            ratatui::symbols::border::PLAIN
        }
    }

    /// Single horizontal line (inner separators, menu separators).
    pub fn hline(&self) -> &'static str {
        if self.ascii { "-" } else { "─" }
    }

    /// Single vertical line (grid columns, results bar).
    pub fn vline(&self) -> &'static str {
        if self.ascii { "|" } else { "│" }
    }

    /// Ends of an inner separator joined to a double window frame (`╟───╢`).
    pub fn separator_ends(&self) -> (&'static str, &'static str) {
        if self.ascii {
            ("+", "+")
        } else {
            ("╟", "╢")
        }
    }

    /// Where inner lines meet a single dialog frame or each other (`├ ┤ ┬ ┴`).
    pub fn plain_junctions(&self) -> Junctions {
        if self.ascii {
            Junctions {
                left: "+",
                right: "+",
                top: "+",
                bottom: "+",
            }
        } else {
            Junctions {
                left: "├",
                right: "┤",
                top: "┬",
                bottom: "┴",
            }
        }
    }

    pub fn get(&self, role: Role) -> Style {
        self.styles[role as usize]
    }

    /// Parses a scheme file on top of `base` (roles not in the file keep `base`'s
    /// style). `fallback_name` is used when the file has no `name`.
    /// Returns the theme and warnings for skipped entries.
    pub fn parse(
        text: &str,
        base: Option<&Theme>,
        fallback_name: &str,
    ) -> Result<(Theme, Vec<String>), String> {
        #[derive(Deserialize)]
        struct File {
            name: Option<String>,
            #[serde(flatten)]
            sections: toml::Table,
        }
        let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut theme = Theme {
            name: file.name.unwrap_or_else(|| fallback_name.to_owned()),
            styles: base.map_or_else(
                || vec![Style::default(); Role::ALL.len()],
                |b| b.styles.clone(),
            ),
            ascii: false,
        };
        let mut warnings = Vec::new();
        for (section, value) in &file.sections {
            let Some(entries) = value.as_table() else {
                warnings.push(format!("'{section}' is not a section"));
                continue;
            };
            for (key, value) in entries {
                let full = format!("{section}.{key}");
                let Some(role) = Role::ALL.iter().find(|r| r.name() == full) else {
                    warnings.push(format!("unknown role '{full}'"));
                    continue;
                };
                match value.as_str().map(parse_style) {
                    Some(Ok(style)) => theme.styles[*role as usize] = style,
                    Some(Err(e)) => warnings.push(format!("{full}: {e}")),
                    None => warnings.push(format!("{full}: expected a string")),
                }
            }
        }
        Ok((theme, warnings))
    }
}

/// Parses `"fg on bg modifiers..."`; every part is optional.
pub fn parse_style(text: &str) -> Result<Style, String> {
    let mut style = Style::default();
    let mut background = false;
    let mut have_fg = false;
    for word in text.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        let modifier = match lower.as_str() {
            "on" => {
                background = true;
                continue;
            }
            "bold" => Some(Modifier::BOLD),
            "dim" => Some(Modifier::DIM),
            "italic" => Some(Modifier::ITALIC),
            "underlined" | "underline" => Some(Modifier::UNDERLINED),
            "reversed" | "reverse" => Some(Modifier::REVERSED),
            _ => None,
        };
        if let Some(m) = modifier {
            style = style.add_modifier(m);
            continue;
        }
        let color = parse_color(&lower).ok_or_else(|| format!("unknown colour '{word}'"))?;
        if background {
            style = style.bg(color);
        } else if !have_fg {
            style = style.fg(color);
            have_fg = true;
        } else {
            return Err(format!(
                "two foreground colours in '{text}' (missing 'on'?)"
            ));
        }
    }
    Ok(style)
}

fn parse_color(name: &str) -> Option<Color> {
    Some(match name {
        "default" | "reset" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" | "brown" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "lightgray" | "lightgrey" | "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" | "brightred" => Color::LightRed,
        "lightgreen" | "brightgreen" => Color::LightGreen,
        "lightyellow" | "brightyellow" => Color::LightYellow,
        "lightblue" | "brightblue" => Color::LightBlue,
        "lightmagenta" | "brightmagenta" => Color::LightMagenta,
        "lightcyan" | "brightcyan" => Color::LightCyan,
        "white" => Color::White,
        hex if hex.starts_with('#') && hex.len() == 7 => {
            let v = u32::from_str_radix(&hex[1..], 16).ok()?;
            Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
        }
        n => Color::Indexed(n.parse().ok()?),
    })
}

/// Built-in schemes, embedded at compile time.
const BUILT_IN: &[(&str, &str)] = &[
    ("Classic", include_str!("../../../themes/classic.toml")),
    ("Dark", include_str!("../../../themes/dark.toml")),
    ("Light", include_str!("../../../themes/light.toml")),
    (
        "Terminal default",
        include_str!("../../../themes/terminal.toml"),
    ),
    // 24-bit colour schemes (need a true colour terminal)
    (
        "Tokyo Night",
        include_str!("../../../themes/tokyo_night.toml"),
    ),
    ("Dracula", include_str!("../../../themes/dracula.toml")),
];

/// All available schemes: built-ins, then user schemes (a user scheme with the name of
/// a built-in replaces it).
#[derive(Debug, Clone)]
pub struct ThemeSet {
    themes: Vec<Theme>,
}

impl ThemeSet {
    /// Built-in schemes only.
    pub fn built_in() -> Self {
        let (classic, _) =
            Theme::parse(BUILT_IN[0].1, None, BUILT_IN[0].0).expect("built-in scheme parses");
        let mut themes = vec![classic.clone()];
        for (name, text) in &BUILT_IN[1..] {
            let (t, _) = Theme::parse(text, Some(&classic), name).expect("built-in scheme parses");
            themes.push(t);
        }
        Self { themes }
    }

    /// Built-ins plus every `*.toml` in `dir`. Returns warnings (logged by the caller).
    pub fn load(dir: &Path) -> (Self, Vec<String>) {
        let mut set = Self::built_in();
        let mut warnings = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (set, warnings),
            Err(e) => {
                warnings.push(format!("{}: {e}", dir.display()));
                return (set, warnings);
            }
        };
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("toml"))
            })
            .collect();
        files.sort();
        for path in files {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            match Theme::parse(&text, Some(&set.themes[0]), &stem) {
                Ok((theme, w)) => {
                    warnings.extend(w.into_iter().map(|w| format!("{}: {w}", path.display())));
                    set.insert(theme);
                }
                Err(e) => warnings.push(format!("{}: {e}", path.display())),
            }
        }
        (set, warnings)
    }

    fn insert(&mut self, theme: Theme) {
        match self
            .themes
            .iter_mut()
            .find(|t| t.name.eq_ignore_ascii_case(&theme.name))
        {
            Some(existing) => *existing = theme,
            None => self.themes.push(theme),
        }
    }

    pub fn names(&self) -> Vec<String> {
        self.themes.iter().map(|t| t.name.clone()).collect()
    }

    /// Index of the scheme called `name` (case-insensitive).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.themes
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Scheme called `name`, or Classic if there is none.
    pub fn get(&self, name: &str) -> &Theme {
        &self.themes[self.index_of(name).unwrap_or(0)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_parse_without_warnings_and_classic_is_complete() {
        let (classic, w) = Theme::parse(BUILT_IN[0].1, None, "x").unwrap();
        assert!(w.is_empty(), "{w:?}");
        let missing: Vec<_> = Role::ALL
            .iter()
            .filter(|r| classic.get(**r) == Style::default())
            .map(|r| r.name())
            .collect();
        assert!(missing.is_empty(), "Classic misses {missing:?}");
        for (name, text) in BUILT_IN {
            let (t, w) = Theme::parse(text, Some(&classic), name).unwrap();
            assert!(w.is_empty(), "{name}: {w:?}");
            assert_eq!(&t.name, name);
        }
    }

    #[test]
    fn true_colour_schemes_define_every_role() {
        for (name, text) in &BUILT_IN[4..] {
            let (t, w) = Theme::parse(text, None, name).unwrap();
            assert!(w.is_empty(), "{name}: {w:?}");
            let missing: Vec<_> = Role::ALL
                .iter()
                .filter(|r| t.get(**r) == Style::default())
                .map(|r| r.name())
                .collect();
            assert!(missing.is_empty(), "{name} misses {missing:?}");
        }
    }

    /// Every built-in scheme is legible and its highlights / hotkeys are visible on
    /// common terminal palettes (see `theme_check`).
    #[test]
    fn every_scheme_is_readable() {
        use crate::theme_check::{PALETTES, problems};
        let set = ThemeSet::built_in();
        let mut report = Vec::new();
        for name in set.names() {
            let theme = set.get(&name);
            let true_colour = BUILT_IN[4..].iter().any(|(n, _)| *n == name);
            for p in PALETTES {
                // 24-bit schemes look the same everywhere: one palette is enough
                if true_colour && p.name != "Campbell" {
                    continue;
                }
                for problem in problems(theme, p) {
                    report.push(format!("{name} [{}]: {problem}", p.name));
                }
            }
        }
        assert!(
            report.is_empty(),
            "
{}",
            report.join(
                "
"
            )
        );
    }

    #[test]
    fn style_strings() {
        assert_eq!(
            parse_style("lightgray on blue").unwrap(),
            Style::default().fg(Color::Gray).bg(Color::Blue)
        );
        assert_eq!(
            parse_style("yellow bold").unwrap(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(
            parse_style("on #102030").unwrap(),
            Style::default().bg(Color::Rgb(0x10, 0x20, 0x30))
        );
        assert_eq!(
            parse_style("reversed").unwrap(),
            Style::default().add_modifier(Modifier::REVERSED)
        );
        assert!(parse_style("pink on blue").is_err());
        assert!(parse_style("red blue").is_err());
    }

    #[test]
    fn user_scheme_overrides_and_falls_back() {
        let set = ThemeSet::built_in();
        let classic = set.get("Classic");
        let (t, w) = Theme::parse(
            "[core]\nnormal = \"white on black\"\n",
            Some(classic),
            "mine",
        )
        .unwrap();
        assert!(w.is_empty());
        assert_eq!(t.name, "mine");
        assert_eq!(
            t.get(Role::CoreNormal),
            parse_style("white on black").unwrap()
        );
        assert_eq!(t.get(Role::MenuNormal), classic.get(Role::MenuNormal));
        let (_, w) = Theme::parse("[core]\nnope = \"red\"\n", Some(classic), "x").unwrap();
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn unknown_name_falls_back_to_classic() {
        let set = ThemeSet::built_in();
        assert_eq!(set.get("does not exist").name, "Classic");
        assert_eq!(set.get("dark").name, "Dark");
        assert_eq!(
            set.names(),
            vec![
                "Classic",
                "Dark",
                "Light",
                "Terminal default",
                "Tokyo Night",
                "Dracula"
            ]
        );
    }
}
