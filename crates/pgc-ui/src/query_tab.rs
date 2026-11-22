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
    /// Editor position (char index) to jump to with Enter (errors, notices, D-019).
    pub jump: Option<usize>,
    /// The jump has a column: the token there is selected.
    pub exact: bool,
}

/// What was sent, for mapping message positions back to the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// Char index in the editor where the executed text starts.
    pub offset: usize,
    /// The sent text.
    pub sql: std::sync::Arc<str>,
    /// Chars pgtc added before the user's text (PL/pgSQL mode, D-020).
    pub prefix: usize,
    /// Chars of the user's text.
    pub user_len: usize,
    /// The statements of `sql`, split when a message first needs them.
    pub statements: std::cell::OnceCell<crate::message_pos::Statements>,
}

impl Sent {
    /// `sql` from editor char index `offset` on, as typed.
    pub fn new(offset: usize, sql: &str) -> Self {
        Self {
            offset,
            sql: sql.into(),
            prefix: 0,
            user_len: sql.chars().count(),
            statements: std::cell::OnceCell::new(),
        }
    }

    /// Editor char index of char `pos` of the sent text (none on pgtc's added text).
    fn editor_pos(&self, pos: usize) -> Option<usize> {
        let user = pos.checked_sub(self.prefix)?;
        (user <= self.user_len).then_some(self.offset + user)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Running {
    pub started: Instant,
    /// A batch arrived and the server waits for "fetch more" (D-019): the query is
    /// still open, but a new execution may start (it cancels this one).
    pub paused: bool,
    /// The paused result was closed and the connection is ending it: cancelling its
    /// statement, or reading the remaining rows when a cancel would abort a
    /// transaction (D-019). Over when that statement has completed.
    pub closing: bool,
    pub cancel_requested: Option<Instant>,
    /// The "close the connection?" question was asked.
    pub asked: bool,
}

/// Editor lines (for the `LINE n:` of messages).
pub struct LineMap<'a> {
    /// Line of a char index.
    pub line_of: &'a dyn Fn(usize) -> usize,
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
    /// The file was read with CR LF line ends: it is saved with them (D-064).
    pub crlf: bool,
    /// The text is being written to a file (D-072); a save asked for meanwhile waits
    /// in `save_next`.
    pub saving: bool,
    pub save_next: Option<PathBuf>,
    pub sent: Option<Sent>,
    /// Statements of the run that have completed (the running one is the next).
    pub completed: usize,
    /// `Ctrl+A` in Messages: copy copies all of them (D-019).
    pub messages_all_selected: bool,
    pub after_run: Option<AfterRun>,
    /// Server uid and database of the tab (kept to reconnect, D-041).
    pub binding: Option<(u64, String)>,
    pub export: Option<ExportRun>,
    /// A batch asked for by an export that has ended is still on its way: drop it.
    pub drop_rows: bool,
    /// Execute the text as soon as the tab's connection is open (D-056).
    pub run_on_connect: bool,
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
            crlf: false,
            saving: false,
            save_next: None,
            sent: None,
            completed: 0,
            messages_all_selected: false,
            after_run: None,
            binding: None,
            export: None,
            drop_rows: false,
            run_on_connect: false,
        }
    }

    /// An execution is working (not just paused at a batch boundary).
    /// The Messages tab is the one shown: chosen, or no result set to show (D-066).
    pub fn on_messages(&self) -> bool {
        self.show_messages || self.results.is_empty()
    }

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

    /// Closes the paused result (D-019): the connection ends its statement; until
    /// then the tab counts as running, so `Alt+Shift+C` is a real cancel.
    pub fn close_paused(&mut self) {
        let Some(conn) = self.conn.as_ref() else {
            return;
        };
        conn.handle.discard(self.last_run);
        if let Some(r) = self.running.as_mut() {
            r.paused = false;
            r.closing = true;
        }
        if let Some(rs) = self.results.last_mut() {
            rs.more = false;
            rs.fetching = false;
        }
    }

    /// The tab's connection is gone (lost, or closed after a cancel without reaction):
    /// nothing runs any more, a paused result has no more rows, and an export that
    /// still needed rows from the stream is aborted — its file would be incomplete
    /// (D-042).
    pub fn connection_gone(&mut self) {
        self.conn = None;
        self.running = None;
        self.tx = TxStatus::Idle;
        self.drop_rows = false;
        let last = self.results.len().saturating_sub(1);
        if let Some(e) = self.export.as_mut() {
            let from_stream = e.awaiting_rows
                || (e.set == last && self.results.get(e.set).is_some_and(|rs| rs.more));
            e.awaiting_rows = false;
            if from_stream && e.abort_reason.is_none() {
                e.abort_reason = Some("The session was lost during the export".to_owned());
                let _ = e.writer.send(crate::export::ToWriter::Abort);
            }
        }
        if let Some(rs) = self.results.last_mut() {
            rs.more = false;
            rs.fetching = false;
        }
    }

    /// Clears results and messages for a new execution; the first result set will be
    /// shown (D-019).
    pub fn start_run(&mut self, run: u64, sent: Sent) {
        self.results.clear();
        self.active_result = 0;
        self.messages.clear();
        self.message_cursor = 0;
        self.messages_all_selected = false;
        self.completed = 0;
        self.show_messages = false;
        self.running = Some(Running {
            started: Instant::now(),
            paused: false,
            closing: false,
            cancel_requested: None,
            asked: false,
        });
        self.finished = None;
        self.last_run = run;
        self.drop_rows = false;
        self.sent = Some(sent);
        self.results_visible = true;
    }

    fn push(&mut self, kind: MessageKind, text: String) {
        self.messages.push(Message {
            kind,
            text,
            jump: None,
            exact: false,
        });
    }

    /// A server message with its editor position and `LINE n:` text (D-019).
    fn push_server(&mut self, kind: MessageKind, m: &ServerMessage, lines: &LineMap<'_>) {
        let place = self.sent.as_ref().and_then(|sent| {
            let at = crate::message_pos::locate_in(&sent.sql, m, self.completed, &sent.statements)?;
            let pos = sent.editor_pos(at.pos)?;
            let line_no = (lines.line_of)(pos) + 1;
            let text = crate::message_pos::line_text(&sent.sql, at.pos, at.exact, line_no);
            Some((pos, at.exact, text))
        });
        let (jump, exact, line) = match place {
            Some((pos, exact, text)) => (Some(pos), exact, Some(text)),
            None => (None, false, None),
        };
        self.messages.push(Message {
            kind,
            text: format_message(m, line.as_deref()),
            jump,
            exact,
        });
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
                // a batch asked for before the result was closed does not pause again
                let more = more && !self.running.is_some_and(|r| r.closing);
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
                // the statement of a closed result has ended; the script goes on
                if let Some(r) = self.running.as_mut() {
                    r.closing = false;
                }
                self.completed += 1;
                self.push(MessageKind::Command, tag);
            }
            QueryEvent::Info(text) => self.push(MessageKind::Info, text),
            QueryEvent::Error(m) => {
                self.push_server(MessageKind::Error, &m, lines);
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
                // D-066: without a result set (not even an empty one) only Messages
                // is there, so it is shown
                self.push(
                    MessageKind::Time,
                    format!("Total time: {}", format_duration(elapsed)),
                );
                return true;
            }
        }
        false
    }

    pub fn notice(&mut self, m: &ServerMessage, lines: &LineMap<'_>) {
        self.push_server(MessageKind::Notice, m, lines);
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(MessageKind::Info, text.into());
    }

    /// Text copied from Messages: all of them after `Ctrl+A`, else the one under the
    /// cursor (D-019).
    pub fn messages_copy_text(&self) -> Option<String> {
        if self.messages_all_selected {
            return self.all_messages_text();
        }
        self.messages
            .get(self.message_cursor)
            .map(|m| m.text.clone())
    }

    pub fn all_messages_text(&self) -> Option<String> {
        let texts: Vec<&str> = self.messages.iter().map(|m| m.text.as_str()).collect();
        (!texts.is_empty()).then(|| texts.join("\n"))
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

/// `ERROR:  message` with psql's `LINE n:` text (if any) and DETAIL / HINT / CONTEXT
/// lines (psql style).
fn format_message(m: &ServerMessage, line: Option<&str>) -> String {
    let mut s = format!("{}:  {}", m.severity, m.message);
    if let Some(l) = line {
        s.push('\n');
        s.push_str(l);
    }
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
/// the text has none. Returns the SQL and the number of chars before the user's text
/// (for message positions).
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
        Ok((format!("DO $pgc$\n{text}\n$pgc$;"), 9))
    } else {
        Ok((format!("DO $pgc$\nBEGIN\n{text}\nEND\n$pgc$;"), 15))
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
            hint: Some("try y".into()),
            context: context.map(str::to_owned),
            position,
            ..ServerMessage::default()
        }
    }

    #[test]
    fn plpgsql_wrapping() {
        let (sql, n) = wrap_plpgsql("RAISE NOTICE 'x';").unwrap();
        assert_eq!(sql, "DO $pgc$\nBEGIN\nRAISE NOTICE 'x';\nEND\n$pgc$;");
        assert_eq!(&sql[n..], "RAISE NOTICE 'x';\nEND\n$pgc$;");
        let (sql, n) = wrap_plpgsql("DECLARE a int;\nBEGIN\n a := 1;\nEND;").unwrap();
        assert!(sql.starts_with("DO $pgc$\nDECLARE"));
        assert_eq!(&sql[n..n + 7], "DECLARE");
        assert!(wrap_plpgsql("select '$pgc$'").is_err());
    }

    #[test]
    fn events_build_results_and_messages() {
        let mut q = QueryState::new(true, false);
        q.start_run(7, Sent::new(10, "select x from t"));
        let zero = |_: usize| 0;
        let line_start = LineMap { line_of: &zero };
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
        assert!(q.messages[1].exact);
        assert_eq!(
            q.messages[1].text,
            "ERROR:  column x does not exist\nLINE 1: select x from t\n            ^\nHINT:  try y"
        );
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
        let lines = LineMap { line_of: &zero };
        let sent = Sent::new(0, "select 1");
        let finished = QueryEvent::Finished {
            elapsed: Duration::from_millis(1),
            status: TxStatus::Idle,
        };
        let mut q = QueryState::new(true, false);
        q.start_run(1, sent.clone());
        q.apply(
            QueryEvent::Command {
                tag: "INSERT 0 5".into(),
            },
            &lines,
        );
        // no result set: only Messages is there (D-066)
        assert!(q.on_messages());
        q.apply(finished.clone(), &lines);
        assert!(q.on_messages(), "no result set: Messages");

        // the first result set is shown when it arrives; SELECT with 0 rows is one
        q.start_run(2, sent);
        assert!(q.on_messages());
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
        assert!(!q.on_messages(), "an empty grid is a result set");
    }

    /// D-019: a closed paused result counts as running until its statement has ended
    /// (so the cancel key is a real cancel), and a batch that was already on its way
    /// does not pause it again.
    #[tokio::test]
    async fn closing_a_paused_result() {
        let zero = |_: usize| 0;
        let lines = LineMap { line_of: &zero };
        let columns = || QueryEvent::ResultStart {
            columns: vec![Column {
                name: "a".into(),
                type_oid: 23,
            }],
            title: None,
        };
        let batch = |more| QueryEvent::Rows {
            rows: vec![vec![Some("1".into())]; 2],
            more,
        };
        let mut q = QueryState::new(true, false);
        q.conn = Some(QueryConn {
            id: 1,
            database: "db".into(),
            handle: ConnectionHandle::detached(1),
            cancel: None,
            state: QueryConnState::Connected,
            ssl_mode: SslMode::Prefer,
            ssl_root_cert: None,
        });
        q.start_run(1, Sent::new(0, "select 1"));
        q.apply(columns(), &lines);
        q.apply(batch(true), &lines);
        assert!(q.paused() && !q.busy());

        q.close_paused();
        assert!(!q.paused(), "no more rows are offered");
        assert!(q.busy(), "running until the connection has ended it");
        assert!(q.running.unwrap().closing);
        // a batch asked for before the result was closed
        q.apply(batch(true), &lines);
        assert!(!q.paused() && q.busy(), "it does not pause again");
        assert_eq!(q.results[0].rows.len(), 4);

        // the closed statement ends; the next result of the script pauses as usual
        q.apply(batch(false), &lines);
        q.apply(
            QueryEvent::Command {
                tag: "SELECT 500000".into(),
            },
            &lines,
        );
        assert!(!q.running.unwrap().closing);
        q.apply(columns(), &lines);
        q.apply(batch(true), &lines);
        assert!(q.paused() && !q.busy());
    }

    #[test]
    fn plpgsql_errors_map_to_editor_lines() {
        let text = "x := 1;\nRAISE NOTICE 'a';";
        let (sql, prefix) = wrap_plpgsql(text).unwrap();
        let mut q = QueryState::new(true, false);
        q.start_run(
            1,
            Sent {
                offset: 100,
                sql: sql.as_str().into(),
                prefix,
                user_len: text.chars().count(),
                statements: std::cell::OnceCell::new(),
            },
        );
        let line_of = |c: usize| text[..c - 100].matches('\n').count();
        let lines = LineMap { line_of: &line_of };
        let ctx = |n: usize| format!("PL/pgSQL function inline_code_block line {n} at RAISE");
        // body line 4 = user line 2 (body line 1 is empty, line 2 is BEGIN)
        q.notice(&msg(None, Some(&ctx(4))), &lines);
        assert_eq!(q.messages[0].kind, MessageKind::Notice);
        assert_eq!(q.messages[0].jump, Some(108));
        assert!(!q.messages[0].exact);
        assert!(q.messages[0].text.contains("\nLINE 2: RAISE NOTICE 'a';\n"));
        // the BEGIN pgtc added: no jump, no LINE
        q.apply(QueryEvent::Error(msg(None, Some(&ctx(2)))), &lines);
        assert_eq!(q.messages[1].jump, None);
        assert!(!q.messages[1].text.contains("LINE"));
    }

    /// A notice of a `DO` in SQL mode jumps to its line in the running `DO` (D-019).
    #[test]
    fn notices_of_a_do_in_sql_mode() {
        let sql =
            "DO $$ BEGIN RAISE NOTICE 'one'; END $$;\nDO $$\nBEGIN\n  RAISE NOTICE 'two';\nEND $$;";
        let mut q = QueryState::new(true, false);
        q.start_run(1, Sent::new(0, sql));
        let line_of = |c: usize| sql[..c].matches('\n').count();
        let lines = LineMap { line_of: &line_of };
        let ctx = |n: usize| ServerMessage {
            severity: "NOTICE".into(),
            message: "x".into(),
            context: Some(format!(
                "PL/pgSQL function inline_code_block line {n} at RAISE"
            )),
            ..ServerMessage::default()
        };
        q.notice(&ctx(1), &lines);
        assert_eq!(q.messages[0].jump, Some(6), "first DO: BEGIN of its line 1");
        q.apply(QueryEvent::Command { tag: "DO".into() }, &lines);
        q.notice(&ctx(3), &lines);
        let jump = q.messages[2].jump.unwrap();
        assert!(sql[jump..].starts_with("RAISE NOTICE 'two'"));
        assert!(
            q.messages[2]
                .text
                .starts_with("NOTICE:  x\nLINE 4:   RAISE")
        );

        // copy: the message under the cursor, or all after Ctrl+A
        q.message_cursor = 1;
        assert_eq!(q.messages_copy_text().as_deref(), Some("DO"));
        q.messages_all_selected = true;
        let all = q.messages_copy_text().unwrap();
        assert_eq!(all.lines().filter(|l| l.starts_with("NOTICE")).count(), 2);
    }
}
