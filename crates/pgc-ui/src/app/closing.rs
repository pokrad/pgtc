//! Closing tabs, quitting, disconnecting and deleting a server (D-019, D-043): the
//! questions about unsaved text and open transactions.

use pgc_db::{ConnectionHandle, TxStatus};

use super::state::TabWait;
use super::{App, Focus, Tab, TabBody};
use crate::dialog::{Button, Dialog, DialogKind};
use crate::query_tab::QueryConn;

/// Closing many tabs at once (D-043).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Bulk {
    Quit,
    /// Disconnect the server with this uid.
    Disconnect(u64),
    /// Delete the connection with this uid: as disconnecting, then it leaves the tree.
    Delete(u64),
    /// The connection with this uid was edited (D-035): as disconnecting, then it is
    /// connected again with the new settings.
    Reconnect(u64),
}

impl App {
    /// Closes a tab after asking about unsaved text (`ask_save`) and an open
    /// transaction (D-019).
    pub(super) fn request_close_tab(&mut self, id: u64, ask_save: bool) {
        let Some(i) = self.tab_index(id) else {
            return;
        };
        let tab = &self.tabs[i];
        let asks = tab.query().is_some() || tab.template().is_some();
        if asks && ask_save && self.settings.query.ask_save && tab.modified() {
            let text = format!("{} has unsaved changes. Save them?", tab.label());
            let title = if tab.template().is_some() {
                "Close template tab"
            } else {
                "Close query tab"
            };
            self.dialogs.push(Dialog::confirm_buttons(
                DialogKind::SaveChanges { tab: id },
                title,
                &text,
                &[
                    (Button::Yes, "&Yes"),
                    (Button::No, "&No"),
                    (Button::Cancel, "&Cancel"),
                ],
            ));
        } else if tab
            .query()
            .is_some_and(|q| q.tx != TxStatus::Idle && q.connected())
        {
            self.dialogs.push(Dialog::confirm_buttons(
                DialogKind::CloseTransaction { tab: id },
                "Close query tab",
                "This query tab has an open transaction.",
                &[
                    (Button::Commit, "Co&mmit"),
                    (Button::Rollback, "&Rollback"),
                    (Button::Cancel, "&Cancel"),
                ],
            ));
        } else {
            self.close_tab(id);
        }
    }

    /// "Save changes?" → Yes: save to the tab's file or ask for one. Saved, the tab
    /// closes — or, `bulk`, closing many tabs goes on.
    pub(super) fn save_before(&mut self, id: u64, bulk: bool) {
        let Some(i) = self.tab_index(id) else {
            return;
        };
        self.active_tab = i;
        if bulk {
            self.closing.bulk_saving(id);
        } else {
            self.closing.wait(id, TabWait::Save);
        }
        // a template tab has its own way of saving (D-074)
        if self.tabs[i].template().is_some() {
            self.save_template(id, false);
            return;
        }
        let file = self.tabs[i].query().and_then(|q| q.file.clone());
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
        self.closing.forget_tab(tab.id);
        self.pending_defs.retain(|_, t| *t != tab.id);
        self.template_lookups.retain(|_, t| t.tab() != Some(tab.id));
        let conn = match tab.body {
            TabBody::Default | TabBody::Object | TabBody::Template(_) => None,
            // closing the connection rolls back an open transaction (D-019)
            TabBody::Query(_, q) => {
                let busy = q.busy();
                q.conn.map(|c| (c.id, end_query_conn(busy, c)))
            }
            TabBody::Dashboard(d) => d.conn.map(|c| (c.id, c.handle)),
            TabBody::DbStats(s) => s.conn.map(|c| (c.id, c.handle)),
            // changed logging settings are put back first (D-058)
            TabBody::Profiler(p) => {
                self.close_profiler_conn(*p);
                None
            }
        };
        // ended properly, without waiting for it (D-071)
        if let Some((id, handle)) = conn {
            handle.end(pgc_db::END_GRACE);
            self.connections.remove(&id);
        }
        // a tab left of the active one: the active tab has moved one place
        if i < self.active_tab {
            self.active_tab -= 1;
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
                t.query()
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
            Bulk::Disconnect(uid) | Bulk::Delete(uid) | Bulk::Reconnect(uid) => {
                t.server == Some(uid)
                    || t.query()
                        .and_then(|q| q.binding.as_ref())
                        .is_some_and(|(s, _)| *s == uid)
            }
        }
    }

    /// Quitting or disconnecting a server: asks about unsaved query text, one tab at
    /// a time, then about open transactions; `Cancel` stops (D-019, D-043).
    pub(super) fn request_bulk(&mut self, b: Bulk) {
        let skip = self.closing.ask(b).skip.clone();
        let title = match b {
            Bulk::Quit => "Quit",
            Bulk::Disconnect(_) => "Disconnect",
            Bulk::Delete(_) => "Delete connection",
            Bulk::Reconnect(_) => "Edit connection",
        };
        if self.settings.query.ask_save
            && let Some(i) = self
                .tabs
                .iter()
                .position(|t| Self::in_bulk(b, t) && t.modified() && !skip.contains(&t.id))
        {
            self.active_tab = i;
            let tab = &self.tabs[i];
            let text = format!("{} has unsaved changes. Save them?", tab.label());
            let id = tab.id;
            self.dialogs.push(Dialog::confirm_buttons(
                DialogKind::QuitSaveChanges { tab: id },
                title,
                &text,
                &[
                    (Button::Yes, "&Yes"),
                    (Button::No, "&No"),
                    (Button::Cancel, "&Cancel"),
                ],
            ));
            return;
        }
        let open = self.bulk_transactions(b).len();
        if open == 0 {
            self.closing.bulk = None;
            self.finish_bulk(b);
            return;
        }
        // (the question about unsaved text is answered for every tab)
        self.closing.ask(b).skip.clear();
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
                (Button::Commit, "Co&mmit all"),
                (Button::Rollback, "&Rollback all"),
                (Button::Cancel, "&Cancel"),
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
                    self.closing.quit = true;
                }
            }
            Bulk::Disconnect(uid) | Bulk::Delete(uid) | Bulk::Reconnect(uid) => {
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
                if b == Bulk::Reconnect(uid) {
                    self.connect_server(uid);
                }
            }
        }
    }

    /// Quitting (D-071): ends every open connection — the tree's, the tabs' and those
    /// of running definition jobs — and returns their handles, so the caller can give
    /// them a moment to finish.
    pub fn end_connections(&mut self) -> Vec<ConnectionHandle> {
        let mut handles = Vec::new();
        for server in &mut self.tree.servers {
            handles.extend(server.reset());
        }
        for tab in &mut self.tabs {
            let handle = match &mut tab.body {
                TabBody::Default | TabBody::Object | TabBody::Template(_) => None,
                TabBody::Query(_, q) => {
                    let busy = q.busy();
                    q.conn.take().map(|c| end_query_conn(busy, c))
                }
                TabBody::Dashboard(d) => d.conn.take().map(|c| c.handle),
                TabBody::DbStats(s) => s.conn.take().map(|c| c.handle),
                TabBody::Profiler(p) => p.conn.take().map(|c| c.handle),
            };
            handles.extend(handle);
        }
        handles.extend(self.jobs.drain().map(|(_, job)| job.handle));
        self.connections.clear();
        for handle in &handles {
            handle.end(pgc_db::END_GRACE);
        }
        handles
    }

    /// "Commit all" of closing many tabs: a COMMIT in every tab with an open
    /// transaction; all of them answered, the tabs close.
    pub(super) fn commit_bulk(&mut self) {
        let Some(b) = self.closing.bulk.as_ref().map(|b| b.what) else {
            return;
        };
        let sent: Vec<u64> = self
            .bulk_transactions(b)
            .into_iter()
            .filter(|id| self.send_commit(*id))
            .collect();
        if self.closing.commit(sent).is_some() {
            self.maybe_finish_bulk();
        }
    }

    /// Finishes quitting / disconnecting once every COMMIT sent for it has finished
    /// (and no tab of it runs anything else).
    pub(super) fn maybe_finish_bulk(&mut self) {
        let Some(b) = self.closing.committing() else {
            return;
        };
        let waiting = self.tabs.iter().any(|t| {
            Self::in_bulk(b, t)
                && (self.closing.commit_awaited(t.id) || t.query().is_some_and(|q| q.busy()))
        });
        if !waiting {
            self.closing.bulk = None;
            self.finish_bulk(b);
        }
    }
}

/// The handle of a query tab's connection that is about to be ended: a statement that
/// is still running (`busy`) is cancelled first, so the connection gets free (D-071).
fn end_query_conn(busy: bool, conn: QueryConn) -> ConnectionHandle {
    if busy && let Some(cancel) = &conn.cancel {
        pgc_db::cancel_query(cancel, conn.ssl_mode, conn.ssl_root_cert.clone());
    }
    conn.handle
}
