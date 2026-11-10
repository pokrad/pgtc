//! Clipboard (D-023): internal buffer always; system clipboard (`arboard`) when
//! available; OSC 52 over SSH or when there is no system clipboard.

use pgc_core::settings::ClipboardSettings;

pub struct Clipboard {
    internal: String,
    /// One handle for the whole session (required on X11 / Wayland).
    system: Option<arboard::Clipboard>,
    osc52: bool,
    ssh: bool,
}

impl Clipboard {
    pub fn new(settings: &ClipboardSettings) -> Self {
        let system = if settings.system {
            match arboard::Clipboard::new() {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::info!("system clipboard not available: {e}");
                    None
                }
            }
        } else {
            None
        };
        let ssh =
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
        Self {
            internal: String::new(),
            system,
            osc52: settings.osc52,
            ssh,
        }
    }

    /// New settings from the Settings dialog (D-051); the internal buffer is kept.
    pub fn apply(&mut self, settings: &ClipboardSettings) {
        let internal = std::mem::take(&mut self.internal);
        *self = Self::new(settings);
        self.internal = internal;
    }

    /// Copies `text`. Returns true if it must also be sent to the terminal via OSC 52.
    pub fn copy(&mut self, text: &str) -> bool {
        self.internal = text.to_owned();
        let system_ok = self
            .system
            .as_mut()
            .is_some_and(|c| c.set_text(text.to_owned()).is_ok());
        self.osc52 && (self.ssh || !system_ok)
    }

    /// Text to paste: the system clipboard first, else the internal buffer.
    pub fn paste(&mut self) -> String {
        if let Some(c) = self.system.as_mut()
            && let Ok(text) = c.get_text()
            && !text.is_empty()
        {
            return text;
        }
        self.internal.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_buffer_and_osc52_without_system_clipboard() {
        let mut c = Clipboard::new(&ClipboardSettings {
            system: false,
            osc52: true,
        });
        assert!(c.copy("abc"), "no system clipboard: OSC 52");
        assert_eq!(c.paste(), "abc");
        let mut c = Clipboard::new(&ClipboardSettings {
            system: false,
            osc52: false,
        });
        assert!(!c.copy("x"));
        assert_eq!(c.paste(), "x");
    }
}
