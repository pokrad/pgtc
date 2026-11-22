//! Server dashboard tab (D-057): its state (graph samples and rates, the summary strip,
//! the lists, tagged sessions) and its drawing. The app side (connection, refresh,
//! commands) is in `app/dashboard.rs`.

use std::collections::{BTreeSet, HashSet, VecDeque};

use pgc_core::ServerVersion;
use pgc_db::{Column, ConnId, ConnectionHandle};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Sparkline};

use crate::grid::{GridStyles, ResultSet, thousands};
use crate::theme::{Role, Theme};

/// Refresh intervals in milliseconds, cycled by `dashboard_interval`; 2 s is the
/// default (D-057).
pub const INTERVALS_MS: [u64; 6] = [500, 1000, 2000, 5000, 10_000, 30_000];
const DEFAULT_INTERVAL: usize = 2;
/// Samples kept per graph (more than any terminal is wide).
const HISTORY: usize = 400;
/// The database size is read at most this often (seconds, server clock).
const SIZE_EVERY: f64 = 30.0;
/// Durations from these (seconds) are `dashboard.slow` / `dashboard.very_slow`.
const SLOW: f64 = 1.0;
const VERY_SLOW: f64 = 10.0;
/// Lines of the summary strip.
const STRIP_LINES: u16 = 4;

/// `0.5 s`, `2 s`
pub fn interval_text(ms: u64) -> String {
    if ms % 1000 == 0 {
        format!("{} s", ms / 1000)
    } else {
        format!("{:.1} s", ms as f64 / 1000.0)
    }
}

/// The lists below the graphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    Sessions,
    Blocking,
    Progress,
    Locks,
    Prepared,
    Configuration,
}

/// (column name, numeric = right-aligned)
type Col = (&'static str, bool);

impl List {
    pub const ALL: [List; 6] = [
        List::Sessions,
        List::Blocking,
        List::Progress,
        List::Locks,
        List::Prepared,
        List::Configuration,
    ];

    pub fn title(self) -> &'static str {
        match self {
            List::Sessions => "Sessions",
            List::Blocking => "Blocking",
            List::Progress => "Progress",
            List::Locks => "Locks",
            List::Prepared => "Prepared transactions",
            List::Configuration => "Configuration",
        }
    }

    /// File in `sql/dashboard/`.
    pub fn file(self) -> &'static str {
        match self {
            List::Sessions => "sessions.sql",
            List::Blocking => "blocking.sql",
            List::Progress => "progress.sql",
            List::Locks => "locks.sql",
            List::Prepared => "prepared.sql",
            List::Configuration => "configuration.sql",
        }
    }

    /// The query takes the database name as `$1`.
    pub fn takes_database(self) -> bool {
        self != List::Configuration
    }

    fn index(self) -> usize {
        self as usize
    }

    /// Columns of the grid (Sessions: the duration column is titled by the mode).
    pub fn columns(self) -> &'static [Col] {
        match self {
            List::Sessions => &[
                ("PID", true),
                ("Database", false),
                ("User", false),
                ("Application", false),
                ("Client", false),
                ("State", false),
                ("Wait event", false),
                ("Blocked by", false),
                ("Duration", true),
                ("Xmin age", true),
                ("Transaction ID", true),
                ("Leader PID", true),
                ("Query ID", true),
                ("Backend start", false),
                ("Transaction start", false),
                ("Query start", false),
                ("Backend type", false),
                ("Query", false),
            ],
            List::Blocking => &[
                ("Blocker PID", true),
                ("Database", false),
                ("Blocker user", false),
                ("Blocker state", false),
                ("Transaction time", true),
                ("Blocked PID", true),
                ("Blocked user", false),
                ("Lock type", false),
                ("Mode", false),
                ("Relation", false),
                ("Waiting for", true),
                ("Blocker query", false),
                ("Blocked query", false),
            ],
            List::Progress => &[
                ("PID", true),
                ("Database", false),
                ("User", false),
                ("Command", false),
                ("Relation", false),
                ("Phase", false),
                ("Done %", true),
                ("Done", true),
                ("Running for", true),
                ("Query", false),
            ],
            List::Locks => &[
                ("PID", true),
                ("Lock type", false),
                ("Database", false),
                ("Relation", false),
                ("Page", true),
                ("Tuple", true),
                ("Virtual XID", false),
                ("XID", true),
                ("Class", false),
                ("Object ID", true),
                ("Sub-object", true),
                ("Virtual transaction", false),
                ("Mode", false),
                ("Granted", false),
                ("Fast path", false),
            ],
            List::Prepared => &[
                ("Name", false),
                ("Database", false),
                ("Owner", false),
                ("Transaction", true),
                ("Prepared", false),
            ],
            List::Configuration => &[
                ("Name", false),
                ("Category", false),
                ("Setting", false),
                ("Unit", false),
                ("Description", false),
            ],
        }
    }
}

/// Columns of `sessions.sql` (the query's rows).
const R_PID: usize = 0;
const R_STATE: usize = 5;
const R_BLOCKED_BY: usize = 7;
const R_QUERY_SECS: usize = 8;
const R_XACT_SECS: usize = 9;
const R_BACKEND_SECS: usize = 10;
const R_LEADER: usize = 13;
const R_BACKEND_START: usize = 15;
/// Columns before / after the three durations that go to the grid unchanged.
const R_BEFORE: usize = 8;
const R_AFTER: usize = 11;
/// Columns of `sessions.sql`.
#[cfg(test)]
pub const SESSION_QUERY_COLUMNS: usize = 20;

/// Sessions grid columns used by the dashboard itself.
pub const COL_PID: usize = 0;
const COL_DATABASE: usize = 1;
const COL_USER: usize = 2;
const COL_STATE: usize = 5;
const COL_DURATION: usize = 8;
const COL_QUERY: usize = 17;

/// Blocking grid columns.
const B_PID: usize = 0;
const B_DATABASE: usize = 1;
const B_USER: usize = 2;
const B_SECS: [usize; 2] = [4, 10];
const B_QUERY: usize = 11;

/// Columns of `progress.sql`.
const P_DONE: usize = 6;
const P_TOTAL: usize = 7;
const P_UNIT: usize = 8;
const P_SECS: usize = 9;
const P_QUERY_ROW: usize = 10;
/// Progress grid columns.
const G_QUERY: usize = 9;

/// Which time the Sessions duration shows (D-057).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationMode {
    Query,
    Transaction,
    Backend,
}

impl DurationMode {
    pub fn next(self) -> Self {
        match self {
            DurationMode::Query => DurationMode::Transaction,
            DurationMode::Transaction => DurationMode::Backend,
            DurationMode::Backend => DurationMode::Query,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DurationMode::Query => "query",
            DurationMode::Transaction => "transaction",
            DurationMode::Backend => "backend",
        }
    }

    fn title(self) -> &'static str {
        match self {
            DurationMode::Query => "Query time",
            DurationMode::Transaction => "Transaction time",
            DurationMode::Backend => "Backend time",
        }
    }

    fn raw(self) -> usize {
        match self {
            DurationMode::Query => R_QUERY_SECS,
            DurationMode::Transaction => R_XACT_SECS,
            DurationMode::Backend => R_BACKEND_SECS,
        }
    }

    /// Durations are coloured (backend times are long by nature).
    fn coloured(self) -> bool {
        self != DurationMode::Backend
    }
}

/// `850 ms`, `12.3 s`, `4 min 05 s`, … of a duration in seconds.
pub fn duration_text(secs: f64) -> String {
    crate::db_stats::millis(secs * 1000.0)
}

/// `12 MB`, as `pg_size_pretty` without the bytes.
pub fn size_text(n: u64) -> String {
    const UNITS: [&str; 6] = ["bytes", "kB", "MB", "GB", "TB", "PB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 10.0 * 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{} {}", thousands(size.round() as usize), UNITS[unit])
}

/// The summary strip's values of one `stats.sql` row (D-057).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub idle_in_xact: u64,
    pub aborted: u64,
    pub waiting: u64,
    pub max_connections: u64,
    /// (count, maximum)
    pub autovacuum: (u64, u64),
    pub parallel: (u64, u64),
    pub logical: (u64, u64),
    pub wal_senders: (u64, u64),
    pub wal_receivers: u64,
    pub slots: (u64, u64),
    /// Seconds since the server started.
    pub uptime: f64,
    /// Temporary files (count, bytes); `None` without the right to list them.
    pub temp: Option<(u64, u64)>,
    pub standby: bool,
    /// Seconds: the oldest open transaction, the longest running query.
    pub oldest_xact: Option<f64>,
    pub longest_query: Option<f64>,
    /// The oldest xmin (age in transactions) and the PID holding it.
    pub oldest_xmin: Option<(u64, String)>,
    /// Cumulative deadlocks, recovery conflicts, checksum failures (`None` before 12).
    pub problems: [Option<u64>; 3],
    /// Bytes: the largest standby lag, the most WAL kept by a replication slot.
    pub lag: Option<u64>,
    pub slot_wal: Option<u64>,
}

/// Columns of `stats.sql`.
pub const STATS_COLUMNS: usize = 41;

/// Counters of one `stats.sql` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub total: u64,
    pub active: u64,
    pub idle: u64,
    /// Cumulative counters (pg_stat_database).
    pub counters: [u64; 9],
    /// Server clock, epoch seconds.
    pub t: f64,
    /// The summary strip's values (`None` for a short row).
    pub summary: Option<Summary>,
}

// indexes into `Sample::counters`
const COMMITS: usize = 0;
const ROLLBACKS: usize = 1;
const INSERTED: usize = 2;
const UPDATED: usize = 3;
const DELETED: usize = 4;
const FETCHED: usize = 5;
const RETURNED: usize = 6;
const BLKS_READ: usize = 7;
const BLKS_HIT: usize = 8;

/// A whole number from a text column (sums are numeric: accept "12.0").
fn count(v: &str) -> Option<u64> {
    let s = v.trim();
    s.parse::<u64>()
        .ok()
        .or_else(|| s.parse::<f64>().ok().map(|f| f.max(0.0) as u64))
}

impl Sample {
    /// From the text columns of `stats.sql`.
    pub fn from_row(row: &[String]) -> Option<Self> {
        if row.len() < 13 {
            return None;
        }
        let n = |i: usize| count(&row[i]);
        let mut counters = [0; 9];
        for (k, c) in counters.iter_mut().enumerate() {
            *c = n(3 + k)?;
        }
        Some(Self {
            total: n(0)?,
            active: n(1)?,
            idle: n(2)?,
            counters,
            t: row[12].trim().parse().ok()?,
            summary: Summary::from_row(row),
        })
    }
}

impl Summary {
    fn from_row(row: &[String]) -> Option<Self> {
        if row.len() < STATS_COLUMNS {
            return None;
        }
        let n = |i: usize| count(&row[i]);
        let pair = |i: usize| Some((n(i)?, n(i + 1)?));
        let secs = |i: usize| row[i].trim().parse::<f64>().ok();
        Some(Self {
            idle_in_xact: n(13)?,
            aborted: n(14)?,
            waiting: n(15)?,
            max_connections: n(16)?,
            autovacuum: pair(17)?,
            parallel: pair(19)?,
            logical: pair(21)?,
            wal_senders: pair(23)?,
            wal_receivers: n(25)?,
            slots: pair(26)?,
            uptime: row[28].trim().parse().ok()?,
            temp: n(29).zip(n(30)),
            standby: row[31].trim() == "true",
            oldest_xact: secs(32),
            longest_query: secs(33),
            oldest_xmin: n(34).map(|age| (age, row[35].trim().to_owned())),
            problems: [n(36), n(37), n(38)],
            lag: n(39),
            slot_wal: n(40),
        })
    }
}

/// One graph box: title, the labels of its values, the current values and the history
/// of its total.
#[derive(Debug, Clone)]
pub struct Graph {
    pub title: &'static str,
    pub labels: &'static [&'static str],
    /// Current values (`None` until there is a rate).
    pub values: Option<Vec<f64>>,
    /// Rates (per second) or counts.
    pub rate: bool,
    pub history: VecDeque<u64>,
}

impl Graph {
    fn new(title: &'static str, labels: &'static [&'static str], rate: bool) -> Self {
        Self {
            title,
            labels,
            values: None,
            rate,
            history: VecDeque::new(),
        }
    }

    /// `values[0]` is the total shown by the sparkline, unless `sum` (then the sum).
    fn push(&mut self, values: Vec<f64>, sum: bool) {
        let total = if sum {
            values.iter().sum()
        } else {
            values.first().copied().unwrap_or(0.0)
        };
        self.history.push_back(total.max(0.0).round() as u64);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.values = Some(values);
    }

    /// `total 42 · active 3 · idle 39`
    pub fn values_text(&self) -> String {
        let Some(values) = &self.values else {
            return "…".to_owned();
        };
        self.labels
            .iter()
            .zip(values)
            .map(|(l, v)| format!("{l} {}", number(*v, self.rate)))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// A count, or a rate with one decimal below 100.
pub fn number(v: f64, rate: bool) -> String {
    if rate && v.abs() < 100.0 && v.fract() != 0.0 {
        format!("{v:.1}")
    } else {
        thousands(v.max(0.0).round() as usize)
    }
}

/// The dashboard's connection (D-057).
#[derive(Debug)]
pub struct DashConn {
    pub id: ConnId,
    pub handle: ConnectionHandle,
    pub connected: bool,
}

/// What the dashboard, profiler and database statistics tabs have in common: their own
/// connection (D-057, D-058, D-062).
pub struct OwnConn<'a> {
    /// Server uid and the database the connection goes to.
    pub uid: u64,
    pub database: &'a str,
    pub conn: &'a mut Option<DashConn>,
    pub version: &'a mut Option<ServerVersion>,
    /// Why the tab has no connection.
    pub problem: &'a mut Option<String>,
}

/// Cancel query / terminate session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Cancel,
    Terminate,
}

impl Signal {
    pub fn file(self) -> &'static str {
        match self {
            Signal::Cancel => "cancel.sql",
            Signal::Terminate => "terminate.sql",
        }
    }
}

/// The session a question / signal is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub pid: String,
    pub user: String,
    pub database: String,
    /// When its backend started, as the Sessions list has it: a signal goes only to
    /// the session that was shown, not to another one that got its PID since (`None`:
    /// not known).
    pub started: Option<String>,
}

/// What colours a Sessions row (computed when the grid is rebuilt).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Mark {
    /// The shown duration, seconds.
    secs: Option<f64>,
    blocked: bool,
    blocking: bool,
    idle_in_xact: bool,
}

/// Signals sent together (one question): what is still on its way and the answers.
#[derive(Debug, Clone, Default)]
pub struct Signals {
    pub signal: Option<Signal>,
    /// (request key, session)
    pub pending: Vec<(u64, Session)>,
    /// (session, sent / no such session, or the server's error)
    pub done: Vec<(Session, Result<bool, String>)>,
}

#[derive(Debug)]
pub struct Dashboard {
    /// The statement pane below the list (D-059).
    pub pane: Pane,
    /// Server uid; database filter (`None` = the whole server); database connected to.
    pub uid: u64,
    pub database: Option<String>,
    pub conn_database: String,
    /// Tab label.
    pub label: String,
    pub conn: Option<DashConn>,
    /// Why there is no connection (failed / lost).
    pub problem: Option<String>,
    pub version: Option<ServerVersion>,
    pub list: List,
    /// One grid per list; rows as last read (before the idle filter).
    pub grids: Vec<ResultSet>,
    raw: Vec<Option<Vec<Vec<String>>>>,
    /// Colours of the Sessions grid rows.
    marks: Vec<Mark>,
    pub graphs: Vec<Graph>,
    prev: Option<Sample>,
    /// Cache hit and rollback ratio (percent) between the last two samples.
    pub cache_hit: Option<f64>,
    pub rollback_ratio: Option<f64>,
    /// Deadlocks, conflicts, checksum failures since the previous refresh.
    pub problems: [Option<u64>; 3],
    /// Database size (bytes, server clock) and its growth (bytes per second).
    pub size: Option<(u64, f64)>,
    pub growth: Option<f64>,
    /// `F5`: read the size with the next refresh.
    pub size_forced: bool,
    pub paused: bool,
    pub hide_idle: bool,
    pub mode: DurationMode,
    /// Tagged sessions (PIDs).
    pub tags: BTreeSet<String>,
    interval: usize,
    /// Requests in flight: stats, size, a list.
    pub stats_key: Option<u64>,
    pub size_key: Option<u64>,
    pub list_key: Option<(u64, List)>,
    /// Signals on their way.
    pub signals: Signals,
    /// A question is open about these sessions.
    pub asking: Option<(Signal, Vec<Session>)>,
    /// Latest timer; older timers are ignored.
    pub timer_seq: u64,
    /// Error of the last refresh (shown in the tab).
    pub error: Option<String>,
    /// Error of the last read of the statistics: shown where the graphs are — the
    /// list below does not depend on it (D-057).
    pub stats_error: Option<String>,
}

impl Dashboard {
    pub fn new(uid: u64, database: Option<String>, conn_database: String, label: String) -> Self {
        let mode = DurationMode::Query;
        Self {
            uid,
            database,
            conn_database,
            label,
            conn: None,
            problem: None,
            version: None,
            list: List::Sessions,
            grids: List::ALL.iter().map(|l| grid_for(*l, mode)).collect(),
            raw: vec![None; List::ALL.len()],
            marks: Vec::new(),
            graphs: vec![
                Graph::new("Sessions", &["total", "active", "idle"], false),
                Graph::new(
                    "Transactions per second",
                    &["total", "commits", "rollbacks"],
                    true,
                ),
                Graph::new("Tuples in", &["inserts", "updates", "deletes"], true),
                Graph::new("Tuples out", &["fetched", "returned"], true),
                Graph::new("Block I/O", &["reads", "hits"], true),
            ],
            prev: None,
            cache_hit: None,
            rollback_ratio: None,
            problems: [None; 3],
            size: None,
            growth: None,
            size_forced: false,
            paused: false,
            hide_idle: false,
            mode,
            tags: BTreeSet::new(),
            interval: DEFAULT_INTERVAL,
            stats_key: None,
            size_key: None,
            list_key: None,
            signals: Signals::default(),
            asking: None,
            timer_seq: 0,
            error: None,
            stats_error: None,
            pane: Pane::default(),
        }
    }

    pub fn connected(&self) -> bool {
        self.conn.as_ref().is_some_and(|c| c.connected)
    }

    pub fn interval_ms(&self) -> u64 {
        INTERVALS_MS[self.interval]
    }

    /// Next refresh interval (wraps).
    pub fn next_interval(&mut self) -> u64 {
        self.interval = (self.interval + 1) % INTERVALS_MS.len();
        self.interval_ms()
    }

    /// Nothing is on its way (time to schedule the next refresh).
    pub fn idle(&self) -> bool {
        self.stats_key.is_none() && self.size_key.is_none() && self.list_key.is_none()
    }

    /// Time to read the database size again (at most every 30 s, or `F5`).
    pub fn size_due(&self) -> bool {
        match (self.size, &self.prev) {
            _ if self.size_forced => true,
            (Some((_, t)), Some(p)) => p.t - t >= SIZE_EVERY,
            _ => true,
        }
    }

    pub fn grid(&self) -> &ResultSet {
        &self.grids[self.list.index()]
    }

    pub fn grid_mut(&mut self) -> &mut ResultSet {
        let i = self.list.index();
        &mut self.grids[i]
    }

    /// The list has been read at least once.
    pub fn loaded(&self, list: List) -> bool {
        self.raw[list.index()].is_some()
    }

    /// A new stats sample: counts at once, rates and ratios from the previous sample.
    pub fn apply_stats(&mut self, s: Sample) {
        self.graphs[0].push(vec![s.total as f64, s.active as f64, s.idle as f64], false);
        if let Some(p) = &self.prev
            && s.t > p.t
        {
            let dt = s.t - p.t;
            // a counter reset (pg_stat_reset) gives 0, not a huge number
            let delta = |k: usize| s.counters[k].saturating_sub(p.counters[k]) as f64;
            let rate = |k: usize| delta(k) / dt;
            let (c, r) = (rate(COMMITS), rate(ROLLBACKS));
            self.graphs[1].push(vec![c + r, c, r], false);
            self.graphs[2].push(vec![rate(INSERTED), rate(UPDATED), rate(DELETED)], true);
            self.graphs[3].push(vec![rate(FETCHED), rate(RETURNED)], true);
            self.graphs[4].push(vec![rate(BLKS_READ), rate(BLKS_HIT)], true);
            let ratio = |part: f64, whole: f64| (whole > 0.0).then(|| part * 100.0 / whole);
            self.cache_hit = ratio(delta(BLKS_HIT), delta(BLKS_HIT) + delta(BLKS_READ));
            self.rollback_ratio = ratio(delta(ROLLBACKS), delta(COMMITS) + delta(ROLLBACKS));
            // problems since the previous refresh
            if let (Some(now), Some(was)) = (&s.summary, &p.summary) {
                for k in 0..3 {
                    self.problems[k] = now.problems[k]
                        .zip(was.problems[k])
                        .map(|(a, b)| a.saturating_sub(b));
                }
            }
        }
        self.prev = Some(s);
    }

    /// A new database size (bytes at server time `t`): the growth from the previous one.
    pub fn apply_size(&mut self, bytes: u64, t: f64) {
        if let Some((b, t0)) = self.size
            && t > t0
        {
            self.growth = Some((bytes as f64 - b as f64) / (t - t0));
        }
        self.size = Some((bytes, t));
        self.size_forced = false;
    }

    /// New rows of `list`; the cursor stays on the row with the same first column.
    pub fn apply_rows(&mut self, list: List, rows: Vec<Vec<String>>) {
        if list == List::Sessions {
            // tags of ended sessions are dropped
            self.tags
                .retain(|p| rows.iter().any(|r| r.get(R_PID) == Some(p)));
        }
        self.raw[list.index()] = Some(rows);
        self.rebuild(list);
    }

    /// Next duration mode; the Sessions grid is rebuilt (no new query).
    pub fn next_mode(&mut self) -> DurationMode {
        self.mode = self.mode.next();
        self.rebuild(List::Sessions);
        self.mode
    }

    /// Grid rows of `list` from its query rows.
    fn grid_rows(&mut self, list: List) -> Vec<Vec<String>> {
        let Some(rows) = &self.raw[list.index()] else {
            return Vec::new();
        };
        match list {
            List::Sessions => {
                let (rows, marks) = sessions_rows(rows, self.mode, self.hide_idle);
                self.marks = marks;
                rows
            }
            List::Blocking => rows
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    for c in B_SECS {
                        if let Some(v) = r.get_mut(c)
                            && let Ok(secs) = v.trim().parse::<f64>()
                        {
                            *v = duration_text(secs);
                        }
                    }
                    r
                })
                .collect(),
            List::Progress => rows.iter().map(|r| progress_row(r)).collect(),
            _ => rows.clone(),
        }
    }

    /// Rebuilds the grid of `list` from its rows (after a refresh, the idle filter or a
    /// new duration mode).
    pub fn rebuild(&mut self, list: List) {
        let i = list.index();
        if self.raw[i].is_none() {
            return;
        }
        let rows = self.grid_rows(list);
        let old = &self.grids[i];
        let key = old
            .rows
            .get(old.row)
            .and_then(|r| r.first())
            .and_then(|v| v.as_deref())
            .map(str::to_owned);
        let (row, col, top, left) = (old.row, old.col, old.top, old.left);
        let mut grid = grid_for(list, self.mode);
        grid.push_rows(
            rows.iter()
                .map(|r| r.iter().map(|v| Some(v.as_str().into())).collect())
                .collect(),
        );
        let found = key.and_then(|k| {
            grid.rows
                .iter()
                .position(|r| r.first().and_then(|v| v.as_deref()) == Some(k.as_str()))
        });
        grid.row = found.unwrap_or(row).min(grid.rows.len().saturating_sub(1));
        grid.col = col.min(grid.columns.len().saturating_sub(1));
        grid.top = top.min(grid.row);
        grid.left = left.min(grid.col);
        self.grids[i] = grid;
    }

    fn cell(&self, list: List, col: usize) -> Option<String> {
        if self.list != list {
            return None;
        }
        let g = self.grid();
        let row = g.rows.get(g.row)?;
        Some(
            row.get(col)
                .and_then(|v| v.as_deref())
                .unwrap_or("")
                .to_owned(),
        )
    }

    /// The session under the cursor: of the Sessions list, or the blocker of the
    /// Blocking list.
    pub fn selected_session(&self) -> Option<Session> {
        let (list, pid, user, database) = match self.list {
            List::Sessions => (List::Sessions, COL_PID, COL_USER, COL_DATABASE),
            List::Blocking => (List::Blocking, B_PID, B_USER, B_DATABASE),
            // Progress: PID, database, user as in Sessions
            List::Progress => (List::Progress, COL_PID, COL_USER, COL_DATABASE),
            _ => return None,
        };
        let pid = self.cell(list, pid)?;
        (!pid.is_empty()).then(|| Session {
            started: self.session_start(&pid),
            pid,
            user: self.cell(list, user).unwrap_or_default(),
            database: self.cell(list, database).unwrap_or_default(),
        })
    }

    /// Backend start of session `pid` in the Sessions rows read last (empty when the
    /// user may not see it).
    fn session_start(&self, pid: &str) -> Option<String> {
        self.raw[List::Sessions.index()]
            .as_ref()?
            .iter()
            .find(|r| r.get(R_PID).map(String::as_str) == Some(pid))?
            .get(R_BACKEND_START)
            .filter(|s| !s.is_empty())
            .cloned()
    }

    /// The tagged sessions, in the order of the Sessions list (D-057).
    pub fn tagged_sessions(&self) -> Vec<Session> {
        let g = &self.grids[List::Sessions.index()];
        let text = |r: &[Option<Box<str>>], c: usize| {
            r.get(c).and_then(|v| v.as_deref()).unwrap_or("").to_owned()
        };
        g.rows
            .iter()
            .filter(|r| {
                r.get(COL_PID)
                    .and_then(|v| v.as_deref())
                    .is_some_and(|p| self.tags.contains(p))
            })
            .map(|r| Session {
                started: self.session_start(&text(r, COL_PID)),
                pid: text(r, COL_PID),
                user: text(r, COL_USER),
                database: text(r, COL_DATABASE),
            })
            .collect()
    }

    /// What cancel / terminate act on: the tagged sessions in the Sessions list, else
    /// the session under the cursor.
    pub fn signal_targets(&self) -> Vec<Session> {
        if self.list == List::Sessions && !self.tags.is_empty() {
            let tagged = self.tagged_sessions();
            if !tagged.is_empty() {
                return tagged;
            }
        }
        self.selected_session().into_iter().collect()
    }

    /// `Insert`: tags / untags the session under the cursor and moves down.
    pub fn toggle_tag(&mut self) -> bool {
        if self.list != List::Sessions {
            return false;
        }
        let Some(s) = self.selected_session() else {
            return false;
        };
        if !self.tags.remove(&s.pid) {
            self.tags.insert(s.pid);
        }
        self.grid_mut().move_cursor(1, 0);
        true
    }

    /// The whole row under the cursor as `name: value` lines (Enter).
    pub fn row_lines(&self) -> Option<Vec<String>> {
        self.grid().row_lines()
    }

    /// The statement pane is part of the shown list (Sessions, Blocking, Progress,
    /// D-059).
    pub fn has_pane(&self) -> bool {
        matches!(self.list, List::Sessions | List::Blocking | List::Progress)
    }

    /// The query of the session under the cursor (the blocker's) and its database
    /// (D-059).
    pub fn statement(&self) -> Option<(String, String)> {
        match self.list {
            List::Sessions => Some((
                self.cell(List::Sessions, COL_QUERY)?,
                self.cell(List::Sessions, COL_DATABASE)?,
            )),
            List::Blocking => Some((
                self.cell(List::Blocking, B_QUERY)?,
                self.cell(List::Blocking, B_DATABASE)?,
            )),
            List::Progress => Some((
                self.cell(List::Progress, G_QUERY)?,
                self.cell(List::Progress, COL_DATABASE)?,
            )),
            _ => None,
        }
    }

    /// Text style of a Sessions cell (row colours win over cell colours, tags over
    /// everything, D-057).
    fn session_style(&self, row: usize, col: usize, s: &Styles) -> Option<Style> {
        let g = &self.grids[List::Sessions.index()];
        let pid = g.rows.get(row)?.get(COL_PID)?.as_deref()?;
        if self.tags.contains(pid) {
            return Some(s.tagged);
        }
        let mark = self.marks.get(row)?;
        if mark.blocked {
            return Some(s.blocked);
        }
        if mark.blocking {
            return Some(s.blocking);
        }
        match col {
            COL_STATE if mark.idle_in_xact => Some(s.idle_in_xact),
            COL_DURATION if self.mode.coloured() => match mark.secs? {
                v if v >= VERY_SLOW => Some(s.very_slow),
                v if v >= SLOW => Some(s.slow),
                _ => None,
            },
            _ => None,
        }
    }

    /// The four lines of the summary strip (D-057): a title and its parts, joined by
    /// ` · `; an alert part (a problem since the last refresh) is coloured.
    pub fn strip_lines(&self) -> [(&'static str, Vec<Part>); 4] {
        let dash = || "-".to_owned();
        let pct = |v: Option<f64>| v.map_or_else(dash, |v| format!("{v:.2} %"));
        let titles = ["Global", "Sessions", "Workers", "Replication"];
        let Some(sample) = &self.prev else {
            return titles.map(|t| (t, vec![Part::new("…".to_owned())]));
        };
        let have = sample.summary.is_some();
        let sum = sample.summary.clone().unwrap_or_default();
        let n = |v: u64| if have { thousands(v as usize) } else { dash() };
        let of = |(a, b): (u64, u64)| {
            if have {
                format!("{} / {}", thousands(a as usize), thousands(b as usize))
            } else {
                dash()
            }
        };
        let time = |v: Option<f64>| v.map_or_else(dash, duration_text);
        let bytes = |v: Option<u64>| v.map_or_else(dash, size_text);
        let size = match (self.size, self.growth) {
            (Some((b, _)), Some(g)) => {
                let sign = if g < 0.0 { "-" } else { "+" };
                format!(
                    "{} ({sign}{}/s)",
                    size_text(b),
                    size_text(g.abs().round() as u64)
                )
            }
            (Some((b, _)), None) => size_text(b),
            _ => dash(),
        };
        let temp = match sum.temp {
            Some((files, bytes)) if have => {
                format!("{} ({})", thousands(files as usize), size_text(bytes))
            }
            _ => dash(),
        };
        let role = match (have, sum.standby) {
            (false, _) => dash(),
            (true, false) => "primary".to_owned(),
            (true, true) => "standby".to_owned(),
        };
        let uptime = if have {
            duration_text(sum.uptime)
        } else {
            dash()
        };
        let p = |t: String| Part::new(t);
        let global = vec![
            p(role),
            p(format!("up {uptime}")),
            p(format!("size {size}")),
            p(format!("cache hit {}", pct(self.cache_hit))),
            p(format!("rollback {}", pct(self.rollback_ratio))),
            p(format!("temp files {temp}")),
        ];
        let count = if have {
            of((sample.total, sum.max_connections))
        } else {
            thousands(sample.total as usize)
        };
        let xmin = match &sum.oldest_xmin {
            Some((age, pid)) => format!("{} (PID {pid})", thousands(*age as usize)),
            None => dash(),
        };
        let sessions = vec![
            p(count),
            p(format!("active {}", thousands(sample.active as usize))),
            p(format!("idle {}", thousands(sample.idle as usize))),
            p(format!("idle in transaction {}", n(sum.idle_in_xact))),
            p(format!("aborted {}", n(sum.aborted))),
            p(format!("waiting {}", n(sum.waiting))),
            p(format!("oldest transaction {}", time(sum.oldest_xact))),
            p(format!("longest query {}", time(sum.longest_query))),
            p(format!("oldest xmin {xmin}")),
        ];
        let mut workers = vec![
            p(format!("autovacuum {}", of(sum.autovacuum))),
            p(format!("parallel {}", of(sum.parallel))),
            p(format!("logical replication {}", of(sum.logical))),
        ];
        for (name, v) in ["deadlocks", "conflicts", "checksum failures"]
            .iter()
            .zip(self.problems)
        {
            workers.push(Part {
                text: format!("{name} {}", v.map_or_else(dash, |v| thousands(v as usize))),
                alert: v.is_some_and(|v| v > 0),
            });
        }
        let replication = vec![
            p(format!("WAL senders {}", of(sum.wal_senders))),
            p(format!("receivers {}", n(sum.wal_receivers))),
            p(format!("slots {}", of(sum.slots))),
            p(format!("lag {}", bytes(sum.lag))),
            p(format!("slot WAL {}", bytes(sum.slot_wal))),
        ];
        let [a, b, c, d] = titles;
        [(a, global), (b, sessions), (c, workers), (d, replication)]
    }
}

/// One value of the summary strip.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub text: String,
    /// A problem: shown in `dashboard.very_slow`.
    pub alert: bool,
}

impl Part {
    fn new(text: String) -> Self {
        Self { text, alert: false }
    }
}

/// The text of a strip line (parts joined by ` · `).
#[cfg(test)]
pub fn line_text(parts: &[Part]) -> String {
    parts
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A Progress grid row from a `progress.sql` row: done % and `done / total unit`.
fn progress_row(r: &[String]) -> Vec<String> {
    let text = |c: usize| r.get(c).map_or("", |v| v.trim());
    let num = |c: usize| text(c).parse::<f64>().ok().map(|v| v.max(0.0) as u64);
    let bytes = text(P_UNIT) == "bytes";
    let amount = |v: u64| {
        if bytes {
            size_text(v)
        } else {
            format!("{} blocks", thousands(v as usize))
        }
    };
    let (pct, done) = match (num(P_DONE), num(P_TOTAL)) {
        (Some(d), Some(t)) if t > 0 => {
            let total = if bytes {
                size_text(t)
            } else {
                thousands(t as usize)
            };
            let done = if bytes {
                size_text(d)
            } else {
                thousands(d as usize)
            };
            let unit = if bytes { "" } else { " blocks" };
            (
                format!("{:.1} %", (d as f64 * 100.0 / t as f64).min(100.0)),
                format!("{done} / {total}{unit}"),
            )
        }
        (Some(d), _) => (String::new(), amount(d)),
        _ => (String::new(), String::new()),
    };
    let mut row: Vec<String> = r.iter().take(P_DONE).cloned().collect();
    row.resize(P_DONE, String::new());
    row.push(pct);
    row.push(done);
    row.push(text(P_SECS).parse().map(duration_text).unwrap_or_default());
    row.push(r.get(P_QUERY_ROW).cloned().unwrap_or_default());
    row
}

/// Sessions grid rows from the query rows: the duration of `mode`, ordered longest
/// first, parallel workers after their leader, idle sessions left out if `hide_idle`.
fn sessions_rows(
    rows: &[Vec<String>],
    mode: DurationMode,
    hide_idle: bool,
) -> (Vec<Vec<String>>, Vec<Mark>) {
    fn text(r: &[String], c: usize) -> &str {
        r.get(c).map_or("", |v| v.as_str())
    }
    /// The leader of a parallel worker, if it is in the list.
    fn leader_of<'a>(r: &'a [String], pids: &HashSet<&str>) -> Option<&'a str> {
        let l = text(r, R_LEADER);
        (!l.is_empty() && l != text(r, R_PID) && pids.contains(l)).then_some(l)
    }
    let secs = |r: &[String]| text(r, mode.raw()).trim().parse::<f64>().ok();
    let pid_num = |r: &[String]| text(r, R_PID).parse::<u64>().unwrap_or(0);
    // sessions others wait for (of all sessions, also hidden ones)
    let blocking: HashSet<&str> = rows
        .iter()
        .flat_map(|r| text(r, R_BLOCKED_BY).split(','))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let shown: Vec<&Vec<String>> = rows
        .iter()
        .filter(|r| !(hide_idle && text(r, R_STATE) == "idle"))
        .collect();
    let pids: HashSet<&str> = shown.iter().map(|r| text(r, R_PID)).collect();
    let (mut workers, mut tops): (Vec<&Vec<String>>, Vec<&Vec<String>>) = shown
        .iter()
        .copied()
        .partition(|r| leader_of(r, &pids).is_some());
    tops.sort_by(|a, b| {
        let (x, y) = (secs(a), secs(b));
        match (x, y) {
            (Some(x), Some(y)) => y.total_cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then(pid_num(a).cmp(&pid_num(b)))
    });
    workers.sort_by_key(|r| pid_num(r));
    let mut ordered = Vec::with_capacity(shown.len());
    for top in tops {
        ordered.push(top);
        let pid = text(top, R_PID);
        ordered.extend(workers.iter().filter(|w| leader_of(w, &pids) == Some(pid)));
    }
    let mut out = Vec::with_capacity(ordered.len());
    let mut marks = Vec::with_capacity(ordered.len());
    for r in ordered {
        let s = secs(r);
        let mut row: Vec<String> = r.iter().take(R_BEFORE).cloned().collect();
        row.resize(R_BEFORE, String::new());
        row.push(s.map(duration_text).unwrap_or_default());
        row.extend(r.iter().skip(R_AFTER).cloned());
        out.push(row);
        let state = text(r, R_STATE);
        marks.push(Mark {
            secs: s,
            blocked: !text(r, R_BLOCKED_BY).trim().is_empty(),
            blocking: blocking.contains(text(r, R_PID)),
            idle_in_xact: state.starts_with("idle in transaction"),
        });
    }
    (out, marks)
}

/// The statement pane below a dashboard or profiler list (D-059).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pane {
    pub visible: bool,
    /// Height in percent of the tab (as the query tab's results, D-019).
    pub percent: u16,
}

impl Default for Pane {
    fn default() -> Self {
        Self {
            visible: true,
            percent: 33,
        }
    }
}

fn grid_for(list: List, mode: DurationMode) -> ResultSet {
    ResultSet::new(
        list.columns()
            .iter()
            .enumerate()
            .map(|(i, (name, numeric))| Column {
                name: if list == List::Sessions && i == COL_DURATION {
                    mode.title().to_owned()
                } else {
                    (*name).to_owned()
                },
                // int8 / text: only used for the alignment
                type_oid: if *numeric { 20 } else { 25 },
            })
            .collect(),
    )
}

// ---- drawing ------------------------------------------------------------------------

/// Click areas of a drawn dashboard.
#[derive(Debug, Default, Clone)]
pub struct DashHits {
    /// List sub-tabs.
    pub lists: Vec<(List, Rect)>,
    /// The grid body.
    pub grid: Rect,
    pub cells: Vec<crate::grid::CellArea>,
}

/// Height of one graph box: title, sparkline rows, values.
fn box_height(spark: u16) -> u16 {
    spark + 2
}

/// Rows below the summary strip that the list keeps: separator, bar, separator,
/// header and two rows.
const LIST_MIN: u16 = 6;

/// Graph rows (0 = no graphs, 1 or 2), sparkline height and strip lines for an area.
/// The graphs go first when the window is short, then strip lines (D-057).
fn layout_for(area: Rect) -> (u16, u16, u16) {
    let strip = STRIP_LINES.min(area.height.saturating_sub(LIST_MIN));
    let rows = if area.width >= 5 * 24 { 1 } else { 2 };
    // graphs + the separator between them and the strip
    let room = area.height.saturating_sub(LIST_MIN + strip + 1);
    if room < rows * box_height(1) {
        return (0, 0, strip);
    }
    let spark = (room / rows).saturating_sub(2).clamp(1, 4);
    (rows, spark, strip)
}

/// Theme styles of the Sessions colours.
struct Styles {
    blocked: Style,
    blocking: Style,
    slow: Style,
    very_slow: Style,
    idle_in_xact: Style,
    tagged: Style,
}

/// Draws the dashboard into `area` (inside the tab window, below the tab strip
/// separator). `separator` draws a line joined to the window frame.
pub fn render(
    frame: &mut Frame,
    d: &Dashboard,
    area: Rect,
    theme: &Theme,
    focused: bool,
    separator: &dyn Fn(&mut Frame, Rect),
) -> DashHits {
    let mut hits = DashHits::default();
    let normal = theme.get(Role::CoreNormal);
    let hint = theme.get(Role::CoreHint);
    if area.height < 4 {
        return hits;
    }
    if !d.connected() {
        let text = match (&d.problem, &d.conn) {
            (Some(p), _) => format!(" {p}\n\n F5 connects again."),
            (None, Some(_)) => " Connecting…".to_owned(),
            (None, None) => " Not connected — F5 connects.".to_owned(),
        };
        let lines: Vec<Line> = text
            .lines()
            .map(|l| Line::from(Span::styled(l.to_owned(), hint)))
            .collect();
        frame.render_widget(Paragraph::new(lines).style(normal), area);
        return hits;
    }
    let (rows, spark, strip) = layout_for(area);
    let graphs_h = rows * box_height(spark);
    let [graphs_area, sep0, strip_area, sep1, bar, sep2, body] = Layout::vertical([
        Constraint::Length(graphs_h),
        Constraint::Length(u16::from(rows > 0)),
        Constraint::Length(strip),
        Constraint::Length(u16::from(strip > 0)),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    // the statistics could not be read: the reason where they would be
    let stats_error = |frame: &mut Frame, area: Rect| {
        let lines: Vec<Line> = d
            .stats_error
            .iter()
            .flat_map(|e| e.lines())
            .map(|l| {
                Line::from(Span::styled(
                    format!(" {l}"),
                    theme.get(Role::MessagesError),
                ))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).style(normal), area);
    };
    let failed = d.stats_error.is_some();
    if rows > 0 {
        if failed {
            stats_error(frame, graphs_area);
        } else {
            render_graphs(frame, d, graphs_area, rows, spark, theme);
        }
        separator(frame, sep0);
    }
    if strip > 0 {
        if failed && rows == 0 {
            stats_error(frame, strip_area);
        } else {
            render_strip(frame, d, strip_area, theme);
        }
        separator(frame, sep1);
    }
    separator(frame, sep2);

    // list sub-tabs, like the results bar (D-019)
    let bar_style = theme.get(Role::TabsBar);
    frame.render_widget(Paragraph::new("").style(bar_style), bar);
    let mut spans = Vec::new();
    let mut x = bar.x;
    for list in List::ALL {
        let label = if list == d.list && d.loaded(list) {
            let n = thousands(d.grid().rows.len());
            if list == List::Sessions && !d.tags.is_empty() {
                // in the label: the hints after the sub-tabs may not fit
                let tagged = thousands(d.tags.len());
                format!(" {} ({n} · {tagged} tagged) ", list.title())
            } else {
                format!(" {} ({n}) ", list.title())
            }
        } else {
            format!(" {} ", list.title())
        };
        let w = label.chars().count() as u16;
        let style = if list == d.list {
            theme.get(Role::TabsActive)
        } else {
            theme.get(Role::TabsInactive)
        };
        hits.lists.push((list, Rect::new(x, bar.y, w, 1)));
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(" ", bar_style));
        x = x.saturating_add(w + 1);
    }
    if d.list == List::Sessions && d.hide_idle {
        spans.push(Span::styled(" idle sessions hidden ", hint));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), bar);

    hits.grid = body;
    if let Some(e) = &d.error {
        let lines: Vec<Line> = e
            .lines()
            .map(|l| {
                Line::from(Span::styled(
                    format!(" {l}"),
                    theme.get(Role::MessagesError),
                ))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).style(normal), body);
        return hits;
    }
    if !d.loaded(d.list) {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" loading…", hint))).style(normal),
            body,
        );
        return hits;
    }
    let styles = grid_styles(theme);
    let colours = Styles {
        blocked: theme.get(Role::DashboardBlocked),
        blocking: theme.get(Role::DashboardBlocking),
        slow: theme.get(Role::DashboardSlow),
        very_slow: theme.get(Role::DashboardVerySlow),
        idle_in_xact: theme.get(Role::DashboardIdleInTransaction),
        tagged: theme.get(Role::DashboardTagged),
    };
    let is_sessions = d.list == List::Sessions;
    let cell_style = |row: usize, col: usize| -> Option<Style> {
        if is_sessions {
            d.session_style(row, col, &colours)
        } else {
            None
        }
    };
    hits.cells = crate::grid::render_styled(
        d.grid(),
        body,
        frame.buffer_mut(),
        &styles,
        focused,
        &cell_style,
    );
    hits
}

/// Grid colours from the theme (as the query tab's grid, D-041).
pub fn grid_styles(t: &Theme) -> GridStyles {
    GridStyles {
        text: t.get(Role::CoreNormal),
        header: t.get(Role::GridHeader),
        null: t.get(Role::GridNull),
        cursor: t.get(Role::GridCursor),
        row_number: t.get(Role::GridRowNumber),
        separator: {
            let text = t.get(Role::CoreNormal);
            match t.get(Role::CoreBorder).fg {
                Some(fg) => text.fg(fg),
                None => text,
            }
        },
        ascii: t.ascii,
    }
}

/// The summary strip: a title and its values per line (D-057).
fn render_strip(frame: &mut Frame, d: &Dashboard, area: Rect, t: &Theme) {
    let normal = t.get(Role::CoreNormal);
    let title = t.get(Role::CoreTitle);
    let alert = t.get(Role::DashboardVerySlow);
    let lines: Vec<Line> = d
        .strip_lines()
        .into_iter()
        .take(area.height as usize)
        .map(|(name, parts)| {
            let mut spans = vec![Span::styled(format!(" {name:<12}"), title)];
            for (i, part) in parts.into_iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" · ", normal));
                }
                spans.push(Span::styled(
                    part.text,
                    if part.alert { alert } else { normal },
                ));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).style(normal), area);
}

fn render_graphs(frame: &mut Frame, d: &Dashboard, area: Rect, rows: u16, spark: u16, t: &Theme) {
    let normal = t.get(Role::CoreNormal);
    let title = t.get(Role::CoreTitle);
    let line_style = {
        match t.get(Role::CoreBorder).fg {
            Some(fg) => normal.fg(fg),
            None => normal,
        }
    };
    let graph = t.get(Role::DashboardGraph);
    let per_row: Vec<&[Graph]> = if rows == 1 {
        vec![&d.graphs[..]]
    } else {
        vec![&d.graphs[..3], &d.graphs[3..]]
    };
    let h = box_height(spark);
    for (r, graphs) in per_row.iter().enumerate() {
        let row = Rect::new(area.x, area.y + r as u16 * h, area.width, h);
        // equal boxes with a vertical line between them
        let n = graphs.len() as u16;
        let width = (row.width.saturating_sub(n - 1)) / n;
        for (k, g) in graphs.iter().enumerate() {
            let x = row.x + k as u16 * (width + 1);
            let w = if k as u16 == n - 1 {
                row.right().saturating_sub(x)
            } else {
                width
            };
            let cell = Rect::new(x, row.y, w, h);
            if k > 0 {
                let buf = frame.buffer_mut();
                for y in row.y..row.bottom() {
                    buf.set_string(x - 1, y, t.vline(), line_style);
                }
            }
            let [head, chart, values] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(spark),
                Constraint::Length(1),
            ])
            .areas(cell);
            let head_text: String = format!(" {}", g.title).chars().take(w as usize).collect();
            frame.render_widget(Paragraph::new(head_text).style(title), head);
            let chart = Rect {
                x: chart.x + 1,
                width: chart.width.saturating_sub(2),
                ..chart
            };
            let data: Vec<u64> = g
                .history
                .iter()
                .skip(g.history.len().saturating_sub(chart.width as usize))
                .copied()
                .collect();
            frame.render_widget(Paragraph::new("").style(normal), chart);
            frame.render_widget(
                Sparkline::default()
                    .data(&data)
                    .max(data.iter().copied().max().unwrap_or(0).max(1))
                    .style(graph),
                chart,
            );
            let v: String = format!(" {}", g.values_text())
                .chars()
                .take(w as usize)
                .collect();
            frame.render_widget(Paragraph::new(v).style(normal), values);
        }
    }
}

impl crate::monitor::MonitorTab for Dashboard {
    fn own_conn(&mut self) -> crate::dashboard::OwnConn<'_> {
        crate::dashboard::OwnConn {
            uid: self.uid,
            database: &self.conn_database,
            conn: &mut self.conn,
            version: &mut self.version,
            problem: &mut self.problem,
        }
    }

    fn place(&self) -> (u64, &str, Option<bool>) {
        (
            self.uid,
            &self.conn_database,
            self.conn.as_ref().map(|c| c.connected),
        )
    }

    fn list(&self) -> &ResultSet {
        self.grid()
    }

    fn list_mut(&mut self) -> &mut ResultSet {
        self.grid_mut()
    }

    fn status_text(&self) -> String {
        if !self.connected() {
            String::new()
        } else if self.paused {
            "Paused (Ctrl+P resumes)".to_owned()
        } else {
            format!("Refresh every {}", interval_text(self.interval_ms()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(total: u64, base: u64, t: f64) -> Vec<String> {
        let mut row = vec![total.to_string(), "1".into(), (total - 1).to_string()];
        for k in 0..9 {
            row.push((base * (k + 1)).to_string());
        }
        row.push(t.to_string());
        row
    }

    /// A full `stats.sql` row: the graph counters and the summary values.
    fn full_stats(t: f64, temp: bool, deadlocks: u64) -> Vec<String> {
        let mut row = stats(5, 100, t);
        for v in [
            "2", "0", "1", "100", "1", "3", "0", "8", "0", "4", "0", "10", "0", "0", "10", "90061",
        ] {
            row.push(v.into());
        }
        if temp {
            row.extend(["2".into(), "20480".into()]);
        } else {
            row.extend([String::new(), String::new()]);
        }
        // primary, oldest transaction 65 s, longest query 12.5 s, oldest xmin 1,234 of
        // PID 4242, deadlocks, conflicts, checksum failures, no standby lag, 1 GB in slots
        for v in ["false", "65", "12.5", "1234", "4242"] {
            row.push(v.into());
        }
        row.push(deadlocks.to_string());
        for v in ["0", "0", "", "1073741824"] {
            row.push(v.into());
        }
        row
    }

    #[test]
    fn rates_from_two_samples() {
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        d.apply_stats(Sample::from_row(&stats(5, 100, 1000.0)).unwrap());
        assert_eq!(d.graphs[0].values, Some(vec![5.0, 1.0, 4.0]));
        assert_eq!(d.graphs[1].values, None, "no rate from one sample");
        assert_eq!(d.graphs[1].values_text(), "…");
        // two seconds later every counter grew by 2 * (k + 1) * 10
        d.apply_stats(Sample::from_row(&stats(6, 110, 1002.0)).unwrap());
        // commits 10 → 5/s, rollbacks 20 → 10/s
        assert_eq!(d.graphs[1].values, Some(vec![15.0, 5.0, 10.0]));
        assert_eq!(d.graphs[1].history.back(), Some(&15));
        // inserts 15/s, updates 20/s, deletes 25/s: the graph shows the sum
        assert_eq!(d.graphs[2].history.back(), Some(&60));
        assert_eq!(
            d.graphs[1].values_text(),
            "total 15 · commits 5 · rollbacks 10"
        );
        // reads 80, hits 90 between the samples; rollbacks 20 of 30
        assert!((d.cache_hit.unwrap() - 90.0 * 100.0 / 170.0).abs() < 1e-9);
        assert!((d.rollback_ratio.unwrap() - 200.0 / 3.0).abs() < 1e-9);
        // a counter reset does not give a huge rate
        d.apply_stats(Sample::from_row(&stats(6, 1, 1004.0)).unwrap());
        assert_eq!(d.graphs[1].values, Some(vec![0.0, 0.0, 0.0]));
        assert_eq!(d.cache_hit, None, "no blocks: no ratio");
        assert!(Sample::from_row(&["1".into()]).is_none());
        assert_eq!(number(2.25, true), "2.2");
        assert_eq!(number(1234.4, true), "1,234");
        assert_eq!(number(3.0, true), "3");
    }

    #[test]
    fn summary_strip() {
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        assert_eq!(line_text(&d.strip_lines()[0].1), "…");
        assert!(d.size_due());
        let row = full_stats(1000.0, true, 7);
        assert_eq!(row.len(), STATS_COLUMNS);
        d.apply_stats(Sample::from_row(&row).unwrap());
        d.apply_size(20 * 1024 * 1024, 1000.0);
        assert!(!d.size_due());
        let lines = d.strip_lines().map(|(t, p)| (t, line_text(&p)));
        assert_eq!(lines[0].0, "Global");
        assert_eq!(
            lines[0].1,
            "primary · up 1 d 01 h · size 20 MB · cache hit - · rollback - · \
             temp files 2 (20 kB)"
        );
        assert_eq!(
            lines[1].1,
            "5 / 100 · active 1 · idle 4 · idle in transaction 2 · aborted 0 · waiting 1 · \
             oldest transaction 1 min 05 s · longest query 12.5 s · oldest xmin 1,234 (PID 4242)"
        );
        // problems need two samples
        assert_eq!(
            lines[2].1,
            "autovacuum 1 / 3 · parallel 0 / 8 · logical replication 0 / 4 · \
             deadlocks - · conflicts - · checksum failures -"
        );
        assert_eq!(lines[3].0, "Replication");
        assert_eq!(
            lines[3].1,
            "WAL senders 0 / 10 · receivers 0 · slots 0 / 10 · lag - · slot WAL 1,024 MB"
        );
        // 30 s later the size is read again: the growth per second; 2 new deadlocks
        d.apply_stats(Sample::from_row(&full_stats(1030.0, false, 9)).unwrap());
        assert!(d.size_due());
        d.apply_size(20 * 1024 * 1024 + 30 * 2048, 1030.0);
        let [global, _, workers, _] = d.strip_lines();
        let text = line_text(&global.1);
        assert!(text.contains("size 20 MB (+2,048 bytes/s)"), "{text}");
        assert!(text.ends_with("temp files -"), "{text}");
        let alerts: Vec<(&str, bool)> = workers
            .1
            .iter()
            .skip(3)
            .map(|p| (p.text.as_str(), p.alert))
            .collect();
        assert_eq!(
            alerts,
            [
                ("deadlocks 2", true),
                ("conflicts 0", false),
                ("checksum failures 0", false)
            ]
        );
        d.size_forced = true;
        assert!(d.size_due());
        // an old 13-column row: the counts are there, the rest is "-"
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        d.apply_stats(Sample::from_row(&stats(5, 100, 1000.0)).unwrap());
        assert!(line_text(&d.strip_lines()[1].1).starts_with("5 · active 1"));
        assert!(line_text(&d.strip_lines()[0].1).starts_with("- · up -"));
        assert_eq!(interval_text(500), "0.5 s");
        assert_eq!(interval_text(2000), "2 s");
        assert_eq!(size_text(500), "500 bytes");
    }

    /// A `sessions.sql` row.
    fn session(pid: &str, state: &str, blocked: &str) -> Vec<String> {
        let mut row = vec![String::new(); SESSION_QUERY_COLUMNS];
        row[R_PID] = pid.into();
        row[1] = "db".into();
        row[2] = "u".into();
        row[R_STATE] = state.into();
        row[R_BLOCKED_BY] = blocked.into();
        row[R_BACKEND_SECS] = "100".into();
        row[R_BACKEND_START] = format!("2026-11-15 10:00:{pid}");
        row[19] = format!("query of {pid}");
        row
    }

    fn timed(pid: &str, query: &str, xact: &str, leader: &str) -> Vec<String> {
        let mut row = session(pid, "active", "");
        row[R_QUERY_SECS] = query.into();
        row[R_XACT_SECS] = xact.into();
        row[R_LEADER] = leader.into();
        row
    }

    fn pids(d: &Dashboard) -> Vec<String> {
        d.grid()
            .rows
            .iter()
            .map(|r| r[COL_PID].as_deref().unwrap_or("").to_owned())
            .collect()
    }

    #[test]
    fn cursor_follows_the_row_and_idle_filter() {
        let mut d = Dashboard::new(1, Some("db".into()), "db".into(), "x".into());
        assert!(!d.loaded(List::Sessions));
        d.apply_rows(
            List::Sessions,
            vec![
                session("10", "idle", ""),
                session("20", "active", ""),
                session("30", "active", "20"),
            ],
        );
        d.grid_mut().row = 2;
        assert_eq!(d.selected_session().unwrap().pid, "30");
        assert_eq!(d.statement().unwrap().0, "query of 30");
        // 10 ended: the cursor stays on 30
        d.apply_rows(
            List::Sessions,
            vec![session("20", "active", ""), session("30", "active", "20")],
        );
        assert_eq!(d.grid().row, 1);
        d.apply_rows(
            List::Sessions,
            vec![
                session("5", "idle", ""),
                session("20", "active", ""),
                session("30", "active", "20"),
            ],
        );
        assert_eq!(d.selected_session().unwrap().pid, "30");
        d.hide_idle = true;
        d.rebuild(List::Sessions);
        assert_eq!(d.grid().rows.len(), 2);
        assert_eq!(d.selected_session().unwrap().pid, "30");
        // gone altogether: the cursor stays in range
        d.apply_rows(List::Sessions, vec![session("20", "active", "")]);
        assert_eq!(d.grid().row, 0);
        let lines = d.row_lines().unwrap();
        assert!(lines[0].starts_with("PID"), "{lines:?}");
        assert!(lines[0].ends_with("20"), "{lines:?}");
        assert_eq!(d.grid().columns.len(), List::Sessions.columns().len());
        assert_eq!(d.grid().rows[0].len(), List::Sessions.columns().len());
        // other lists have no session
        d.list = List::Locks;
        assert!(d.selected_session().is_none());
        assert!(!d.has_pane());
    }

    #[test]
    fn durations_order_and_colours() {
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        d.apply_rows(
            List::Sessions,
            vec![
                timed("1", "", "", ""),
                timed("2", "0.5", "50", ""),
                timed("3", "12.5", "13", ""),
                // a parallel worker of 3, and one of a leader not shown
                timed("4", "12", "12", "3"),
                timed("5", "2", "2", "99"),
            ],
        );
        assert_eq!(d.grid().columns[COL_DURATION].name, "Query time");
        // longest first, the worker after its leader, no duration last
        assert_eq!(pids(&d), ["3", "4", "5", "2", "1"]);
        assert_eq!(d.grid().rows[0][COL_DURATION].as_deref(), Some("12.5 s"));
        assert_eq!(d.grid().rows[3][COL_DURATION].as_deref(), Some("500 ms"));
        assert_eq!(d.grid().rows[4][COL_DURATION].as_deref(), Some(""));
        let s = Styles {
            blocked: Style::new().fg(ratatui::style::Color::Red),
            blocking: Style::new().fg(ratatui::style::Color::Magenta),
            slow: Style::new().fg(ratatui::style::Color::Yellow),
            very_slow: Style::new().fg(ratatui::style::Color::LightRed),
            idle_in_xact: Style::new().fg(ratatui::style::Color::Cyan),
            tagged: Style::new().fg(ratatui::style::Color::White),
        };
        assert_eq!(d.session_style(0, COL_DURATION, &s), Some(s.very_slow));
        assert_eq!(d.session_style(2, COL_DURATION, &s), Some(s.slow));
        assert_eq!(d.session_style(3, COL_DURATION, &s), None);
        assert_eq!(d.session_style(0, COL_PID, &s), None);
        // transaction mode: a new order, no new query
        assert_eq!(d.next_mode(), DurationMode::Transaction);
        assert_eq!(d.grid().columns[COL_DURATION].name, "Transaction time");
        assert_eq!(pids(&d), ["2", "3", "4", "5", "1"]);
        // backend mode: not coloured
        assert_eq!(d.next_mode(), DurationMode::Backend);
        assert_eq!(d.session_style(0, COL_DURATION, &s), None);
        assert_eq!(d.next_mode(), DurationMode::Query);

        // blocked, blocking and idle in transaction rows
        let mut idle = session("7", "idle in transaction", "");
        idle[R_QUERY_SECS] = "1".into();
        d.apply_rows(
            List::Sessions,
            vec![
                session("6", "active", "8"),
                idle,
                session("8", "active", ""),
            ],
        );
        assert_eq!(pids(&d), ["7", "6", "8"]);
        assert_eq!(d.session_style(0, COL_STATE, &s), Some(s.idle_in_xact));
        assert_eq!(d.session_style(0, COL_DURATION, &s), Some(s.slow));
        assert_eq!(d.session_style(1, COL_PID, &s), Some(s.blocked));
        assert_eq!(d.session_style(2, COL_STATE, &s), Some(s.blocking));
        // a tag wins
        d.tags.insert("6".into());
        assert_eq!(d.session_style(1, COL_PID, &s), Some(s.tagged));
    }

    #[test]
    fn tags_and_signal_targets() {
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        d.apply_rows(
            List::Sessions,
            vec![
                session("10", "active", ""),
                session("20", "active", ""),
                session("30", "active", ""),
            ],
        );
        assert_eq!(d.signal_targets().len(), 1, "the row under the cursor");
        assert!(d.toggle_tag());
        assert_eq!(d.grid().row, 1, "moves down");
        d.grid_mut().row = 2;
        assert!(d.toggle_tag());
        // each with the backend start that was shown: the signal checks it
        let starts: Vec<Option<String>> =
            d.signal_targets().into_iter().map(|s| s.started).collect();
        assert_eq!(
            starts,
            [
                Some("2026-11-15 10:00:10".to_owned()),
                Some("2026-11-15 10:00:30".to_owned())
            ]
        );
        let targets: Vec<String> = d.signal_targets().into_iter().map(|s| s.pid).collect();
        assert_eq!(targets, ["10", "30"]);
        // untag, and an ended session loses its tag
        d.grid_mut().row = 0;
        assert!(d.toggle_tag());
        d.apply_rows(List::Sessions, vec![session("20", "active", "")]);
        assert!(d.tags.is_empty());
        // no tags in other lists
        d.list = List::Locks;
        assert!(!d.toggle_tag());
    }

    #[test]
    fn blocking_list() {
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        d.list = List::Blocking;
        let row: Vec<String> = [
            "100",
            "db",
            "alice",
            "idle in transaction",
            "75",
            "200",
            "bob",
            "relation",
            "AccessExclusiveLock",
            "t",
            "3.5",
            "lock table t",
            "select * from t",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(row.len(), List::Blocking.columns().len());
        d.apply_rows(List::Blocking, vec![row]);
        let g = d.grid();
        assert_eq!(g.rows[0][4].as_deref(), Some("1 min 15 s"));
        assert_eq!(g.rows[0][10].as_deref(), Some("3.5 s"));
        assert!(d.has_pane());
        assert_eq!(
            d.statement(),
            Some(("lock table t".to_owned(), "db".to_owned()))
        );
        let s = d.selected_session().unwrap();
        assert_eq!((s.pid.as_str(), s.user.as_str()), ("100", "alice"));
        assert_eq!(d.signal_targets(), vec![s]);
    }

    #[test]
    fn progress_list() {
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        d.list = List::Progress;
        let row = |pid: &str, done: &str, total: &str, unit: &str| -> Vec<String> {
            [
                pid,
                "db",
                "alice",
                "VACUUM",
                "t",
                "scanning heap",
                done,
                total,
                unit,
                "75",
                "vacuum t",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect()
        };
        d.apply_rows(
            List::Progress,
            vec![
                row("10", "12000", "50000", "blocks"),
                row("20", "30720", "0", "bytes"),
                row("30", "", "", "blocks"),
            ],
        );
        let g = d.grid();
        assert_eq!(g.columns.len(), List::Progress.columns().len());
        let cells = |r: usize| -> Vec<&str> {
            g.rows[r]
                .iter()
                .map(|v| v.as_deref().unwrap_or(""))
                .collect()
        };
        assert_eq!(
            cells(0)[6..],
            ["24.0 %", "12,000 / 50,000 blocks", "1 min 15 s", "vacuum t"]
        );
        // unknown total: no %
        assert_eq!(cells(1)[6..8], ["", "30 kB"]);
        assert_eq!(cells(2)[6..8], ["", ""]);
        assert!(d.has_pane());
        assert_eq!(
            d.statement(),
            Some(("vacuum t".to_owned(), "db".to_owned()))
        );
        assert_eq!(d.selected_session().unwrap().pid, "10");
    }

    #[test]
    fn intervals_cycle() {
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        assert_eq!(d.interval_ms(), 2000);
        assert_eq!(d.next_interval(), 5000);
        assert_eq!(d.next_interval(), 10_000);
        assert_eq!(d.next_interval(), 30_000);
        assert_eq!(d.next_interval(), 500);
        assert_eq!(d.next_interval(), 1000);
    }

    #[test]
    fn layout_drops_graphs_first() {
        // tall and wide: one row of graphs with 4-line sparklines
        assert_eq!(layout_for(Rect::new(0, 0, 200, 40)), (1, 4, 4));
        // narrow: two rows
        assert_eq!(layout_for(Rect::new(0, 0, 100, 40)).0, 2);
        // short: no graphs, the whole strip
        assert_eq!(layout_for(Rect::new(0, 0, 200, 12)), (0, 0, 4));
        // shorter still: fewer strip lines
        assert_eq!(layout_for(Rect::new(0, 0, 200, 8)), (0, 0, 2));
    }
}
