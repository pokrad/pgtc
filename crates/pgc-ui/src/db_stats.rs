//! Database statistics tab (D-062): its state (the statistics as last read) and its
//! drawing. The app side (connection, refresh, commands) is in `app/db_stats.rs`.

use pgc_core::ServerVersion;
use pgc_db::Column;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::dashboard::DashConn;
use crate::grid::{ResultSet, thousands};
use crate::theme::{Role, Theme};

/// One row of `stats.sql`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub key: String,
    pub group: String,
    pub label: String,
    pub kind: String,
    /// As read (empty = NULL).
    pub value: String,
}

impl Stat {
    /// From the text columns of `stats.sql`: key, group, statistic, kind, value.
    pub fn from_row(row: &[String]) -> Option<Self> {
        let [key, group, label, kind, value] = row else {
            return None;
        };
        Some(Self {
            key: key.clone(),
            group: group.clone(),
            label: label.clone(),
            kind: kind.clone(),
            value: value.clone(),
        })
    }

    /// The value as shown (`None` for NULL).
    pub fn shown(&self) -> Option<String> {
        (!self.value.is_empty()).then(|| format_value(&self.kind, &self.value))
    }
}

/// A value of `kind` (count, bytes, ms, percent, time, text) for display.
pub fn format_value(kind: &str, value: &str) -> String {
    let v = value.trim();
    match kind {
        "count" => v.parse::<u64>().map_or_else(|_| v.to_owned(), big),
        "bytes" => v.parse::<u64>().map_or_else(|_| v.to_owned(), bytes),
        "ms" => v.parse::<f64>().map_or_else(|_| v.to_owned(), millis),
        "percent" => v
            .parse::<f64>()
            .map_or_else(|_| v.to_owned(), |p| format!("{p:.2} %")),
        _ => v.to_owned(),
    }
}

fn big(n: u64) -> String {
    thousands(n as usize)
}

/// `12 MB (12,582,912 bytes)`, units of 1024 as `pg_size_pretty`.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["bytes", "kB", "MB", "GB", "TB", "PB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 10.0 * 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} bytes", big(n))
    } else {
        format!("{} {} ({} bytes)", size.round(), UNITS[unit], big(n))
    }
}

/// `850 ms`, `12.3 s`, `4 min 05 s`, `2 h 03 min`, `3 d 04 h`.
pub fn millis(ms: f64) -> String {
    let ms = ms.max(0.0);
    if ms < 1000.0 {
        return format!("{} ms", ms.round());
    }
    let s = ms / 1000.0;
    if s < 60.0 {
        return format!("{s:.1} s");
    }
    let s = s.round() as u64;
    let (d, h, m, sec) = (s / 86400, s / 3600 % 24, s / 60 % 60, s % 60);
    if d > 0 {
        format!("{d} d {h:02} h")
    } else if h > 0 {
        format!("{h} h {m:02} min")
    } else {
        format!("{m} min {sec:02} s")
    }
}

/// The statistics tab of one database (D-062).
#[derive(Debug)]
pub struct DbStats {
    /// Server uid; the database (connected to).
    pub uid: u64,
    pub database: String,
    /// Tab label.
    pub label: String,
    pub conn: Option<DashConn>,
    /// Why there is no connection (failed / lost).
    pub problem: Option<String>,
    pub version: Option<ServerVersion>,
    /// The statistics as last read (`None` until the first answer).
    pub stats: Option<Vec<Stat>>,
    pub grid: ResultSet,
    /// The request in flight.
    pub key: Option<u64>,
    /// Error of the last read (shown in the tab).
    pub error: Option<String>,
}

impl DbStats {
    pub fn new(uid: u64, database: String, label: String) -> Self {
        Self {
            uid,
            database,
            label,
            conn: None,
            problem: None,
            version: None,
            stats: None,
            grid: empty_grid(),
            key: None,
            error: None,
        }
    }

    pub fn connected(&self) -> bool {
        self.conn.as_ref().is_some_and(|c| c.connected)
    }

    /// New rows of `stats.sql`; the cursor stays on the same statistic.
    pub fn apply_rows(&mut self, rows: &[Vec<String>]) {
        let stats: Vec<Stat> = rows.iter().filter_map(|r| Stat::from_row(r)).collect();
        let old = &self.grid;
        let (row, col, top, left) = (old.row, old.col, old.top, old.left);
        let mut grid = empty_grid();
        grid.push_rows(
            stats
                .iter()
                // shown in the tab status instead
                .filter(|s| s.key != "read_at")
                .map(|s| {
                    vec![
                        Some(s.group.as_str().into()),
                        Some(s.label.as_str().into()),
                        s.shown().map(Into::into),
                    ]
                })
                .collect(),
        );
        grid.row = row.min(grid.rows.len().saturating_sub(1));
        grid.col = col.min(grid.columns.len().saturating_sub(1));
        grid.top = top.min(grid.row);
        grid.left = left.min(grid.col);
        self.grid = grid;
        self.stats = Some(stats);
        self.error = None;
    }

    fn raw(&self, key: &str) -> Option<&str> {
        self.stats
            .as_ref()?
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.value.as_str())
            .filter(|v| !v.is_empty())
    }

    fn shown(&self, key: &str) -> Option<String> {
        self.stats
            .as_ref()?
            .iter()
            .find(|s| s.key == key)
            .and_then(Stat::shown)
    }

    /// When the values were read (server clock).
    pub fn read_at(&self) -> Option<&str> {
        self.raw("read_at")
    }

    /// The summary boxes: (title, value).
    pub fn summary(&self) -> Vec<(&'static str, String)> {
        let dash = || "–".to_owned();
        let size = self
            .raw("size")
            .and_then(|v| v.parse::<u64>().ok())
            .map_or_else(dash, short_bytes);
        let connections = match (self.shown("backends"), self.raw("conn_limit")) {
            (Some(b), Some("no limit")) => format!("{b} (no limit)"),
            (Some(b), Some(l)) => format!("{b} / {l}"),
            (Some(b), None) => b,
            _ => dash(),
        };
        let cache = self.shown("cache_hit").unwrap_or_else(dash);
        let transactions = match (self.shown("xact_commit"), self.shown("xact_rollback")) {
            (Some(c), Some(r)) => format!("committed {c} · rolled back {r}"),
            _ => dash(),
        };
        let xid = self.shown("xid_age").unwrap_or_else(dash);
        vec![
            ("Size", size),
            ("Connections", connections),
            ("Cache hit ratio", cache),
            ("Transactions", transactions),
            ("XID age", xid),
        ]
    }
}

/// `12 MB` (the summary box; the list has the bytes too).
fn short_bytes(n: u64) -> String {
    let full = bytes(n);
    match full.split_once(" (") {
        Some((short, _)) => short.to_owned(),
        None => full,
    }
}

fn empty_grid() -> ResultSet {
    ResultSet::new(
        ["Group", "Statistic", "Value"]
            .iter()
            .map(|name| Column {
                name: (*name).to_owned(),
                type_oid: 25,
            })
            .collect(),
    )
}

// ---- drawing ------------------------------------------------------------------------

/// Click areas of a drawn statistics tab.
#[derive(Debug, Default, Clone)]
pub struct StatsHits {
    /// The grid body.
    pub grid: Rect,
    pub cells: Vec<crate::grid::CellArea>,
}

/// Width below which the boxes go into two rows (3 + 2).
const ONE_ROW_WIDTH: u16 = 5 * 22;

/// Draws the tab into `area` (inside the tab window, below the tab strip separator).
/// `separator` draws a line joined to the window frame.
pub fn render(
    frame: &mut Frame,
    s: &DbStats,
    area: Rect,
    theme: &Theme,
    focused: bool,
    separator: &dyn Fn(&mut Frame, Rect),
) -> StatsHits {
    let mut hits = StatsHits::default();
    let normal = theme.get(Role::CoreNormal);
    let hint = theme.get(Role::CoreHint);
    if area.height < 4 {
        return hits;
    }
    let message = |frame: &mut Frame, text: String, style: ratatui::style::Style| {
        let lines: Vec<Line> = text
            .lines()
            .map(|l| Line::from(Span::styled(format!(" {l}"), style)))
            .collect();
        frame.render_widget(Paragraph::new(lines).style(normal), area);
    };
    if !s.connected() && s.stats.is_none() {
        let text = match (&s.problem, &s.conn) {
            (Some(p), _) => format!("{p}\n\nF5 connects again."),
            (None, Some(_)) => "Connecting…".to_owned(),
            (None, None) => "Not connected — F5 connects.".to_owned(),
        };
        message(frame, text, hint);
        return hits;
    }
    let rows: u16 = if area.width >= ONE_ROW_WIDTH { 1 } else { 2 };
    let [boxes, sep, body] = Layout::vertical([
        Constraint::Length(rows * 2),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    render_boxes(frame, s, boxes, rows, theme);
    separator(frame, sep);
    hits.grid = body;
    if let Some(e) = &s.error {
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
    if s.stats.is_none() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" loading…", hint))).style(normal),
            body,
        );
        return hits;
    }
    let styles = crate::dashboard::grid_styles(theme);
    hits.cells = crate::grid::render(&s.grid, body, frame.buffer_mut(), &styles, focused);
    hits
}

/// The summary boxes: title and value, a vertical line between them.
fn render_boxes(frame: &mut Frame, s: &DbStats, area: Rect, rows: u16, t: &Theme) {
    let normal = t.get(Role::CoreNormal);
    let title = t.get(Role::CoreTitle);
    let line_style = match t.get(Role::CoreBorder).fg {
        Some(fg) => normal.fg(fg),
        None => normal,
    };
    frame.render_widget(Paragraph::new("").style(normal), area);
    let summary = s.summary();
    let per_row: Vec<&[(&str, String)]> = if rows == 1 {
        vec![&summary[..]]
    } else {
        vec![&summary[..3], &summary[3..]]
    };
    for (r, boxes) in per_row.iter().enumerate() {
        let row = Rect::new(area.x, area.y + r as u16 * 2, area.width, 2);
        let n = boxes.len() as u16;
        let width = row.width.saturating_sub(n - 1) / n;
        for (k, (head, value)) in boxes.iter().enumerate() {
            let x = row.x + k as u16 * (width + 1);
            let w = if k as u16 == n - 1 {
                row.right().saturating_sub(x)
            } else {
                width
            };
            if k > 0 {
                let buf = frame.buffer_mut();
                for y in row.y..row.bottom() {
                    buf.set_string(x - 1, y, t.vline(), line_style);
                }
            }
            let cut = |text: String| -> String { text.chars().take(w as usize).collect() };
            frame.render_widget(
                Paragraph::new(cut(format!(" {head}"))).style(title),
                Rect::new(x, row.y, w, 1),
            );
            frame.render_widget(
                Paragraph::new(cut(format!(" {value}"))).style(normal),
                Rect::new(x, row.y + 1, w, 1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, group: &str, label: &str, kind: &str, value: &str) -> Vec<String> {
        [key, group, label, kind, value]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn values_are_formatted_by_kind() {
        assert_eq!(format_value("count", "1234567"), "1,234,567");
        assert_eq!(format_value("bytes", "512"), "512 bytes");
        assert_eq!(
            format_value("bytes", "12582912"),
            "12 MB (12,582,912 bytes)"
        );
        assert_eq!(format_value("bytes", "8000"), "8,000 bytes");
        assert_eq!(format_value("percent", "99.257"), "99.26 %");
        assert_eq!(format_value("ms", "850.4"), "850 ms");
        assert_eq!(format_value("ms", "12345"), "12.3 s");
        assert_eq!(format_value("ms", "245000"), "4 min 05 s");
        assert_eq!(format_value("ms", "7380000"), "2 h 03 min");
        assert_eq!(format_value("ms", "273600000"), "3 d 04 h");
        assert_eq!(format_value("text", "no limit"), "no limit");
        // not a number: as read
        assert_eq!(format_value("count", "checksums off"), "checksums off");
    }

    #[test]
    fn rows_grid_and_summary() {
        let mut s = DbStats::new(1, "db".into(), "Stats: a · db".into());
        assert_eq!(s.summary()[0], ("Size", "–".to_owned()));
        s.apply_rows(&[
            row(
                "read_at",
                "Database",
                "Read at",
                "time",
                "2026-11-13 10:00:00+01",
            ),
            row("size", "Database", "Size", "bytes", "12582912"),
            row("backends", "Database", "Backends", "count", "3"),
            row(
                "conn_limit",
                "Database",
                "Connection limit",
                "text",
                "no limit",
            ),
            row("stats_reset", "Database", "Statistics reset", "time", ""),
            row("xact_commit", "Transactions", "Committed", "count", "1000"),
            row("xact_rollback", "Transactions", "Rolled back", "count", "2"),
            row(
                "cache_hit",
                "Block I/O",
                "Cache hit ratio",
                "percent",
                "99.5",
            ),
            vec!["short".into()],
        ]);
        assert_eq!(s.read_at(), Some("2026-11-13 10:00:00+01"));
        // Read at is not in the list; the broken row is left out
        assert_eq!(s.grid.rows.len(), 7);
        assert_eq!(s.grid.rows[0][1].as_deref(), Some("Size"));
        assert_eq!(
            s.grid.rows[0][2].as_deref(),
            Some("12 MB (12,582,912 bytes)")
        );
        // NULL stays NULL
        assert_eq!(s.grid.rows[3][2], None);
        let summary = s.summary();
        assert_eq!(summary[0].1, "12 MB");
        assert_eq!(summary[1].1, "3 (no limit)");
        assert_eq!(summary[2].1, "99.50 %");
        assert_eq!(summary[3].1, "committed 1,000 · rolled back 2");
        assert_eq!(summary[4].1, "–");

        // the cursor stays on its row across a refresh
        s.grid.row = 4;
        s.apply_rows(&[
            row("size", "Database", "Size", "bytes", "1"),
            row("conn_limit", "Database", "Connection limit", "text", "20"),
            row("backends", "Database", "Backends", "count", "4"),
            row("a", "g", "a", "count", "1"),
            row("b", "g", "b", "count", "1"),
            row("c", "g", "c", "count", "1"),
        ]);
        assert_eq!(s.grid.row, 4);
        assert_eq!(s.summary()[1].1, "4 / 20");
        s.apply_rows(&[]);
        assert_eq!(s.grid.row, 0);
    }
}
