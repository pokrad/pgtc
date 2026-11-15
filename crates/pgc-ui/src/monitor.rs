//! What the dashboard, profiler and database statistics tabs have in common (D-057,
//! D-058, D-062): their own connection, one list of rows and a status text. The
//! handlers that work on these alone are in `app/monitor.rs`.

use crate::dashboard::OwnConn;
use crate::grid::ResultSet;

pub trait MonitorTab {
    /// The tab's own connection.
    fn own_conn(&mut self) -> OwnConn<'_>;

    /// Where the connection goes: server uid, database, and its state (`None`
    /// without a connection, `Some(false)` while it is being opened).
    fn place(&self) -> (u64, &str, Option<bool>);

    /// The list that is shown.
    fn list(&self) -> &ResultSet;

    fn list_mut(&mut self) -> &mut ResultSet;

    /// What the tab is doing: the right side of its status row (D-043).
    fn status_text(&self) -> String;
}
