//! Closing tabs, quitting, disconnecting and deleting a server (D-019, D-043): the
//! questions about unsaved text and open transactions.

use pgc_db::TxStatus;

use super::{App, Focus, Tab};
use crate::dialog::{Dialog, DialogKind};
use crate::query_tab::AfterRun;

/// What follows a save asked for by "Save changes?" (D-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AfterSave {
    Close(u64),
    /// Continue quitting / disconnecting.
    Bulk,
}

/// Closing many tabs at once (D-043).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Bulk {
    Quit,
    /// Disconnect the server with this uid.
    Disconnect(u64),
    /// Delete the connection with this uid: as disconnecting, then it leaves the tree.
    Delete(u64),
}

impl App {
    /// Closes a tab after asking about unsaved text (`ask_save`) and an open
    /// transaction (D-019).
    pub(super) fn request_close_tab(&mut self, id: u64, ask_save: bool) {
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
    pub(super) fn save_before(&mut self, id: u64, then: AfterSave) {
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

    pub(super) fn close_tab(&mut self, id: u64) {
        let Some(i) = self.tab_index(id) else {
            return;
        };
        if i == 0 {
            return;
        }
        let tab = self.tabs.remove(i);
        self.pending_defs.retain(|_, t| *t != tab.id);
        self.template_lookups.retain(|_, t| t.tab() != Some(tab.id));
        if let Some(conn) = tab.query.and_then(|q| q.conn) {
            // dropping the connection rolls back an open transaction (D-019)
            conn.handle.abort();
            self.connections.remove(&conn.id);
        }
        if let Some(conn) = tab.dashboard.and_then(|d| d.conn) {
            conn.handle.abort();
            self.connections.remove(&conn.id);
        }
        if let Some(conn) = tab.db_stats.and_then(|s| s.conn) {
            conn.handle.abort();
            self.connections.remove(&conn.id);
        }
        // changed logging settings are put back first (D-058)
        if let Some(p) = tab.profiler {
            self.close_profiler_conn(p);
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
    pub(super) fn request_quit(&mut self) {
        self.request_bulk(Bulk::Quit);
    }

    /// The tab belongs to what is being quit / disconnected.
    fn in_bulk(b: Bulk, t: &Tab) -> bool {
        match b {
            Bulk::Quit => true,
            Bulk::Disconnect(uid) | Bulk::Delete(uid) => {
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
    pub(super) fn request_bulk(&mut self, b: Bulk) {
        let skip = match &self.bulk {
            Some((current, skip)) if *current == b => skip.clone(),
            _ => Vec::new(),
        };
        self.bulk = Some((b, skip.clone()));
        let title = match b {
            Bulk::Quit => "Quit",
            Bulk::Disconnect(_) => "Disconnect",
            Bulk::Delete(_) => "Delete connection",
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
    pub(super) fn bulk_transactions(&self, b: Bulk) -> Vec<u64> {
        let ids = self.tabs_with_transaction();
        self.tabs
            .iter()
            .filter(|t| ids.contains(&t.id) && Self::in_bulk(b, t))
            .map(|t| t.id)
            .collect()
    }

    /// Quits, or closes every tab of the server and its connections (D-043); a
    /// deleted server then leaves the tree.
    pub(super) fn finish_bulk(&mut self, b: Bulk) {
        match b {
            // profilers put the logging settings back first (D-058)
            Bulk::Quit => {
                if !self.restore_before_quit() {
                    self.quit = true;
                }
            }
            Bulk::Disconnect(uid) | Bulk::Delete(uid) => {
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
                if b == Bulk::Delete(uid) {
                    self.delete_server(uid);
                }
            }
        }
    }

    /// Finishes quitting / disconnecting once every COMMIT sent for it has finished.
    pub(super) fn maybe_finish_bulk(&mut self) {
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
}
