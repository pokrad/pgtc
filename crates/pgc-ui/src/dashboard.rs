//! Server dashboard tab (D-057): its state (graph samples and rates, the lists) and
//! its drawing. The app side (connection, refresh, commands) is in `app/dashboard.rs`.

use std::collections::VecDeque;

use pgc_core::ServerVersion;
use pgc_db::{Column, ConnId, ConnectionHandle};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Sparkline};

use crate::grid::{GridStyles, ResultSet, thousands};
use crate::theme::{Role, Theme};

/// Refresh intervals in seconds, cycled by `dashboard_interval`; the second is the
/// default (D-057).
pub const INTERVALS: [u64; 5] = [1, 2, 5, 10, 30];
const DEFAULT_INTERVAL: usize = 1;
/// Samples kept per graph (more than any terminal is wide).
const HISTORY: usize = 400;

/// The lists below the graphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    Sessions,
    Locks,
    Prepared,
    Configuration,
}

/// (column name, numeric = right-aligned)
type Col = (&'static str, bool);

impl List {
    pub const ALL: [List; 4] = [
        List::Sessions,
        List::Locks,
        List::Prepared,
        List::Configuration,
    ];

    pub fn title(self) -> &'static str {
        match self {
            List::Sessions => "Sessions",
            List::Locks => "Locks",
            List::Prepared => "Prepared transactions",
            List::Configuration => "Configuration",
        }
    }

    /// File in `sql/dashboard/`.
    pub fn file(self) -> &'static str {
        match self {
            List::Sessions => "sessions.sql",
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

    /// Columns, in the order of the query's columns.
    pub fn columns(self) -> &'static [Col] {
        match self {
            List::Sessions => &[
                ("PID", true),
                ("Database", false),
                ("User", false),
                ("Application", false),
                ("Client", false),
                ("Backend start", false),
                ("State", false),
                ("Wait event", false),
                ("Blocked by", false),
                ("Transaction start", false),
                ("Query start", false),
                ("Running for", false),
                ("Backend type", false),
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

/// Sessions columns used by the dashboard itself.
pub const COL_PID: usize = 0;
const COL_DATABASE: usize = 1;
const COL_USER: usize = 2;
const COL_STATE: usize = 6;
const COL_BLOCKED_BY: usize = 8;
const COL_QUERY: usize = 13;

/// Counters of one `stats.sql` row.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    pub total: u64,
    pub active: u64,
    pub idle: u64,
    /// Cumulative counters (pg_stat_database).
    pub counters: [u64; 9],
    /// Server clock, epoch seconds.
    pub t: f64,
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

impl Sample {
    /// From the text columns of `stats.sql`.
    pub fn from_row(row: &[String]) -> Option<Self> {
        if row.len() < 13 {
            return None;
        }
        let n = |i: usize| -> Option<u64> {
            let s = row[i].trim();
            // sums are numeric: a fraction never appears, but accept "12.0"
            s.parse::<u64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|f| f.max(0.0) as u64))
        };
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
    pub graphs: Vec<Graph>,
    prev: Option<Sample>,
    pub paused: bool,
    pub hide_idle: bool,
    interval: usize,
    /// Requests in flight: stats, a list, a signal.
    pub stats_key: Option<u64>,
    pub list_key: Option<(u64, List)>,
    pub signal_key: Option<(u64, Signal, Session)>,
    /// A question is open about this session.
    pub asking: Option<(Signal, Session)>,
    /// Latest timer; older timers are ignored.
    pub timer_seq: u64,
    /// Error of the last refresh (shown in the tab).
    pub error: Option<String>,
}

impl Dashboard {
    pub fn new(uid: u64, database: Option<String>, conn_database: String, label: String) -> Self {
        Self {
            uid,
            database,
            conn_database,
            label,
            conn: None,
            problem: None,
            version: None,
            list: List::Sessions,
            grids: List::ALL.iter().map(|l| grid_for(*l)).collect(),
            raw: vec![None; List::ALL.len()],
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
            paused: false,
            hide_idle: false,
            interval: DEFAULT_INTERVAL,
            stats_key: None,
            list_key: None,
            signal_key: None,
            asking: None,
            timer_seq: 0,
            error: None,
            pane: Pane::default(),
        }
    }

    pub fn connected(&self) -> bool {
        self.conn.as_ref().is_some_and(|c| c.connected)
    }

    /// The tab's own connection, as the three monitor tabs share it.
    pub fn own_conn(&mut self) -> OwnConn<'_> {
        OwnConn {
            uid: self.uid,
            database: &self.conn_database,
            conn: &mut self.conn,
            version: &mut self.version,
            problem: &mut self.problem,
        }
    }

    pub fn interval_secs(&self) -> u64 {
        INTERVALS[self.interval]
    }

    /// Next refresh interval (wraps).
    pub fn next_interval(&mut self) -> u64 {
        self.interval = (self.interval + 1) % INTERVALS.len();
        self.interval_secs()
    }

    /// Nothing is on its way (time to schedule the next refresh).
    pub fn idle(&self) -> bool {
        self.stats_key.is_none() && self.list_key.is_none()
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

    /// A new stats sample: counts at once, rates from the previous sample.
    pub fn apply_stats(&mut self, s: Sample) {
        self.graphs[0].push(vec![s.total as f64, s.active as f64, s.idle as f64], false);
        if let Some(p) = self.prev
            && s.t > p.t
        {
            let dt = s.t - p.t;
            // a counter reset (pg_stat_reset) gives 0, not a huge number
            let rate = |k: usize| s.counters[k].saturating_sub(p.counters[k]) as f64 / dt;
            let (c, r) = (rate(COMMITS), rate(ROLLBACKS));
            self.graphs[1].push(vec![c + r, c, r], false);
            self.graphs[2].push(vec![rate(INSERTED), rate(UPDATED), rate(DELETED)], true);
            self.graphs[3].push(vec![rate(FETCHED), rate(RETURNED)], true);
            self.graphs[4].push(vec![rate(BLKS_READ), rate(BLKS_HIT)], true);
        }
        self.prev = Some(s);
    }

    /// New rows of `list`; the cursor stays on the row with the same first column.
    pub fn apply_rows(&mut self, list: List, rows: Vec<Vec<String>>) {
        self.raw[list.index()] = Some(rows);
        self.rebuild(list);
    }

    /// Rebuilds the grid of `list` from its rows (after a refresh or the idle filter).
    pub fn rebuild(&mut self, list: List) {
        let i = list.index();
        let Some(rows) = &self.raw[i] else {
            return;
        };
        let old = &self.grids[i];
        let key = old
            .rows
            .get(old.row)
            .and_then(|r| r.first())
            .and_then(|v| v.as_deref())
            .map(str::to_owned);
        let (row, col, top, left) = (old.row, old.col, old.top, old.left);
        let hide = list == List::Sessions && self.hide_idle;
        let mut grid = grid_for(list);
        grid.push_rows(
            rows.iter()
                .filter(|r| !(hide && r.get(COL_STATE).is_some_and(|s| s == "idle")))
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

    /// The session under the cursor of the Sessions list.
    pub fn selected_session(&self) -> Option<Session> {
        if self.list != List::Sessions {
            return None;
        }
        let g = self.grid();
        let row = g.rows.get(g.row)?;
        let text = |i: usize| {
            row.get(i)
                .and_then(|v| v.as_deref())
                .unwrap_or("")
                .to_owned()
        };
        let pid = text(COL_PID);
        (!pid.is_empty()).then(|| Session {
            pid,
            user: text(COL_USER),
            database: text(COL_DATABASE),
        })
    }

    /// The whole row under the cursor as `name: value` lines (Enter).
    pub fn row_lines(&self) -> Option<Vec<String>> {
        self.grid().row_lines()
    }

    /// The statement pane is part of the shown list (Sessions only, D-059).
    pub fn has_pane(&self) -> bool {
        self.list == List::Sessions
    }

    /// The query of the session under the cursor and its database (D-059).
    pub fn statement(&self) -> Option<(String, String)> {
        if !self.has_pane() {
            return None;
        }
        let g = self.grid();
        let row = g.rows.get(g.row)?;
        let col = |i: usize| {
            row.get(i)
                .and_then(|v| v.as_deref())
                .unwrap_or("")
                .to_owned()
        };
        Some((col(COL_QUERY), col(COL_DATABASE)))
    }
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

fn grid_for(list: List) -> ResultSet {
    ResultSet::new(
        list.columns()
            .iter()
            .map(|(name, numeric)| Column {
                name: (*name).to_owned(),
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

/// Graph rows (1 or 2) and sparkline height for an area.
fn graph_layout(area: Rect) -> (u16, u16) {
    let rows = if area.width >= 5 * 24 { 1 } else { 2 };
    // keep at least 6 rows for the list (separator, bar, separator, header, 2 rows)
    let room = area.height.saturating_sub(7) / rows;
    let spark = room.saturating_sub(2).clamp(1, 4);
    (rows, spark)
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
    let (rows, spark) = graph_layout(area);
    let graphs_h = rows * box_height(spark);
    let [graphs_area, sep1, bar, sep2, body] = Layout::vertical([
        Constraint::Length(graphs_h),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    render_graphs(frame, d, graphs_area, rows, spark, theme);
    separator(frame, sep1);
    separator(frame, sep2);

    // list sub-tabs, like the results bar (D-019)
    let bar_style = theme.get(Role::TabsBar);
    frame.render_widget(Paragraph::new("").style(bar_style), bar);
    let mut spans = Vec::new();
    let mut x = bar.x;
    for list in List::ALL {
        let label = if list == d.list && d.loaded(list) {
            let n = d.grid().rows.len();
            format!(" {} ({}) ", list.title(), thousands(n))
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
    let blocked = theme.get(Role::DashboardBlocked);
    let grid = d.grid();
    let is_sessions = d.list == List::Sessions;
    let row_style = |i: usize| -> Option<Style> {
        let row = grid.rows.get(i)?;
        let by = row.get(COL_BLOCKED_BY)?.as_deref()?;
        (is_sessions && !by.is_empty()).then_some(blocked)
    };
    hits.cells =
        crate::grid::render_styled(grid, body, frame.buffer_mut(), &styles, focused, &row_style);
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
        // a counter reset does not give a huge rate
        d.apply_stats(Sample::from_row(&stats(6, 1, 1004.0)).unwrap());
        assert_eq!(d.graphs[1].values, Some(vec![0.0, 0.0, 0.0]));
        assert!(Sample::from_row(&["1".into()]).is_none());
        assert_eq!(number(2.25, true), "2.2");
        assert_eq!(number(1234.4, true), "1,234");
        assert_eq!(number(3.0, true), "3");
    }

    fn session(pid: &str, state: &str, blocked: &str) -> Vec<String> {
        let mut row = vec![String::new(); List::Sessions.columns().len()];
        row[COL_PID] = pid.into();
        row[COL_DATABASE] = "db".into();
        row[COL_USER] = "u".into();
        row[COL_STATE] = state.into();
        row[COL_BLOCKED_BY] = blocked.into();
        row
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
        // other lists have no session
        d.list = List::Locks;
        assert!(d.selected_session().is_none());
    }

    #[test]
    fn intervals_cycle() {
        let mut d = Dashboard::new(1, None, "postgres".into(), "x".into());
        assert_eq!(d.interval_secs(), 2);
        assert_eq!(d.next_interval(), 5);
        assert_eq!(d.next_interval(), 10);
        assert_eq!(d.next_interval(), 30);
        assert_eq!(d.next_interval(), 1);
    }
}
