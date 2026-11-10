//! Key combinations: normalisation of terminal key events, parsing and display of
//! key names used in `keymap.toml` (e.g. `Ctrl+R`, `Alt+Shift+C`, `F5`, `Alt+.`).

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A normalised key combination.
///
/// Letters are stored lower-case with `SHIFT` for upper case; for other characters
/// `SHIFT` is dropped (it is already part of the character, e.g. `?`).
/// `BackTab` becomes `Shift+Tab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyCombo {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mut mods = mods & (KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT);
        let code = match code {
            KeyCode::BackTab => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            KeyCode::Char(c) if c.is_alphabetic() => {
                if c.is_uppercase() {
                    mods |= KeyModifiers::SHIFT;
                }
                KeyCode::Char(c.to_lowercase().next().unwrap_or(c))
            }
            KeyCode::Char(c) => {
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::Char(c)
            }
            other => other,
        };
        Self { code, mods }
    }

    pub fn plain(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }

    pub fn from_event(ev: &KeyEvent) -> Self {
        Self::new(ev.code, ev.modifiers)
    }

    /// Parses `Ctrl+Shift+F5`, `Alt+.`, `Esc`, `Ctrl++` ... (case-insensitive).
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (mods_part, key_part) = if let Some(prefix) = text.strip_suffix("++") {
            (prefix, "+")
        } else {
            match text.rsplit_once('+') {
                Some((m, k)) => (m, k),
                None => ("", text),
            }
        };
        let mut mods = KeyModifiers::NONE;
        for m in mods_part.split('+').filter(|m| !m.is_empty()) {
            mods |= match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return Err(format!("unknown modifier '{m}' in '{text}'")),
            };
        }
        let lower = key_part.to_ascii_lowercase();
        let code = match lower.as_str() {
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "enter" | "return" => KeyCode::Enter,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "space" => KeyCode::Char(' '),
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                let n: u8 = f[1..].parse().unwrap_or(0);
                if !(1..=24).contains(&n) {
                    return Err(format!("unknown key '{key_part}' in '{text}'"));
                }
                KeyCode::F(n)
            }
            _ => {
                let mut chars = key_part.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => KeyCode::Char(c.to_ascii_lowercase()),
                    _ => return Err(format!("unknown key '{key_part}' in '{text}'")),
                }
            }
        };
        Ok(Self::new(code, mods))
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            f.write_str("Ctrl+")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            f.write_str("Alt+")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            f.write_str("Shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            KeyCode::Char(c) => write!(f, "{}", c.to_uppercase()),
            KeyCode::F(n) => write!(f, "F{n}"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::Enter => f.write_str("Enter"),
            KeyCode::Backspace => f.write_str("Backspace"),
            KeyCode::Delete => f.write_str("Delete"),
            KeyCode::Insert => f.write_str("Insert"),
            KeyCode::Home => f.write_str("Home"),
            KeyCode::End => f.write_str("End"),
            KeyCode::PageUp => f.write_str("PgUp"),
            KeyCode::PageDown => f.write_str("PgDn"),
            KeyCode::Up => f.write_str("Up"),
            KeyCode::Down => f.write_str("Down"),
            KeyCode::Left => f.write_str("Left"),
            KeyCode::Right => f.write_str("Right"),
            other => write!(f, "{other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> KeyCombo {
        KeyCombo::parse(s).unwrap()
    }

    #[test]
    fn parse_and_display() {
        for s in [
            "Ctrl+R",
            "Alt+Shift+C",
            "F5",
            "Alt+.",
            "Esc",
            "Shift+Tab",
            "Ctrl+Insert",
            "Alt+Right",
            "Ctrl+W",
            "Space",
            "Ctrl++",
        ] {
            assert_eq!(k(s).to_string(), s, "{s}");
        }
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(k("ctrl+r"), k("Ctrl+R"));
        assert_eq!(k("ALT+SHIFT+c"), k("Alt+Shift+C"));
        assert_eq!(k("pgdn"), k("PageDown"));
    }

    #[test]
    fn events_are_normalised() {
        let ev = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(KeyCombo::from_event(&ev), k("Alt+Shift+C"));
        let ev = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::ALT);
        assert_eq!(KeyCombo::from_event(&ev), k("Alt+Shift+C"));
        let ev = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(KeyCombo::from_event(&ev), k("Shift+Tab"));
        let ev = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        assert_eq!(
            KeyCombo::from_event(&ev),
            KeyCombo::plain(KeyCode::Char('?'))
        );
    }

    #[test]
    fn bad_keys_are_errors() {
        assert!(KeyCombo::parse("Hyper+X").is_err());
        assert!(KeyCombo::parse("Ctrl+Foo").is_err());
        assert!(KeyCombo::parse("F99").is_err());
    }
}
