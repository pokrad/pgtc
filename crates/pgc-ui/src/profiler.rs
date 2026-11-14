//! SQL profiler tab (D-058): statements read from the server log (jsonlog / csvlog)
//! through the connection. This file has the pure parts — log records, entries,
//! filters, the settings to change and to put back — and the drawing; the app side
//! (connection, start / stop, reading) is in `app/profiler.rs`.

use std::collections::VecDeque;

use pgc_core::ServerVersion;
use pgc_db::Column;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::dashboard::DashConn;
use crate::grid::{ResultSet, thousands};
use crate::theme::{Role, Theme};

/// Entries kept; when there are this many plus [`DROP_CHUNK`], the oldest chunk goes.
pub const MAX_ENTRIES: usize = 100_000;
const DROP_CHUNK: usize = 10_000;
/// Bytes read from the log per request.
pub const READ_CHUNK: u64 = 1024 * 1024;

/// Machine-readable log formats (D-058).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Csv,
}

impl LogFormat {
    /// Name in `log_destination` / for `pg_current_logfile`.
    pub fn name(self) -> &'static str {
        match self {
            LogFormat::Json => "jsonlog",
            LogFormat::Csv => "csvlog",
        }
    }
}

/// The fields of one log record pgtc uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    pub time: String,
    pub user: String,
    pub database: String,
    pub pid: String,
    pub client: String,
    pub severity: String,
    pub message: String,
    pub detail: String,
    /// The statement of an error (`statement` / `query`).
    pub statement: String,
    pub application: String,
}

/// Splits the bytes read from the log into complete records; a partial record waits
/// for the next read.
#[derive(Debug)]
pub struct LogReader {
    pub format: LogFormat,
    pending: Vec<u8>,
}

impl LogReader {
    pub fn new(format: LogFormat) -> Self {
        Self {
            format,
            pending: Vec::new(),
        }
    }

    /// Forgets a partial record (a new log file starts).
    pub fn reset(&mut self) {
        self.pending.clear();
    }

    /// Adds `bytes`; returns the records completed by them.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Record> {
        self.pending.extend_from_slice(bytes);
        let end = match self.format {
            LogFormat::Json => self.pending.iter().rposition(|b| *b == b'\n'),
            LogFormat::Csv => csv_end(&self.pending),
        };
        let Some(end) = end else {
            return Vec::new();
        };
        let complete: Vec<u8> = self.pending.drain(..=end).collect();
        match self.format {
            LogFormat::Json => complete
                .split(|b| *b == b'\n')
                .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
                .filter_map(|l| json_record(&String::from_utf8_lossy(l)))
                .collect(),
            LogFormat::Csv => csv_records(&complete),
        }
    }
}

/// Index of the last line break outside quotes (the end of the last complete record).
fn csv_end(bytes: &[u8]) -> Option<usize> {
    let mut quoted = false;
    let mut end = None;
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'"' => quoted = !quoted,
            b'\n' if !quoted => end = Some(i),
            _ => {}
        }
    }
    end
}

/// csvlog columns (the same positions in PostgreSQL 11 to 18).
fn csv_records(bytes: &[u8]) -> Vec<Record> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(bytes);
    let mut out = Vec::new();
    for rec in reader.byte_records() {
        let Ok(rec) = rec else {
            continue;
        };
        let f = |i: usize| {
            rec.get(i)
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .unwrap_or_default()
        };
        if rec.len() < 23 {
            continue;
        }
        out.push(Record {
            time: f(0),
            user: f(1),
            database: f(2),
            pid: f(3),
            client: f(4),
            severity: f(11),
            message: f(13),
            detail: f(14),
            statement: f(19),
            application: f(22),
        });
    }
    out
}

/// One jsonlog line (PostgreSQL 15+).
fn json_record(line: &str) -> Option<Record> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| match v.get(k) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    let host = s("remote_host");
    let port = s("remote_port");
    let client = match (host.is_empty(), port.is_empty()) {
        (false, false) => format!("{host}:{port}"),
        _ => host,
    };
    Some(Record {
        time: s("timestamp"),
        user: s("user"),
        database: s("dbname"),
        pid: s("pid"),
        client,
        severity: s("error_severity"),
        message: s("message"),
        detail: s("detail"),
        statement: s("statement"),
        application: s("application_name"),
    })
}

/// What a shown entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Statement,
    Execute,
    Error,
    /// A duration line not recognised (other `lc_messages` languages): its own text.
    Other(String),
}

impl Kind {
    pub fn label(&self) -> &str {
        match self {
            Kind::Statement => "statement",
            Kind::Execute => "execute",
            Kind::Error => "error",
            Kind::Other(s) => s,
        }
    }
}

/// One profiler entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub time: String,
    pub duration: Option<f64>,
    pub pid: String,
    pub user: String,
    pub database: String,
    pub application: String,
    pub client: String,
    pub kind: Kind,
    pub statement: String,
    /// Parameters of a prepared statement, or the error message.
    pub detail: String,
}

/// `duration: 0.123 ms  statement: SELECT 1` → (0.123, "statement", "SELECT 1"). The
/// words may be translated; the shape (`word: number ms  words: text`) is the same.
fn split_duration(message: &str) -> Option<(f64, &str, &str)> {
    let (_, rest) = message.split_once(": ")?;
    let (number, rest) = rest.split_once(' ')?;
    let ms: f64 = number.parse().ok()?;
    let rest = rest.strip_prefix("ms")?.trim_start();
    let (kind, text) = rest.split_once(": ")?;
    Some((ms, kind, text))
}

/// A record as a profiler entry; `None` for records that are not statements or errors
/// (connections, checkpoints, the extended protocol's parse / bind steps, D-058).
pub fn entry(r: Record) -> Option<Entry> {
    let (duration, kind, statement, detail) = match r.severity.as_str() {
        "ERROR" | "FATAL" | "PANIC" => (None, Kind::Error, r.statement, r.message),
        "LOG" => {
            let (duration, kind, text) = match split_duration(&r.message) {
                Some((ms, kind, text)) => (Some(ms), kind, text),
                // log_statement lines: `statement: …` without a duration
                None => {
                    let (kind, text) = r.message.split_once(": ")?;
                    if !(kind == "statement" || kind.starts_with("execute ")) {
                        return None;
                    }
                    (None, kind, text)
                }
            };
            let kind = if kind == "statement" {
                Kind::Statement
            } else if kind.starts_with("execute") {
                Kind::Execute
            } else if kind.starts_with("parse") || kind.starts_with("bind") {
                return None;
            } else {
                Kind::Other(kind.to_owned())
            };
            (duration, kind, text.to_owned(), r.detail)
        }
        _ => return None,
    };
    Some(Entry {
        time: r.time,
        duration,
        pid: r.pid,
        user: r.user,
        database: r.database,
        application: r.application,
        client: r.client,
        kind,
        statement,
        detail,
    })
}

/// Filters (D-058): empty = all; the text is a case-insensitive "contains".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub user: String,
    pub database: String,
    pub pid: String,
    pub application: String,
    pub text: String,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    pub fn matches(&self, e: &Entry) -> bool {
        let eq = |want: &str, have: &str| want.is_empty() || want == have;
        eq(self.user.trim(), &e.user)
            && eq(self.database.trim(), &e.database)
            && eq(self.pid.trim(), &e.pid)
            && eq(self.application.trim(), &e.application)
            && (self.text.is_empty()
                || e.statement
                    .to_lowercase()
                    .contains(&self.text.to_lowercase()))
    }

    /// `user bob · database shop` (shown above the grid).
    pub fn text(&self) -> String {
        [
            ("user", &self.user),
            ("database", &self.database),
            ("PID", &self.pid),
            ("application", &self.application),
            ("text", &self.text),
        ]
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| format!("{k} {}", v.trim()))
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

/// Columns of the profiler grid: (name, numeric).
const COLUMNS: &[(&str, bool)] = &[
    ("Time", false),
    ("Duration ms", true),
    ("PID", true),
    ("User", false),
    ("Database", false),
    ("Statement", false),
    ("Detail", false),
    ("Kind", false),
    ("Application", false),
    ("Client", false),
];
/// Columns used by the profiler itself; `error` rows (kind) are drawn in the error
/// colour.
const COL_DATABASE: usize = 4;
const COL_STATEMENT: usize = 5;
const COL_DETAIL: usize = 6;
const COL_KIND: usize = 7;

/// `-- parameters: …` / `-- ERROR: …` comment lines, then the statement (D-059).
pub fn statement_text(statement: &str, detail: &str, error: bool) -> String {
    let mut out = String::new();
    for (i, line) in detail.lines().enumerate() {
        out.push_str(if i == 0 && error { "-- ERROR: " } else { "-- " });
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(statement);
    out
}

fn empty_grid() -> ResultSet {
    ResultSet::new(
        COLUMNS
            .iter()
            .map(|(name, numeric)| Column {
                name: (*name).to_owned(),
                type_oid: if *numeric { 701 } else { 25 },
            })
            .collect(),
    )
}

fn grid_row(e: &Entry) -> Vec<pgc_db::Value> {
    let d = e.duration.map(|d| format!("{d:.3}")).unwrap_or_default();
    [
        e.time.as_str(),
        &d,
        &e.pid,
        &e.user,
        &e.database,
        &e.statement,
        &e.detail,
        e.kind.label(),
        &e.application,
        &e.client,
    ]
    .iter()
    .map(|v| Some((*v).into()))
    .collect()
}

/// A setting changed by Start and how Stop puts it back (D-058).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: &'static str,
    pub value: String,
    /// `Some(old)`: `ALTER SYSTEM SET name = old`; `None`: `ALTER SYSTEM RESET name`.
    pub restore: Option<String>,
}

impl Change {
    pub fn set_sql(&self) -> String {
        format!(
            "ALTER SYSTEM SET {} = {}",
            self.name,
            pgc_core::quote::quote_literal(&self.value)
        )
    }

    pub fn restore_sql(&self) -> String {
        match &self.restore {
            Some(old) => format!(
                "ALTER SYSTEM SET {} = {}",
                self.name,
                pgc_core::quote::quote_literal(old)
            ),
            None => format!("ALTER SYSTEM RESET {}", self.name),
        }
    }
}

/// The answer of `check.sql`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub logging_collector: bool,
    /// `log_destination` (raw), where it comes from.
    pub destination: String,
    pub destination_file: String,
    /// `log_min_duration_statement` in ms (raw), where it comes from.
    pub min_duration: String,
    pub min_duration_file: String,
    pub own_pid: String,
}

impl Check {
    pub fn from_row(row: &[String]) -> Option<Self> {
        if row.len() < 6 {
            return None;
        }
        Some(Self {
            logging_collector: row[0] == "on",
            destination: row[1].clone(),
            destination_file: row[2].clone(),
            min_duration: row[3].clone(),
            min_duration_file: row[4].clone(),
            own_pid: row[5].clone(),
        })
    }

    /// The format to read and the settings to change for it (D-058).
    pub fn plan(&self, version: ServerVersion) -> (LogFormat, Vec<Change>) {
        let dests: Vec<String> = self
            .destination
            .split(',')
            .map(|d| d.trim().to_lowercase())
            .filter(|d| !d.is_empty())
            .collect();
        let has = |n: &str| dests.iter().any(|d| d == n);
        let json_ok = version.major() >= 15;
        let restore = |old: &str, file: &str| {
            file.ends_with("postgresql.auto.conf")
                .then(|| old.to_owned())
                // the source file is hidden without the rights to see it: set it back
                .or_else(|| file.is_empty().then(|| old.to_owned()))
        };
        let mut changes = Vec::new();
        let format = if json_ok && has("jsonlog") {
            LogFormat::Json
        } else if has("csvlog") {
            LogFormat::Csv
        } else {
            let format = if json_ok {
                LogFormat::Json
            } else {
                LogFormat::Csv
            };
            let mut value = dests.join(",");
            if !value.is_empty() {
                value.push(',');
            }
            value.push_str(format.name());
            changes.push(Change {
                name: "log_destination",
                value,
                restore: restore(&self.destination, &self.destination_file),
            });
            format
        };
        if self.min_duration.trim() != "0" {
            changes.push(Change {
                name: "log_min_duration_statement",
                value: "0".to_owned(),
                restore: restore(&self.min_duration, &self.min_duration_file),
            });
        }
        (format, changes)
    }
}

/// Where the profiler is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    /// `check.sql` is on its way.
    Checking,
    /// The question about the settings is open.
    Asking,
    /// `ALTER SYSTEM` statements are on their way.
    Applying,
    Running,
    /// The settings are being put back.
    Restoring,
}

/// What follows the restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterRestore {
    Stop,
    /// Quitting pgtc waits for it (D-058).
    Quit,
}

#[derive(Debug)]
pub struct Profiler {
    /// The statement pane below the list (D-059).
    pub pane: crate::dashboard::Pane,
    pub uid: u64,
    pub conn_database: String,
    pub label: String,
    pub conn: Option<DashConn>,
    pub problem: Option<String>,
    pub version: Option<ServerVersion>,
    pub phase: Phase,
    pub own_pid: Option<String>,
    pub reader: Option<LogReader>,
    /// The settings changed by Start (to put back) and the ones planned.
    pub applied: Vec<Change>,
    pub planned: Vec<Change>,
    /// Log file and offset being read (`None`: not known yet — the next read finds the
    /// end of the current file).
    pub file: Option<String>,
    pub offset: u64,
    /// Reads that found no file of the format yet (just after the reload).
    pub missing_reads: u32,
    pub entries: VecDeque<Entry>,
    /// Entries read since Start.
    pub read: u64,
    pub grid: ResultSet,
    pub filter: Filter,
    pub paused: bool,
    /// Requests in flight: a log read; the statements of a step (apply / restore).
    pub read_key: Option<u64>,
    pub step_keys: Vec<u64>,
    pub step_error: Option<String>,
    pub after_restore: AfterRestore,
    pub timer_seq: u64,
    pub error: Option<String>,
}

impl Profiler {
    pub fn new(uid: u64, conn_database: String, label: String, database: Option<String>) -> Self {
        Self {
            uid,
            conn_database,
            label,
            conn: None,
            problem: None,
            version: None,
            phase: Phase::Stopped,
            own_pid: None,
            reader: None,
            applied: Vec::new(),
            planned: Vec::new(),
            file: None,
            offset: 0,
            missing_reads: 0,
            entries: VecDeque::new(),
            read: 0,
            grid: empty_grid(),
            filter: Filter {
                database: database.unwrap_or_default(),
                ..Filter::default()
            },
            paused: false,
            read_key: None,
            step_keys: Vec::new(),
            step_error: None,
            after_restore: AfterRestore::Stop,
            timer_seq: 0,
            error: None,
            pane: crate::dashboard::Pane::default(),
        }
    }

    pub fn connected(&self) -> bool {
        self.conn.as_ref().is_some_and(|c| c.connected)
    }

    /// The tab's own connection, as the three monitor tabs share it.
    pub fn own_conn(&mut self) -> crate::dashboard::OwnConn<'_> {
        crate::dashboard::OwnConn {
            uid: self.uid,
            database: &self.conn_database,
            conn: &mut self.conn,
            version: &mut self.version,
            problem: &mut self.problem,
        }
    }

    /// Settings changed by Start must still be put back.
    pub fn needs_restore(&self) -> bool {
        !self.applied.is_empty()
    }

    /// New log records: the statements and errors (not the profiler's own) become
    /// entries; the grid shows the ones the filter lets through.
    pub fn add_records(&mut self, records: Vec<Record>) {
        let follow = self.grid.rows.is_empty() || self.grid.row + 1 >= self.grid.rows.len();
        let mut rows = Vec::new();
        for r in records {
            if self.own_pid.as_deref() == Some(r.pid.as_str()) {
                continue;
            }
            let Some(e) = entry(r) else {
                continue;
            };
            self.read += 1;
            if self.filter.matches(&e) {
                rows.push(grid_row(&e));
            }
            self.entries.push_back(e);
        }
        if self.entries.len() >= MAX_ENTRIES + DROP_CHUNK {
            self.entries.drain(..DROP_CHUNK);
            self.rebuild();
            return;
        }
        if rows.is_empty() {
            return;
        }
        self.grid.push_rows(rows);
        if follow {
            self.grid.row = self.grid.rows.len() - 1;
        }
    }

    /// Rebuilds the grid from the entries (filter changed, entries dropped).
    pub fn rebuild(&mut self) {
        let (col, left) = (self.grid.col, self.grid.left);
        let mut grid = empty_grid();
        grid.push_rows(
            self.entries
                .iter()
                .filter(|e| self.filter.matches(e))
                .map(grid_row)
                .collect(),
        );
        grid.row = grid.rows.len().saturating_sub(1);
        grid.col = col;
        grid.left = left;
        self.grid = grid;
    }

    /// The statement of the entry under the cursor, with its parameters or error as
    /// `--` comments at the top, and its database (D-059).
    pub fn statement(&self) -> Option<(String, String)> {
        let row = self.grid.rows.get(self.grid.row)?;
        let col = |i: usize| row.get(i).and_then(|v| v.as_deref()).unwrap_or("");
        Some((
            statement_text(
                col(COL_STATEMENT),
                col(COL_DETAIL),
                col(COL_KIND) == "error",
            ),
            col(COL_DATABASE).to_owned(),
        ))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.grid = empty_grid();
    }

    /// `Running · jsonlog · 12 entries shown (40 read)`
    pub fn state_text(&self) -> String {
        let state = match self.phase {
            Phase::Stopped => "Stopped".to_owned(),
            Phase::Checking => "Checking the logging settings…".to_owned(),
            Phase::Asking => "Waiting for your answer…".to_owned(),
            Phase::Applying => "Changing the logging settings…".to_owned(),
            Phase::Restoring => "Restoring the logging settings…".to_owned(),
            Phase::Running if self.paused => "Paused".to_owned(),
            Phase::Running => match &self.reader {
                Some(r) => format!("Running · {}", r.format.name()),
                None => "Running".to_owned(),
            },
        };
        format!(
            "{state} · {} shown ({} read)",
            thousands(self.grid.rows.len()),
            thousands(self.read as usize)
        )
    }
}

/// Click areas of a drawn profiler.
#[derive(Debug, Default, Clone)]
pub struct ProfHits {
    pub grid: Rect,
    pub cells: Vec<crate::grid::CellArea>,
}

/// Draws the profiler into `area`: state line, filter line, separator, grid.
pub fn render(
    frame: &mut Frame,
    p: &Profiler,
    area: Rect,
    theme: &Theme,
    focused: bool,
    separator: &dyn Fn(&mut Frame, Rect),
) -> ProfHits {
    let mut hits = ProfHits::default();
    let normal = theme.get(Role::CoreNormal);
    let hint = theme.get(Role::CoreHint);
    if area.height < 4 {
        return hits;
    }
    let [state, filter, sep, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let state_text = match (&p.problem, p.connected(), &p.conn) {
        (Some(problem), _, _) => format!(" {problem} — F5 connects again."),
        (None, false, Some(_)) => " Connecting…".to_owned(),
        (None, false, None) => " Not connected — F5 connects.".to_owned(),
        (None, true, _) if p.phase == Phase::Stopped && p.read == 0 => format!(
            " {} — F5 starts: every statement on the server is shown from then on.",
            p.state_text()
        ),
        _ => format!(" {}", p.state_text()),
    };
    frame.render_widget(Paragraph::new(state_text).style(normal), state);
    let filter_line = if p.filter.is_empty() {
        Line::from(Span::styled(" No filter (Ctrl+F)", hint))
    } else {
        Line::from(vec![
            Span::styled(" Filter: ", hint),
            Span::styled(p.filter.text(), normal),
        ])
    };
    frame.render_widget(Paragraph::new(filter_line).style(normal), filter);
    separator(frame, sep);
    hits.grid = body;
    if let Some(e) = &p.error {
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
    let styles = crate::dashboard::grid_styles(theme);
    let error = theme.get(Role::MessagesError);
    let row_style = |i: usize| {
        p.grid
            .rows
            .get(i)
            .and_then(|r| r.get(COL_KIND))
            .and_then(|v| v.as_deref())
            .filter(|k| *k == "error")
            .map(|_| error)
    };
    hits.cells = crate::grid::render_styled(
        &p.grid,
        body,
        frame.buffer_mut(),
        &styles,
        focused,
        &row_style,
    );
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv_line(severity: &str, message: &str, detail: &str, query: &str, pid: &str) -> String {
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        format!(
            "2026-11-12 21:51:52.123 CET,\"bob\",\"shop\",{pid},\"10.0.0.5:5050\",sess,1,\"SELECT\",\
             2026-11-12 21:50:00 CET,3/7,0,{severity},00000,{},{},,,,,{},,,\"psql\",client backend,,0\n",
            q(message),
            if detail.is_empty() {
                String::new()
            } else {
                q(detail)
            },
            if query.is_empty() {
                String::new()
            } else {
                q(query)
            },
        )
    }

    #[test]
    fn csv_records_across_reads_and_line_breaks() {
        let mut r = LogReader::new(LogFormat::Csv);
        let text = csv_line(
            "LOG",
            "duration: 0.250 ms  statement: SELECT 'a\"b',\n  2",
            "",
            "",
            "4242",
        ) + &csv_line(
            "LOG",
            "duration: 1.5 ms  execute <unnamed>: SELECT $1",
            "parameters: $1 = '5'",
            "",
            "4242",
        );
        let bytes = text.as_bytes();
        // split inside the quoted line break: nothing complete yet
        let cut = text.find("\n  2").unwrap() + 1;
        assert!(r.feed(&bytes[..cut]).is_empty());
        let recs = r.feed(&bytes[cut..]);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].user, "bob");
        assert_eq!(recs[0].client, "10.0.0.5:5050");
        assert_eq!(recs[0].application, "psql");
        let e = entry(recs[0].clone()).unwrap();
        assert_eq!(e.kind, Kind::Statement);
        assert_eq!(e.duration, Some(0.25));
        assert_eq!(e.statement, "SELECT 'a\"b',\n  2");
        let e = entry(recs[1].clone()).unwrap();
        assert_eq!(e.kind, Kind::Execute);
        assert_eq!(e.detail, "parameters: $1 = '5'");
    }

    #[test]
    fn json_records_and_kinds() {
        let mut r = LogReader::new(LogFormat::Json);
        let line = |sev: &str, msg: &str, stmt: &str| {
            serde_json::json!({
                "timestamp": "2026-11-12 21:51:52.123 CET", "user": "bob", "dbname": "shop",
                "pid": 77, "remote_host": "::1", "remote_port": 6000,
                "error_severity": sev, "message": msg, "statement": stmt,
                "application_name": "app"
            })
            .to_string()
        };
        let text = format!(
            "{}\n{}\n{}\n{}\n{}",
            line("LOG", "duration: 0.1 ms  parse <unnamed>: SELECT 1", ""),
            line("LOG", "duration: 0.1 ms  bind <unnamed>: SELECT 1", ""),
            line("ERROR", "division by zero", "SELECT 1/0"),
            line("LOG", "connection authorized: user=bob", ""),
            line("LOG", "duration: 2 ms  Anweisung: SELECT 2", ""),
        );
        // the last line has no line break yet
        let recs = r.feed(text.as_bytes());
        assert_eq!(recs.len(), 4);
        assert_eq!(recs[0].pid, "77");
        assert_eq!(recs[0].client, "::1:6000");
        let entries: Vec<Entry> = recs.into_iter().filter_map(entry).collect();
        assert_eq!(
            entries.len(),
            1,
            "parse / bind / connection lines are left out"
        );
        assert_eq!(entries[0].kind, Kind::Error);
        assert_eq!(entries[0].statement, "SELECT 1/0");
        assert_eq!(entries[0].detail, "division by zero");
        let recs = r.feed(b"\n");
        let e = entry(recs[0].clone()).unwrap();
        assert_eq!(e.kind, Kind::Other("Anweisung".into()));
        assert_eq!(e.statement, "SELECT 2");
        assert_eq!(e.duration, Some(2.0));
    }

    fn rec(user: &str, pid: &str, sql: &str) -> Record {
        Record {
            user: user.into(),
            database: "shop".into(),
            pid: pid.into(),
            severity: "LOG".into(),
            message: format!("duration: 1 ms  statement: {sql}"),
            ..Record::default()
        }
    }

    #[test]
    fn filters_own_pid_and_follow() {
        let mut p = Profiler::new(1, "postgres".into(), "x".into(), None);
        p.own_pid = Some("1".into());
        p.add_records(vec![
            rec("bob", "1", "SELECT own"),
            rec("bob", "2", "SELECT a"),
            rec("eve", "3", "select B"),
        ]);
        assert_eq!(p.read, 2, "the profiler's own statements are left out");
        assert_eq!(p.grid.rows.len(), 2);
        assert_eq!(p.grid.row, 1, "the cursor follows new rows");
        p.grid.row = 0;
        p.add_records(vec![rec("bob", "2", "SELECT c")]);
        assert_eq!(p.grid.row, 0, "not on the last row: stays");
        p.filter.user = "bob".into();
        p.rebuild();
        assert_eq!(p.grid.rows.len(), 2);
        p.filter = Filter {
            text: "b".into(),
            ..Filter::default()
        };
        p.rebuild();
        assert_eq!(p.grid.rows.len(), 1, "text is case-insensitive");
        assert_eq!(p.filter.text(), "text b");
        p.clear();
        assert!(p.grid.rows.is_empty());
    }

    #[test]
    fn statement_text_with_comments() {
        assert_eq!(statement_text("SELECT 1", "", false), "SELECT 1");
        assert_eq!(
            statement_text("SELECT $1", "parameters: $1 = '5'", false),
            "-- parameters: $1 = '5'\nSELECT $1"
        );
        assert_eq!(
            statement_text("SELECT 1/0", "division by zero\nsecond line", true),
            "-- ERROR: division by zero\n-- second line\nSELECT 1/0"
        );
    }

    #[test]
    fn plan_changes_and_restores() {
        let check = |dest: &str, dest_file: &str, min: &str| Check {
            logging_collector: true,
            destination: dest.into(),
            destination_file: dest_file.into(),
            min_duration: min.into(),
            min_duration_file: "/etc/postgresql/postgresql.conf".into(),
            own_pid: "1".into(),
        };
        let v15 = ServerVersion(150004);
        let v14 = ServerVersion(140010);
        let (f, c) = check("stderr", "", "-1").plan(v15);
        assert_eq!(f, LogFormat::Json);
        assert_eq!(c[0].value, "stderr,jsonlog");
        assert_eq!(
            c[0].restore.as_deref(),
            Some("stderr"),
            "source unknown: set back"
        );
        assert_eq!(c[1].value, "0");
        assert_eq!(c[1].restore, None, "from postgresql.conf: RESET");
        assert_eq!(
            c[1].restore_sql(),
            "ALTER SYSTEM RESET log_min_duration_statement"
        );
        assert_eq!(
            c[0].set_sql(),
            "ALTER SYSTEM SET log_destination = 'stderr,jsonlog'"
        );
        let (f, c) = check("stderr", "/x/postgresql.conf", "0").plan(v14);
        assert_eq!(f, LogFormat::Csv);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].value, "stderr,csvlog");
        assert_eq!(c[0].restore, None);
        let (f, c) = check("csvlog, stderr", "/d/postgresql.auto.conf", "0").plan(v15);
        assert_eq!(f, LogFormat::Csv, "a format already written is used");
        assert!(c.is_empty());
        let from_auto = Check {
            min_duration_file: "/d/postgresql.auto.conf".into(),
            ..check("jsonlog", "", "250")
        };
        let (_, c) = from_auto.plan(v15);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].restore.as_deref(), Some("250"));
        assert_eq!(
            c[0].restore_sql(),
            "ALTER SYSTEM SET log_min_duration_statement = '250'"
        );
    }
}
