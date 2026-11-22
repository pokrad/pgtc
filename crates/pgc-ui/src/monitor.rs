//! What the dashboard, profiler and database statistics tabs have in common (D-057,
//! D-058, D-062): their own connection, one list of rows and a status text. The
//! handlers that work on these alone are in `app/monitor.rs`.

use pgc_core::ServerVersion;

use crate::dashboard::DashConn;
use crate::grid::ResultSet;

/// The own connection of a dashboard, profiler or statistics tab: where it goes and
/// what is known of it.
#[derive(Debug)]
pub struct OwnConn {
    /// The server (its uid) and the database the connection goes to.
    pub uid: u64,
    pub database: String,
    pub conn: Option<DashConn>,
    pub version: Option<ServerVersion>,
    /// Why the tab has no connection (failed / lost).
    pub problem: Option<String>,
}

impl OwnConn {
    pub fn new(uid: u64, database: String) -> Self {
        Self {
            uid,
            database,
            conn: None,
            version: None,
            problem: None,
        }
    }

    pub fn connected(&self) -> bool {
        self.conn.as_ref().is_some_and(|c| c.connected)
    }

    /// What a tab without a connection says instead of its content, `None` when it
    /// is connected: why there is none (the problem, connecting, not connected) and
    /// what `F5` does about it. `one_line`: for a tab that has one row for it (the
    /// profiler's state row); else the problem stands alone, the hint below it.
    pub fn offline_text(&self, one_line: bool) -> Option<String> {
        if self.connected() {
            return None;
        }
        Some(match (&self.problem, &self.conn) {
            (Some(problem), _) if one_line => format!("{problem} — F5 connects again."),
            (Some(problem), _) => format!("{problem}\n\nF5 connects again."),
            (None, Some(_)) => "Connecting…".to_owned(),
            (None, None) => "Not connected — F5 connects.".to_owned(),
        })
    }
}

pub trait MonitorTab {
    /// The tab's own connection.
    fn own(&self) -> &OwnConn;

    fn own_mut(&mut self) -> &mut OwnConn;

    /// Where the connection goes: server uid, database, and its state (`None`
    /// without a connection, `Some(false)` while it is being opened).
    fn place(&self) -> (u64, &str, Option<bool>) {
        let own = self.own();
        (
            own.uid,
            &own.database,
            own.conn.as_ref().map(|c| c.connected),
        )
    }

    /// The list that is shown.
    fn list(&self) -> &ResultSet;

    fn list_mut(&mut self) -> &mut ResultSet;

    /// What the tab is doing: the right side of its status row (D-043).
    fn status_text(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::OwnConn;

    #[test]
    fn a_tab_without_a_connection_says_why() {
        let mut own = OwnConn::new(1, "shop".into());
        assert_eq!(
            own.offline_text(false).as_deref(),
            Some("Not connected — F5 connects.")
        );
        own.problem = Some("Session lost: reset".into());
        assert_eq!(
            own.offline_text(false).as_deref(),
            Some("Session lost: reset\n\nF5 connects again.")
        );
        assert_eq!(
            own.offline_text(true).as_deref(),
            Some("Session lost: reset — F5 connects again.")
        );
    }
}
