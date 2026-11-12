//! State of a query tab (D-019, D-020, D-041): its own connection, results, messages,
//! running execution, transaction status and modes.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pgc_core::settings::SslMode;
use pgc_db::{Cancel, ConnId, ConnectionHandle, QueryEvent, ServerMessage, TxStatus};

use crate::grid::ResultSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryConnState {
    Connecting,
    Connected,
}

/// The tab's own connection (D-011).
#[derive(Debug)]
pub struct QueryConn {
    pub id: ConnId,
    pub database: String,
    pub handle: ConnectionHandle,
    pub cancel: Option<Cancel>,
    pub state: QueryConnState,
    pub ssl_mode: SslMode,
    pub ssl_root_cert: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Notice,
    Command,
    Error,
    Time,
}

/// A line (or block) in the Messages tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: MessageKind,
    pub text: String,
    /// Editor position (char index) to jump to with Enter (errors, D-019).
    pub jump: Option<usize>,
}

/// What was sent, for mapping error positions back to the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    /// Char index in the editor where the executed text starts.
    pub offset: usize,
    /// PL/pgSQL mode: lines added before the user's text (D-020).
    pub plpgsql_lines: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct Running {
    pub started: Instant,
    /// A batch arrived and the server waits for "fetch more" (D-019): the query is
    /// still open, but a new execution may start (it cancels this one).
    pub paused: bool,
    pub cancel_requested: Option<Instant>,
    /// The "close the connection?" question was asked.
    pub asked: bool,
}

/// Maps between editor positions and lines (for error line jumps).
pub struct LineMap<'a> {
    /// Line of a char index.
    pub line_of: &'a dyn Fn(usize) -> usize,
    /// Char index where a line starts.
    pub line_start: &'a dyn Fn(usize) -> usize,
}

/// A running result export (D-021, D-042).
#[derive(Debug)]
pub struct ExportRun {
    pub id: u64,
    /// Result set being exported.
    pub set: usize,
    pub path: PathBuf,
    pub writer: std::sync::mpsc::Sender<crate::export::ToWriter>,
    /// Next grid row to hand to the writer.
    pub next_loaded: usize,
    /// A batch is at the writer.
    pub writing: bool,
    /// A batch was requested from the paused stream for the file.
    pub awaiting_rows: bool,
    /// Rows were taken from the stream (they are not in the grid).
    pub from_stream: bool,
    /// `Finish` was sent.
    pub finishing: bool,
    /// Rows written so far.
    pub written: u64,
    /// Why the export is being aborted (shown when the writer confirms).
    pub abort_reason: Option<String>,
}

/// How an execution ended: shown in the tab status until the next one (D-043).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    pub elapsed: Duration,
    /// An error was reported (this includes a cancelled execution).
    pub failed: bool,
}

impl Finished {
    pub fn text(&self) -> String {
        if self.failed {
            format!("Execution failed after {}", format_duration(self.elapsed))
        } else {
            format!("Execution finished in {}", format_duration(self.elapsed))
        }
    }
}

/// What to do when the current execution has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterRun {
    CloseTab,
    /// Quitting or disconnecting a server waits for this COMMIT (D-043).
    Bulk,
}

#[derive(Debug)]
pub struct QueryState {
    pub conn: Option<QueryConn>,
    /// Names for autocomplete, read through this tab's connection (D-052).
    pub names: crate::completion::Names,
    pub results: Vec<ResultSet>,
    pub active_result: usize,
    pub messages: Vec<Message>,
    pub message_cursor: usize,
    pub show_messages: bool,
    pub results_visible: bool,
    /// Height of the results pane in percent of the tab (D-019 resizable split).
    pub results_percent: u16,
    pub running: Option<Running>,
    /// How the last execution ended, for the tab status (D-043).
    pub finished: Option<Finished>,
    /// Run id of the execution whose result may be paused.
    pub last_run: u64,
    pub tx: TxStatus,
    pub auto_commit: bool,
    pub auto_rollback: bool,
    pub plpgsql: bool,
    pub file: Option<PathBuf>,
    pub sent: Option<Sent>,
    pub after_run: Option<AfterRun>,
    /// Server uid and database of the tab (kept to reconnect, D-041).
    pub binding: Option<(u64, String)>,
    pub export: Option<ExportRun>,
    /// A batch asked for by an export that has ended is still on its way: drop it.
    pub drop_rows: bool,
}

impl QueryState {
    pub fn new(auto_commit: bool, auto_rollback: bool) -> Self {
        Self {
            conn: None,
            names: crate::completion::Names::default(),
            results: Vec::new(),
            active_result: 0,
            messages: Vec::new(),
            message_cursor: 0,
            show_messages: false,
            results_visible: true,
            results_percent: 45,
            running: None,
            finished: None,
            last_run: 0,
            tx: TxStatus::Idle,
            auto_commit,
            auto_rollback,
            plpgsql: false,
            file: None,
            sent: None,
            after_run: None,
            binding: None,
            export: None,
            drop_rows: false,
        }
    }

    /// An execution is working (not just paused at a batch boundary).
    pub fn busy(&self) -> bool {
        self.running.is_some_and(|r| !r.paused)
    }

    pub fn database(&self) -> Option<&str> {
        self.conn.as_ref().map(|c| c.database.as_str())
    }

    pub fn connected(&self) -> bool {
        self.conn
            .as_ref()
            .is_some_and(|c| c.state == QueryConnState::Connected)
    }

    /// The last result set is paused with more rows (D-019).
    pub fn paused(&self) -> bool {
        self.results.last().is_some_and(|r| r.more)
    }

    /// Clears results and messages for a new execution; the first result set will be
    /// shown (D-019).
    pub fn start_run(&mut self, run: u64, sent: Sent) {
        self.results.clear();
        self.active_result = 0;
        self.messages.clear();
        self.message_cursor = 0;
        self.show_messages = false;
        self.running = Some(Running {
            started: Instant::now(),
            paused: false,
            cancel_requested: None,
            asked: false,
        });
        self.finished = None;
        self.last_run = run;
        self.drop_rows = false;
        self.sent = Some(sent);
        self.results_visible = true;
    }

    fn push(&mut self, kind: MessageKind, text: String, jump: Option<usize>) {
        self.messages.push(Message { kind, text, jump });
    }

    /// Applies an event of the running execution. Returns true when it finished.
    pub fn apply(&mut self, event: QueryEvent, lines: &LineMap<'_>) -> bool {
        match event {
            QueryEvent::ResultStart { columns, title } => {
                let mut rs = ResultSet::new(columns);
                rs.title = title;
                self.results.push(rs);
            }
            QueryEvent::Rows { rows, more } => {
                if let Some(rs) = self.results.last_mut() {
                    rs.push_rows(rows);
                    rs.more = more;
                    rs.fetching = false;
                }
                if let Some(r) = self.running.as_mut() {
                    r.paused = more;
                }
            }
            QueryEvent::Command { tag } => {
                // DDL may have changed the names autocomplete offers (D-052)
                let word = tag.split_whitespace().next().unwrap_or("");
                if matches!(word, "CREATE" | "ALTER" | "DROP" | "COMMENT") {
                    self.names.clear();
                }
                self.push(MessageKind::Command, tag, None);
            }
            QueryEvent::Info(text) => self.push(MessageKind::Info, text, None),
            QueryEvent::Error(m) => {
                let jump = self.error_position(&m, lines);
                self.push(MessageKind::Error, format_server_message(&m), jump);
                // D-019: on error the Messages tab is shown
                self.show_messages = true;
                self.message_cursor = self.messages.len() - 1;
            }
            QueryEvent::Finished { elapsed, status } => {
                self.tx = status;
                self.running = None;
                self.finished = Some(Finished {
                    elapsed,
                    failed: self.messages.iter().any(|m| m.kind == MessageKind::Error),
                });
                // a discarded paused result has no more rows
                if let Some(rs) = self.results.last_mut() {
                    rs.more = false;
                    rs.fetching = false;
                }
                // D-019: nothing to show in Results (no result set, not even an empty
                // one) — the Messages tab is shown
                if self.results.is_empty() {
                    self.show_messages = true;
                }
                self.push(
                    MessageKind::Time,
                    format!("Total time: {}", format_duration(elapsed)),
                    None,
                );
                return true;
            }
        }
        false
    }

    pub fn notice(&mut self, m: &ServerMessage) {
        self.push(MessageKind::Notice, format_server_message(m), None);
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(MessageKind::Info, text.into(), None);
    }

    /// Editor char index for an error: SQL mode from the error position, PL/pgSQL
    /// mode from "line N" in the context (D-019, D-020).
    fn error_position(&self, m: &ServerMessage, lines: &LineMap<'_>) -> Option<usize> {
        let sent = self.sent?;
        match sent.plpgsql_lines {
            None => m
                .position
                .map(|p| sent.offset + (p as usize).saturating_sub(1)),
            Some(added) => {
                let ctx = m.context.as_deref()?;
                let line: usize = ctx
                    .split("line ")
                    .nth(1)?
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()?;
                let user_line = line.checked_sub(added + 1)?;
                let base_line = (lines.line_of)(sent.offset);
                Some((lines.line_start)(base_line + user_line))
            }
        }
    }

    /// Text for the tab label: database, mode and transaction state (D-019, D-020).
    pub fn label_suffix(&self) -> String {
        let mut s = String::new();
        if let Some(db) = self.database() {
            s.push_str(&format!(" – {db}"));
        }
        if self.plpgsql {
            s.push_str(" [PL/pgSQL]");
        }
        match self.tx {
            TxStatus::Idle => {}
            TxStatus::InTransaction => s.push_str(" (tx)"),
            TxStatus::Failed => s.push_str(" (tx failed)"),
        }
        s
    }
}

/// `ERROR:  message` with DETAIL / HINT / CONTEXT lines (psql style).
pub fn format_server_message(m: &ServerMessage) -> String {
    let mut s = format!("{}:  {}", m.severity, m.message);
    if let Some(d) = &m.detail {
        s.push_str(&format!("\nDETAIL:  {d}"));
    }
    if let Some(h) = &m.hint {
        s.push_str(&format!("\nHINT:  {h}"));
    }
    if let Some(c) = &m.context {
        s.push_str(&format!("\nCONTEXT:  {c}"));
    }
    s
}

pub fn format_duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.3} s", d.as_secs_f64())
    }
}

/// PL/pgSQL mode (D-020): wraps the text in a `DO` block; `BEGIN … END` is added when
/// the text has none. Returns the SQL and the number of lines before the user's text
/// inside the DO body (for error line mapping).
pub fn wrap_plpgsql(text: &str) -> Result<(String, usize), String> {
    if text.contains("$pgc$") {
        return Err("The text contains $pgc$, which PL/pgSQL mode uses as its quote.".to_owned());
    }
    let words: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let has_block = matches!(words.first().map(String::as_str), Some("declare" | "begin"))
        && words.last().map(String::as_str) == Some("end");
    if has_block {
        Ok((format!("DO $pgc$\n{text}\n$pgc$;"), 1))
    } else {
        Ok((format!("DO $pgc$\nBEGIN\n{text}\nEND\n$pgc$;"), 2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgc_db::Column;

    fn msg(position: Option<u32>, context: Option<&str>) -> ServerMessage {
        ServerMessage {
            severity: "ERROR".into(),
            code: "42703".into(),
            message: "column x does not exist".into(),
            detail: None,
            hint: Some("try y".into()),
            context: context.map(str::to_owned),
            position,
        }
    }

    #[test]
    fn plpgsql_wrapping() {
        let (sql, n) = wrap_plpgsql("RAISE NOTICE 'x';").unwrap();
        assert_eq!(sql, "DO $pgc$\nBEGIN\nRAISE NOTICE 'x';\nEND\n$pgc$;");
        assert_eq!(n, 2);
        let (sql, n) = wrap_plpgsql("DECLARE a int;\nBEGIN\n a := 1;\nEND;").unwrap();
        assert!(sql.starts_with("DO $pgc$\nDECLARE"));
        assert_eq!(n, 1);
        assert!(wrap_plpgsql("select '$pgc$'").is_err());
    }

    #[test]
    fn events_build_results_and_messages() {
        let mut q = QueryState::new(true, false);
        q.start_run(
            7,
            Sent {
                offset: 10,
                plpgsql_lines: None,
            },
        );
        let zero = |_: usize| 0;
        let line_start = LineMap {
            line_of: &zero,
            line_start: &zero,
        };
        q.apply(
            QueryEvent::ResultStart {
                columns: vec![Column {
                    name: "a".into(),
                    type_oid: 23,
                }],
                title: None,
            },
            &line_start,
        );
        q.apply(
            QueryEvent::Rows {
                rows: vec![vec![Some("1".into())]; 3],
                more: true,
            },
            &line_start,
        );
        assert!(q.paused());
        q.apply(
            QueryEvent::Command {
                tag: "SELECT 3".into(),
            },
            &line_start,
        );
        q.apply(QueryEvent::Error(msg(Some(5), None)), &line_start);
        assert!(q.show_messages, "errors switch to Messages");
        assert_eq!(q.messages[1].jump, Some(14), "offset 10 + position 5 - 1");
        assert!(q.messages[1].text.contains("HINT:  try y"));
        let done = q.apply(
            QueryEvent::Finished {
                elapsed: Duration::from_millis(12),
                status: TxStatus::Failed,
            },
            &line_start,
        );
        assert!(done && q.running.is_none());
        assert_eq!(q.messages.last().unwrap().text, "Total time: 12 ms");
        assert_eq!(
            q.finished.unwrap().text(),
            "Execution failed after 12 ms",
            "the run had an error"
        );
        assert!(q.label_suffix().contains("(tx failed)"));
    }

    /// A run without any result set shows Messages; an empty SELECT keeps Results.
    #[test]
    fn runs_without_result_sets_show_messages() {
        let zero = |_: usize| 0;
        let lines = LineMap {
            line_of: &zero,
            line_start: &zero,
        };
        let sent = Sent {
            offset: 0,
            plpgsql_lines: None,
        };
        let finished = QueryEvent::Finished {
            elapsed: Duration::from_millis(1),
            status: TxStatus::Idle,
        };
        let mut q = QueryState::new(true, false);
        q.start_run(1, sent);
        q.apply(
            QueryEvent::Command {
                tag: "INSERT 0 5".into(),
            },
            &lines,
        );
        assert!(!q.show_messages, "only when the run has finished");
        q.apply(finished.clone(), &lines);
        assert!(q.show_messages, "no result set: Messages");

        // the next run starts on Results; SELECT with 0 rows is a result set
        q.start_run(2, sent);
        assert!(!q.show_messages);
        q.apply(
            QueryEvent::ResultStart {
                columns: vec![Column {
                    name: "a".into(),
                    type_oid: 23,
                }],
                title: None,
            },
            &lines,
        );
        q.apply(
            QueryEvent::Command {
                tag: "SELECT 0".into(),
            },
            &lines,
        );
        q.apply(finished, &lines);
        assert!(!q.show_messages, "an empty grid stays on Results");
    }

    #[test]
    fn plpgsql_errors_map_to_editor_lines() {
        let mut q = QueryState::new(true, false);
        q.start_run(
            1,
            Sent {
                offset: 0,
                plpgsql_lines: Some(2),
            },
        );
        // editor lines start at 0, 10, 20, ...
        let line_of = |c: usize| c / 10;
        let start = |l: usize| l * 10;
        let line_start = LineMap {
            line_of: &line_of,
            line_start: &start,
        };
        q.apply(
            QueryEvent::Error(msg(
                Some(1),
                Some("PL/pgSQL function inline_code_block line 4 at RAISE"),
            )),
            &line_start,
        );
        // body line 4 = user line 2 (body line 1 is empty, line 2 is BEGIN)
        assert_eq!(q.messages[0].jump, Some(10));
    }
}
