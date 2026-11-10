//! Query execution for query tabs (D-019, D-020): the text is sent as one simple-query
//! request; results stream in batches with backpressure; command tags, errors and the
//! transaction status are reported (patched tokio-postgres, D-040).

use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_postgres::error::ErrorPosition;
use tokio_postgres::{Client, SimpleQueryMessage, SimpleQueryRow, TransactionStatus};

use crate::connection::Request;

/// One execution (F5).
#[derive(Debug, Clone)]
pub struct ExecuteRequest {
    /// Caller's id of this execution; events carry it.
    pub run: u64,
    pub sql: String,
    /// Rows per batch (D-019, default 1,000).
    pub batch_rows: usize,
    /// Auto commit off: send `BEGIN` first when no transaction is open.
    pub auto_begin: bool,
    /// Auto rollback on error: `ROLLBACK` after an error inside a transaction.
    pub auto_rollback: bool,
}

/// Transaction status of a session (D-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    Idle,
    InTransaction,
    Failed,
}

impl From<TransactionStatus> for TxStatus {
    fn from(s: TransactionStatus) -> Self {
        match s {
            TransactionStatus::Idle => TxStatus::Idle,
            TransactionStatus::InTransaction => TxStatus::InTransaction,
            TransactionStatus::Failed => TxStatus::Failed,
        }
    }
}

/// A notice or error from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMessage {
    /// `ERROR`, `NOTICE`, `WARNING`, `INFO`, ...
    pub severity: String,
    /// SQLSTATE.
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    /// CONTEXT (e.g. `PL/pgSQL function inline_code_block line 3 at RAISE`).
    pub context: Option<String>,
    /// 1-based character position in the sent text.
    pub position: Option<u32>,
}

impl ServerMessage {
    pub fn from_db(e: &tokio_postgres::error::DbError) -> Self {
        Self {
            severity: e.severity().to_owned(),
            code: e.code().code().to_owned(),
            message: e.message().to_owned(),
            detail: e.detail().map(str::to_owned),
            hint: e.hint().map(str::to_owned),
            context: e.where_().map(str::to_owned),
            position: match e.position() {
                Some(ErrorPosition::Original(p)) => Some(*p),
                _ => None,
            },
        }
    }

    /// A LISTEN / NOTIFY notification, shown like a notice (D-051):
    /// `NOTIFY:  channel "name" (pid 1234): payload`.
    pub fn notification(channel: &str, pid: i32, payload: &str) -> Self {
        let mut message = format!("channel \"{channel}\" (pid {pid})");
        if !payload.is_empty() {
            message.push_str(": ");
            message.push_str(payload);
        }
        Self {
            severity: "NOTIFY".to_owned(),
            code: String::new(),
            message,
            detail: None,
            hint: None,
            context: None,
            position: None,
        }
    }

    pub fn from_error(e: &tokio_postgres::Error) -> Self {
        match e.as_db_error() {
            Some(db) => Self::from_db(db),
            None => Self {
                severity: "ERROR".to_owned(),
                code: String::new(),
                message: crate::describe(e),
                detail: None,
                hint: None,
                context: None,
                position: None,
            },
        }
    }
}

/// A result column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub type_oid: u32,
}

/// A value as text (`None` = SQL NULL), kept compact (D-019).
pub type Value = Option<Box<str>>;

/// Events of one execution, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryEvent {
    /// A statement returns rows: a new result set starts.
    ResultStart {
        columns: Vec<Column>,
        /// Label of the result set (psql commands, D-054); `None` for SQL.
        title: Option<String>,
    },
    /// A batch of rows of the current result set; `more` = paused, more available
    /// (answer with fetch more / fetch all).
    Rows {
        rows: Vec<Vec<Value>>,
        more: bool,
    },
    /// A statement completed: its command tag (`INSERT 0 5`, `CREATE TABLE`, ...).
    Command {
        tag: String,
    },
    Error(ServerMessage),
    /// Something pgtc did on its own (`BEGIN` for auto commit off, auto rollback).
    Info(String),
    Finished {
        elapsed: Duration,
        status: TxStatus,
    },
}

/// One query of a catalog run (psql-style commands, D-054).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogQuery {
    /// Label of its result set.
    pub title: String,
    pub sql: String,
    /// Text bind parameters (`$1`, ...); every result column must be text.
    pub params: Vec<Option<String>>,
    /// Without rows: this message, and the run stops (e.g. "Did not find …").
    pub stop_if_empty: Option<String>,
    /// Without rows: the result set is left out (describe sections).
    pub skip_empty: bool,
}

/// Catalog queries shown as one run of a query tab (D-054).
#[derive(Debug, Clone)]
pub struct CatalogRequest {
    pub run: u64,
    pub queries: Vec<CatalogQuery>,
}

/// Runs the queries of `req` one after another and reports them like an execution.
pub(crate) async fn catalog(
    client: &Client,
    req: CatalogRequest,
    emit: &(dyn Fn(QueryEvent) + Sync),
) {
    let start = Instant::now();
    for q in req.queries {
        let result = async {
            let stmt = client.prepare(&q.sql).await?;
            let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = q
                .params
                .iter()
                .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
                .collect();
            let rows = client.query(&stmt, &refs).await?;
            let mut values = Vec::with_capacity(rows.len());
            for row in &rows {
                let mut v: Vec<Value> = Vec::with_capacity(row.len());
                for i in 0..row.len() {
                    v.push(row.try_get::<_, Option<String>>(i)?.map(Box::from));
                }
                values.push(v);
            }
            let columns: Vec<Column> = stmt
                .columns()
                .iter()
                .map(|c| Column {
                    name: c.name().to_owned(),
                    type_oid: c.type_().oid(),
                })
                .collect();
            Ok::<_, tokio_postgres::Error>((columns, values))
        }
        .await;
        match result {
            Err(e) => {
                // the position is in pgtc's query, not in the editor text
                let mut m = ServerMessage::from_error(&e);
                m.position = None;
                emit(QueryEvent::Error(m));
                break;
            }
            Ok((_, rows)) if rows.is_empty() && q.stop_if_empty.is_some() => {
                emit(QueryEvent::Info(q.stop_if_empty.unwrap_or_default()));
                break;
            }
            Ok((_, rows)) if rows.is_empty() && q.skip_empty => {}
            Ok((columns, rows)) => {
                let n = rows.len();
                emit(QueryEvent::ResultStart {
                    columns,
                    title: Some(q.title),
                });
                emit(QueryEvent::Rows { rows, more: false });
                emit(QueryEvent::Command {
                    tag: format!("SELECT {n}"),
                });
            }
        }
    }
    // current transaction status (also after an error)
    let _ = client.simple_query("").await;
    emit(QueryEvent::Finished {
        elapsed: start.elapsed(),
        status: client.transaction_status().into(),
    });
}

fn row_values(row: &SimpleQueryRow) -> Vec<Value> {
    (0..row.len()).map(|i| row.get(i).map(Box::from)).collect()
}

/// Runs one execution. Returns a request that arrived while the result was paused
/// and must be handled next (a new execution, close, ...).
pub(crate) async fn execute(
    client: &Client,
    req: ExecuteRequest,
    rx: &mut mpsc::UnboundedReceiver<Request>,
    cancel: &(dyn Fn() + Sync),
    emit: &(dyn Fn(QueryEvent) + Sync),
) -> Option<Request> {
    let start = Instant::now();
    let mut pending = None;
    let mut had_error = false;

    if req.auto_begin && TxStatus::from(client.transaction_status()) == TxStatus::Idle {
        match client.simple_query("BEGIN").await {
            Ok(_) => emit(QueryEvent::Info("BEGIN (auto commit is off)".to_owned())),
            Err(e) => {
                emit(QueryEvent::Error(ServerMessage::from_error(&e)));
                had_error = true;
            }
        }
    }

    if !had_error {
        match client.simple_query_raw(&req.sql).await {
            Err(e) => {
                emit(QueryEvent::Error(ServerMessage::from_error(&e)));
                had_error = true;
            }
            Ok(stream) => {
                let mut stream = Box::pin(stream);
                let mut rows: Vec<Vec<Value>> = Vec::new();
                let mut in_result = false;
                let mut fetch_all = false;
                // After a close / new request while paused: cancel and drop the rest.
                let mut discard = false;
                loop {
                    match stream.next().await {
                        None => break,
                        Some(Err(e)) => {
                            if in_result && !discard {
                                emit(QueryEvent::Rows {
                                    rows: std::mem::take(&mut rows),
                                    more: false,
                                });
                            }
                            if !discard {
                                emit(QueryEvent::Error(ServerMessage::from_error(&e)));
                            }
                            had_error = true;
                            break;
                        }
                        Some(Ok(SimpleQueryMessage::RowDescription(cols))) => {
                            in_result = true;
                            if !discard {
                                emit(QueryEvent::ResultStart {
                                    columns: cols
                                        .iter()
                                        .map(|c| Column {
                                            name: c.name().to_owned(),
                                            type_oid: c.type_oid(),
                                        })
                                        .collect(),
                                    title: None,
                                });
                            }
                        }
                        Some(Ok(SimpleQueryMessage::Row(row))) => {
                            if discard {
                                continue;
                            }
                            rows.push(row_values(&row));
                            if rows.len() >= req.batch_rows.max(1) {
                                emit(QueryEvent::Rows {
                                    rows: std::mem::take(&mut rows),
                                    more: !fetch_all,
                                });
                                if !fetch_all {
                                    // Backpressure (D-019): stop reading until the UI asks.
                                    match rx.recv().await {
                                        Some(Request::FetchMore { run }) if run == req.run => {}
                                        Some(Request::FetchAll { run }) if run == req.run => {
                                            fetch_all = true;
                                        }
                                        Some(Request::Discard { run }) if run == req.run => {
                                            cancel();
                                            discard = true;
                                        }
                                        Some(
                                            Request::FetchMore { .. }
                                            | Request::FetchAll { .. }
                                            | Request::Discard { .. },
                                        ) => {}
                                        other => {
                                            pending = Some(other.unwrap_or(Request::Close));
                                            cancel();
                                            discard = true;
                                        }
                                    }
                                }
                            }
                        }
                        Some(Ok(SimpleQueryMessage::CommandComplete(_))) => {
                            if in_result && !discard {
                                emit(QueryEvent::Rows {
                                    rows: std::mem::take(&mut rows),
                                    more: false,
                                });
                            }
                            in_result = false;
                            if !discard {
                                let tag = stream
                                    .as_ref()
                                    .get_ref()
                                    .command_tag()
                                    .unwrap_or_default()
                                    .to_owned();
                                emit(QueryEvent::Command { tag });
                            }
                        }
                        Some(Ok(_)) => {}
                    }
                }
            }
        }
    }

    // A round trip makes sure the ReadyForQuery after an error has been read, so the
    // transaction status is current.
    let _ = client.simple_query("").await;
    if had_error
        && req.auto_rollback
        && TxStatus::from(client.transaction_status()) != TxStatus::Idle
    {
        match client.simple_query("ROLLBACK").await {
            Ok(_) => emit(QueryEvent::Info(
                "ROLLBACK (auto rollback on error)".to_owned(),
            )),
            Err(e) => emit(QueryEvent::Error(ServerMessage::from_error(&e))),
        }
    }
    emit(QueryEvent::Finished {
        elapsed: start.elapsed(),
        status: client.transaction_status().into(),
    });
    pending
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_reads_like_a_notice() {
        let m = ServerMessage::notification("jobs", 1234, "done 5");
        assert_eq!(m.severity, "NOTIFY");
        assert_eq!(m.message, "channel \"jobs\" (pid 1234): done 5");
        let m = ServerMessage::notification("jobs", 7, "");
        assert_eq!(m.message, "channel \"jobs\" (pid 7)");
    }
}
