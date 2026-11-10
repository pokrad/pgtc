//! The Settings dialog (D-051), saving `settings.toml`, and the messages of the
//! setup steps and the templates folders (D-047).

use std::path::Path;

use super::App;
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::setup;
use crate::theme::Theme;

impl App {
    /// Show the outcome of installation steps (D-047) in one message.
    pub(super) fn show_steps(&mut self, title: &str, steps: Vec<setup::StepOutcome>) {
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
    pub(super) fn show_templates_folder(&mut self) {
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

    /// Settings dialog (D-051): one column, groups separated by an empty row.
    pub(super) fn open_settings(&mut self) {
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

    pub(super) fn apply_settings(&mut self, dialog: Dialog) {
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
    pub(super) fn save_settings(&mut self) -> bool {
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
}
