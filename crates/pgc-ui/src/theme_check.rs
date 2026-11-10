//! Readability check of colour schemes: every role must be legible, and selected /
//! focused / hotkey roles must stand out from what surrounds them. Used by the tests
//! for all built-in schemes, on several terminal palettes.

use ratatui::style::{Color, Modifier, Style};

use crate::theme::{Role, Theme};

type Rgb = (u8, u8, u8);

/// A terminal's 16 colours plus its default foreground / background.
pub struct Palette {
    pub name: &'static str,
    pub ansi: [Rgb; 16],
    pub default_fg: Rgb,
    pub default_bg: Rgb,
}

const fn hex(v: u32) -> Rgb {
    ((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Windows Terminal "Campbell", xterm, VGA / Linux console (mc's classic look); dark
/// and light default background for schemes that use the terminal's own colours.
pub const PALETTES: &[Palette] = &[
    Palette {
        name: "Campbell",
        ansi: [
            hex(0x0C0C0C),
            hex(0xC50F1F),
            hex(0x13A10E),
            hex(0xC19C00),
            hex(0x0037DA),
            hex(0x881798),
            hex(0x3A96DD),
            hex(0xCCCCCC),
            hex(0x767676),
            hex(0xE74856),
            hex(0x16C60C),
            hex(0xF9F1A5),
            hex(0x3B78FF),
            hex(0xB4009E),
            hex(0x61D6D6),
            hex(0xF2F2F2),
        ],
        default_fg: hex(0xCCCCCC),
        default_bg: hex(0x0C0C0C),
    },
    Palette {
        name: "xterm",
        ansi: [
            hex(0x000000),
            hex(0xCD0000),
            hex(0x00CD00),
            hex(0xCDCD00),
            hex(0x0000EE),
            hex(0xCD00CD),
            hex(0x00CDCD),
            hex(0xE5E5E5),
            hex(0x7F7F7F),
            hex(0xFF0000),
            hex(0x00FF00),
            hex(0xFFFF00),
            hex(0x5C5CFF),
            hex(0xFF00FF),
            hex(0x00FFFF),
            hex(0xFFFFFF),
        ],
        default_fg: hex(0x000000),
        default_bg: hex(0xFFFFFF),
    },
    Palette {
        name: "VGA",
        ansi: [
            hex(0x000000),
            hex(0xAA0000),
            hex(0x00AA00),
            hex(0xAA5500),
            hex(0x0000AA),
            hex(0xAA00AA),
            hex(0x00AAAA),
            hex(0xAAAAAA),
            hex(0x555555),
            hex(0xFF5555),
            hex(0x55FF55),
            hex(0xFFFF55),
            hex(0x5555FF),
            hex(0xFF55FF),
            hex(0x55FFFF),
            hex(0xFFFFFF),
        ],
        default_fg: hex(0xAAAAAA),
        default_bg: hex(0x000000),
    },
];

fn ansi_index(c: Color) -> Option<usize> {
    Some(match c {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Indexed(i) if i < 16 => i as usize,
        _ => return None,
    })
}

fn rgb(c: Option<Color>, default: Rgb, p: &Palette) -> Rgb {
    match c {
        None | Some(Color::Reset) => default,
        Some(Color::Rgb(r, g, b)) => (r, g, b),
        Some(c) => match ansi_index(c) {
            Some(i) => p.ansi[i],
            None => default,
        },
    }
}

/// What the terminal shows for `style`: (foreground, background).
pub fn shown(style: Style, p: &Palette) -> (Rgb, Rgb) {
    let mut fg = rgb(style.fg, p.default_fg, p);
    let mut bg = rgb(style.bg, p.default_bg, p);
    if style.add_modifier.contains(Modifier::REVERSED) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if style.add_modifier.contains(Modifier::DIM) {
        // most terminals blend dim text halfway to the background
        fg = (
            ((fg.0 as u16 + bg.0 as u16) / 2) as u8,
            ((fg.1 as u16 + bg.1 as u16) / 2) as u8,
            ((fg.2 as u16 + bg.2 as u16) / 2) as u8,
        );
    }
    (fg, bg)
}

fn luminance(c: Rgb) -> f64 {
    let ch = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.0) + 0.7152 * ch(c.1) + 0.0722 * ch(c.2)
}

/// WCAG contrast ratio (1 … 21).
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Two colours look clearly different (by brightness or by hue).
fn distinct(a: Rgb, b: Rgb) -> bool {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
    let dist = ((d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)) as f64).sqrt();
    contrast(a, b) >= 1.4 || dist >= 120.0
}

use Role as R;

/// Text roles: contrast ≥ 3 (WCAG for large / bold text and UI components).
const READABLE: &[Role] = &[
    R::CoreNormal,
    R::CoreTitle,
    R::CoreTitleFocused,
    R::CoreSelected,
    R::CoreSelectedUnfocused,
    R::MenuNormal,
    R::MenuHotkey,
    R::MenuSelected,
    R::MenuSelectedHotkey,
    R::DialogNormal,
    R::DialogFocus,
    R::DialogHotkey,
    R::DialogFocusHotkey,
    R::DialogTitle,
    R::DialogInput,
    R::DialogInputFocus,
    R::ErrorNormal,
    R::ErrorTitle,
    R::TabsBar,
    R::TabsActive,
    R::TabsInactive,
    R::StatusNormal,
    R::FkeyNumber,
    R::FkeyLabel,
    R::EditorText,
    R::EditorKeyword,
    R::EditorString,
    R::EditorNumber,
    R::EditorQuotedIdent,
    R::EditorOperator,
    R::EditorSelection,
    R::GridHeader,
    R::GridCursor,
    R::MessagesError,
    R::MessagesNotice,
    R::MessagesInfo,
];

/// Deliberately quieter roles: contrast ≥ 2.
const QUIET: &[Role] = &[
    R::CoreHint,
    R::CoreBorder,
    R::MenuDisabled,
    R::DialogDisabled,
    R::EditorComment,
    R::EditorLineNumber,
    R::GridNull,
    R::GridRowNumber,
];

/// (highlight, surrounding): the highlight's background must differ visibly.
const HIGHLIGHTS: &[(Role, Role)] = &[
    (R::MenuSelected, R::MenuNormal),
    (R::DialogFocus, R::DialogNormal),
    (R::CoreSelected, R::CoreNormal),
    (R::CoreSelectedUnfocused, R::CoreNormal),
    (R::CoreTitleFocused, R::CoreTitle),
    (R::GridCursor, R::CoreNormal),
    (R::GridHeader, R::CoreNormal),
    (R::EditorSelection, R::EditorText),
    (R::TabsActive, R::TabsInactive),
    (R::DialogInput, R::DialogNormal),
    (R::DialogInputFocus, R::DialogNormal),
    // selected item of a list / open combo list, drawn on the input colours
    (R::DialogFocus, R::DialogInput),
    (R::CoreSelectedUnfocused, R::DialogInput),
    (R::FkeyLabel, R::FkeyNumber),
    (R::StatusNormal, R::CoreNormal),
    (R::MenuNormal, R::CoreNormal),
];

/// (hotkey, text around it): the hotkey letter must stand out (other colour, or
/// underlined / bold where the text is not).
const HOTKEYS: &[(Role, Role)] = &[
    (R::MenuHotkey, R::MenuNormal),
    (R::MenuSelectedHotkey, R::MenuSelected),
    (R::DialogHotkey, R::DialogNormal),
    (R::DialogFocusHotkey, R::DialogFocus),
];

/// Problems of `theme` on palette `p`, as readable sentences.
pub fn problems(theme: &Theme, p: &Palette) -> Vec<String> {
    let mut out = Vec::new();
    let name = |r: Role| r.name();
    for (roles, min) in [(READABLE, 3.0), (QUIET, 2.0)] {
        for &r in roles {
            let (fg, bg) = shown(theme.get(r), p);
            let c = contrast(fg, bg);
            if c < min {
                out.push(format!("{}: text contrast {c:.1} < {min}", name(r)));
            }
        }
    }
    for &(hi, around) in HIGHLIGHTS {
        let (_, a) = shown(theme.get(hi), p);
        let (_, b) = shown(theme.get(around), p);
        if !distinct(a, b) {
            out.push(format!(
                "{} does not stand out from {} (backgrounds too similar)",
                name(hi),
                name(around)
            ));
        }
    }
    for &(hot, around) in HOTKEYS {
        let hs = theme.get(hot);
        let ts = theme.get(around);
        let (hf, hb) = shown(hs, p);
        let (tf, tb) = shown(ts, p);
        let marked = |s: Style| {
            s.add_modifier
                .intersects(Modifier::UNDERLINED | Modifier::BOLD)
        };
        let by_colour = hb == tb && distinct(hf, tf);
        let by_mark = marked(hs) && !marked(ts);
        if !(by_colour || by_mark) {
            out.push(format!(
                "{} is not visible inside {}",
                name(hot),
                name(around)
            ));
        }
    }
    out
}
