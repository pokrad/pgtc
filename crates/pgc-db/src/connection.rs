//! One tokio task per connection (D-006, D-011): the task owns the
//! `tokio_postgres::Client`; requests come in through a channel and events go out
//! through an [`EventSink`]. The UI never waits on the database.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use pgc_core::ServerVersion;
use pgc_core::settings::{SavedConnection, SslMode};
use tokio::sync::mpsc;
use tokio_postgres::types::ToSql;
use tokio_postgres::{AsyncMessage, CancelToken};

use crate::query::{self, ExecuteRequest, QueryEvent, ServerMessage};
use crate::{DbError, tls};

/// Identifier of a connection, chosen by the caller.
pub type ConnId = u64;

/// Everything needed to open one connection.
#[derive(Clone)]
pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
    pub ssl_mode: SslMode,
    pub ssl_root_cert: Option<String>,
    pub connect_timeout: Duration,
}

impl std::fmt::Debug for ConnectParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectParams")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("ssl_mode", &self.ssl_mode)
            .finish()
    }
}

impl ConnectParams {
    /// Parameters for `database` on the server of `saved`.
    pub fn new(
        saved: &SavedConnection,
        database: &str,
        password: Option<String>,
        connect_timeout: Duration,
    ) -> Self {
        Self {
            host: saved.host.clone(),
            port: saved.port,
            database: database.to_owned(),
            username: saved.username.clone(),
            password,
            ssl_mode: saved.ssl_mode,
            ssl_root_cert: saved.ssl_root_cert.clone(),
            connect_timeout,
        }
    }
}

/// Requests to a connection task.
#[derive(Debug)]
pub(crate) enum Request {
    Nodes {
        key: u64,
        sql: String,
        parent: Option<u32>,
    },
    Definition {
        key: u64,
        sql: String,
        params: Vec<u32>,
    },
    Execute(ExecuteRequest),
    /// psql-style command: catalog queries as one run (D-054).
    Catalog(crate::query::CatalogRequest),
    FetchMore {
        run: u64,
    },
    FetchAll {
        run: u64,
    },
    /// Ends a paused result: its statement is cancelled or, when that would abort a
    /// transaction, its remaining rows are read and dropped (D-019, D-042).
    Discard {
        run: u64,
    },
    /// Rows of a catalog query as text (template choice lists D-046, autocomplete
    /// D-052); `params` are text bind parameters (`$1`, ...).
    Lookup {
        key: u64,
        sql: String,
        params: Vec<Option<String>>,
    },
    /// The structure model of the connected database (D-048).
    ReadModel {
        key: u64,
        queries: Box<crate::model::ModelQueries>,
    },
    Close,
}

/// One row of a tree query (`sql/*/nodes.sql`, D-036).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRow {
    pub oid: u32,
    pub name: String,
    /// Text shown instead of the name (e.g. a function signature).
    pub label: Option<String>,
    /// Partitioned table (has a Partitions group).
    pub partitioned: bool,
}

/// Events from connection tasks.
#[derive(Debug)]
pub enum DbEvent {
    Connected {
        id: ConnId,
        version: ServerVersion,
        cancel: Cancel,
    },
    ConnectFailed {
        id: ConnId,
        error: DbError,
    },
    /// Answer to [`ConnectionHandle::load_nodes`] with the caller's `key`.
    Nodes {
        id: ConnId,
        key: u64,
        result: Result<Vec<NodeRow>, DbError>,
    },
    /// Answer to [`ConnectionHandle::load_definition`]: formatted SQL text (D-037).
    Definition {
        id: ConnId,
        key: u64,
        result: Result<String, DbError>,
    },
    /// An event of execution `run` (query tabs, D-019).
    Query {
        id: ConnId,
        run: u64,
        event: QueryEvent,
    },
    /// Answer to [`ConnectionHandle::lookup`]: rows as text (NULL = empty).
    Lookup {
        id: ConnId,
        key: u64,
        result: Result<Vec<Vec<String>>, DbError>,
    },
    /// Answer to [`ConnectionHandle::read_model`] (D-048).
    Model {
        id: ConnId,
        key: u64,
        result: Result<Box<pgc_model::Database>, DbError>,
    },
    /// A notice from the server (`RAISE NOTICE`, warnings, ...).
    Notice {
        id: ConnId,
        notice: ServerMessage,
    },
    /// The connection ended; `error` is set if it was not closed on request.
    Closed {
        id: ConnId,
        error: Option<DbError>,
    },
}

/// Cancels the running query of a connection from outside its task (D-006, D-019).
#[derive(Clone)]
pub struct Cancel(pub CancelToken);

impl std::fmt::Debug for Cancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancel")
    }
}

/// Receives events; called from the connection tasks.
pub type EventSink = Arc<dyn Fn(DbEvent) + Send + Sync>;

/// Handle to a connection task. Dropping it closes the connection.
#[derive(Debug, Clone)]
pub struct ConnectionHandle {
    pub id: ConnId,
    tx: mpsc::UnboundedSender<Request>,
    abort: tokio::task::AbortHandle,
}

impl ConnectionHandle {
    /// A handle without a connection behind it (requests are dropped). Only for
    /// tests and the README screenshots; needs a tokio runtime.
    #[doc(hidden)]
    pub fn detached(id: ConnId) -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        let abort = tokio::spawn(async {}).abort_handle();
        Self { id, tx, abort }
    }

    /// Runs a tree query (`$1` = `parent` when given); answered with
    /// [`DbEvent::Nodes`] carrying `key`.
    pub fn load_nodes(&self, key: u64, sql: String, parent: Option<u32>) {
        let _ = self.tx.send(Request::Nodes { key, sql, parent });
    }

    /// Runs a `definition.sql` query (`$1`, `$2`, ... = `params` as OIDs); answered
    /// with [`DbEvent::Definition`] carrying `key`.
    pub fn load_definition(&self, key: u64, sql: String, params: Vec<u32>) {
        let _ = self.tx.send(Request::Definition { key, sql, params });
    }

    /// Runs catalog queries as one execution (psql commands, D-054); answered with
    /// [`DbEvent::Query`] events like [`Self::execute`].
    pub fn catalog(&self, request: crate::query::CatalogRequest) {
        let _ = self.tx.send(Request::Catalog(request));
    }

    /// Executes query text (query tabs); answered with [`DbEvent::Query`] events.
    pub fn execute(&self, request: ExecuteRequest) {
        let _ = self.tx.send(Request::Execute(request));
    }

    /// Continues a paused result with the next batch.
    pub fn fetch_more(&self, run: u64) {
        let _ = self.tx.send(Request::FetchMore { run });
    }

    /// Continues a paused result without pausing again.
    pub fn fetch_all(&self, run: u64) {
        let _ = self.tx.send(Request::FetchAll { run });
    }

    /// Runs a read-only catalog query (fixed SQL or a template's own `query`); answered
    /// with [`DbEvent::Lookup`] carrying `key`.
    pub fn lookup(&self, key: u64, sql: String) {
        self.lookup_with(key, sql, Vec::new());
    }

    /// [`Self::lookup`] with text bind parameters; the query's columns must be text.
    pub fn lookup_with(&self, key: u64, sql: String, params: Vec<Option<String>>) {
        let _ = self.tx.send(Request::Lookup { key, sql, params });
    }

    /// Reads the structure model of the connected database (D-048); answered with
    /// [`DbEvent::Model`] carrying `key`.
    pub fn read_model(&self, key: u64, queries: crate::model::ModelQueries) {
        let _ = self.tx.send(Request::ReadModel {
            key,
            queries: Box::new(queries),
        });
    }

    /// Ends a paused result without reading its remaining rows into the UI: the
    /// statement is cancelled when nothing can be lost, otherwise the connection reads
    /// and drops the rows (D-019). [`QueryEvent::Finished`] follows when the run ends.
    pub fn discard(&self, run: u64) {
        let _ = self.tx.send(Request::Discard { run });
    }

    /// Closes the connection; answered with [`DbEvent::Closed`].
    pub fn close(&self) {
        let _ = self.tx.send(Request::Close);
    }

    /// Ends the connection at once, even while a query hangs (no event follows).
    pub fn abort(&self) {
        self.abort.abort();
    }
}

/// How long a cancel request sent in the background may take.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);

/// Sends PostgreSQL's cancel request and waits until the server has closed the cancel
/// connection: the request is then processed (vendored tokio-postgres, D-040).
/// Failures and a missing answer after `timeout` are logged.
async fn send_cancel(
    token: CancelToken,
    ssl_mode: SslMode,
    root_cert: Option<&str>,
    timeout: Duration,
) {
    let connector = match tls::connector(ssl_mode, root_cert) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("cancel request: {e}");
            return;
        }
    };
    match tokio::time::timeout(timeout, token.cancel_query(connector)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("cancel request failed: {e}"),
        Err(_) => tracing::warn!("cancel request: no answer after {timeout:?}"),
    }
}

/// Sends PostgreSQL's cancel request for the running query (D-019). Runs in the
/// background; failures are logged.
pub fn cancel_query(cancel: &Cancel, ssl_mode: SslMode, root_cert: Option<String>) {
    let token = cancel.0.clone();
    tokio::spawn(async move {
        send_cancel(token, ssl_mode, root_cert.as_deref(), CANCEL_TIMEOUT).await;
    });
}

/// The cancel request of a connection, for its own task: awaited, so the next request
/// is sent only after the server has processed the cancel (D-019).
pub(crate) struct Canceller {
    token: CancelToken,
    ssl_mode: SslMode,
    root_cert: Option<String>,
    timeout: Duration,
}

impl Canceller {
    pub(crate) async fn cancel(&self) {
        send_cancel(
            self.token.clone(),
            self.ssl_mode,
            self.root_cert.as_deref(),
            self.timeout,
        )
        .await;
    }
}

/// Aborts the connection driver when the task ends (also when it is aborted).
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Starts a connection task. Must be called inside a tokio runtime.
pub fn spawn(id: ConnId, params: ConnectParams, sink: EventSink) -> ConnectionHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(id, params, rx, sink));
    ConnectionHandle {
        id,
        tx,
        abort: task.abort_handle(),
    }
}

/// `values` as bind parameters (`$1`, ...).
pub(crate) fn bind<T: ToSql + Sync>(values: &[T]) -> Vec<&(dyn ToSql + Sync)> {
    values.iter().map(|v| v as &(dyn ToSql + Sync)).collect()
}

async fn load_definition(
    client: &tokio_postgres::Client,
    sql: &str,
    params: &[u32],
) -> Result<String, DbError> {
    let row = client
        .query_opt(sql, &bind(params))
        .await
        .map_err(|e| DbError::from_pg(&e))?
        .ok_or_else(|| {
            DbError::Other("The object was not found; it may have been dropped.".to_owned())
        })?;
    let text = |col: &str| -> Result<Option<String>, DbError> {
        row.try_get(col)
            .map_err(|e| DbError::Other(format!("definition query: {e}")))
    };
    Ok(crate::definition::format(
        &text("kind")?.unwrap_or_default(),
        &text("name")?.unwrap_or_default(),
        text("drop")?.as_deref(),
        &text("body")?.unwrap_or_default(),
    ))
}

async fn load_nodes(
    client: &tokio_postgres::Client,
    sql: &str,
    parent: Option<u32>,
) -> Result<Vec<NodeRow>, DbError> {
    let rows = match parent {
        Some(oid) => client.query(sql, &[&oid]).await,
        None => client.query(sql, &[]).await,
    }
    .map_err(|e| DbError::from_pg(&e))?;
    rows.iter()
        .map(|r| {
            Ok(NodeRow {
                oid: r
                    .try_get("oid")
                    .map_err(|e| DbError::Other(format!("tree query: {e}")))?,
                name: r
                    .try_get("name")
                    .map_err(|e| DbError::Other(format!("tree query: {e}")))?,
                label: r.try_get("label").ok().flatten(),
                partitioned: r
                    .try_get::<_, Option<bool>>("partitioned")
                    .ok()
                    .flatten()
                    .unwrap_or(false),
            })
        })
        .collect()
}

async fn run(
    id: ConnId,
    params: ConnectParams,
    mut rx: mpsc::UnboundedReceiver<Request>,
    sink: EventSink,
) {
    tracing::info!(id, ?params, "connecting");
    let connector = match tls::connector(params.ssl_mode, params.ssl_root_cert.as_deref()) {
        Ok(c) => c,
        Err(error) => {
            sink(DbEvent::ConnectFailed { id, error });
            return;
        }
    };
    let mut config = tokio_postgres::Config::new();
    config
        .host(&params.host)
        .port(params.port)
        .dbname(&params.database)
        .user(&params.username)
        .application_name(pgc_core::APP_NAME)
        .connect_timeout(params.connect_timeout)
        .ssl_mode(match params.ssl_mode {
            SslMode::Disable => tokio_postgres::config::SslMode::Disable,
            SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
            SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => {
                tokio_postgres::config::SslMode::Require
            }
        });
    if let Some(pw) = &params.password {
        config.password(pw);
    }

    let (client, connection) = match config.connect(connector).await {
        Ok(pair) => pair,
        Err(e) => {
            let error = DbError::from_pg(&e);
            tracing::info!(id, "connect failed: {error}");
            sink(DbEvent::ConnectFailed { id, error });
            return;
        }
    };
    // The driver forwards server notices (D-019 Messages tab).
    let notice_sink = sink.clone();
    let mut connection = connection;
    let mut driver = tokio::spawn(async move {
        let mut messages = futures::stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(message) = messages.next().await {
            match message {
                Ok(AsyncMessage::Notice(n)) => notice_sink(DbEvent::Notice {
                    id,
                    notice: ServerMessage::from_db(&n),
                }),
                // LISTEN / NOTIFY: into the Messages tab like a notice (D-051)
                Ok(AsyncMessage::Notification(n)) => notice_sink(DbEvent::Notice {
                    id,
                    notice: ServerMessage::notification(n.channel(), n.process_id(), n.payload()),
                }),
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    });
    let _driver_guard = AbortOnDrop(driver.abort_handle());
    let canceller = Canceller {
        token: client.cancel_token(),
        ssl_mode: params.ssl_mode,
        root_cert: params.ssl_root_cert.clone(),
        timeout: params.connect_timeout,
    };

    let version = match client
        .query_one("SELECT current_setting('server_version_num')::int4", &[])
        .await
    {
        Ok(row) => ServerVersion(row.get::<_, i32>(0).max(0) as u32),
        Err(e) => {
            sink(DbEvent::ConnectFailed {
                id,
                error: DbError::from_pg(&e),
            });
            return;
        }
    };
    if !version.is_supported() {
        sink(DbEvent::ConnectFailed {
            id,
            error: DbError::Other(format!(
                "PostgreSQL {version} is not supported; the oldest supported version is {}.",
                pgc_core::version::MIN_SUPPORTED.major()
            )),
        });
        return;
    }
    tracing::info!(id, %version, "connected");
    sink(DbEvent::Connected {
        id,
        version,
        cancel: Cancel(client.cancel_token()),
    });

    // A request that arrived while a result was paused is handled next.
    let mut pending: Option<Request> = None;
    loop {
        let request = match pending.take() {
            Some(r) => Some(r),
            None => tokio::select! {
                request = rx.recv() => request,
                ended = &mut driver => {
                    let error = match ended {
                        Ok(Err(e)) => DbError::from_pg(&e),
                        Err(e) => DbError::Other(e.to_string()),
                        Ok(Ok(())) => DbError::Other("connection closed by the server".to_owned()),
                    };
                    tracing::info!(id, "connection lost: {error}");
                    sink(DbEvent::Closed { id, error: Some(error) });
                    return;
                }
            },
        };
        match request {
            None | Some(Request::Close) => break,
            Some(Request::Nodes { key, sql, parent }) => {
                let result = load_nodes(&client, &sql, parent).await;
                sink(DbEvent::Nodes { id, key, result });
            }
            Some(Request::Definition { key, sql, params }) => {
                let result = load_definition(&client, &sql, &params).await;
                sink(DbEvent::Definition { id, key, result });
            }
            Some(Request::Lookup { key, sql, params }) => {
                let result = if params.is_empty() {
                    lookup(&client, &sql).await
                } else {
                    lookup_params(&client, &sql, &params).await
                };
                sink(DbEvent::Lookup { id, key, result });
            }
            Some(Request::ReadModel { key, queries }) => {
                let result = crate::model::read(&client, &queries).await.map(Box::new);
                sink(DbEvent::Model { id, key, result });
            }
            Some(Request::Catalog(req)) => {
                let run = req.run;
                let emit = |event| sink(DbEvent::Query { id, run, event });
                query::catalog(&client, req, &emit).await;
            }
            Some(Request::Execute(req)) => {
                let run = req.run;
                tracing::debug!(id, run, "execute");
                let emit = |event| sink(DbEvent::Query { id, run, event });
                pending = query::execute(&client, req, &mut rx, &canceller, &emit).await;
            }
            // stale: the execution they belong to has finished
            Some(
                Request::FetchMore { .. } | Request::FetchAll { .. } | Request::Discard { .. },
            ) => {}
        }
    }
    drop(client);
    let _ = driver.await;
    tracing::info!(id, "disconnected");
    sink(DbEvent::Closed { id, error: None });
}

/// Rows of a query with text parameters; every column must be text (NULL = empty).
async fn lookup_params(
    client: &tokio_postgres::Client,
    sql: &str,
    params: &[Option<String>],
) -> Result<Vec<Vec<String>>, DbError> {
    let rows = client
        .query(sql, &bind(params))
        .await
        .map_err(|e| DbError::from_pg(&e))?;
    rows.iter()
        .map(|r| {
            (0..r.len())
                .map(|i| {
                    r.try_get::<_, Option<String>>(i)
                        .map(Option::unwrap_or_default)
                        .map_err(|e| DbError::Other(format!("lookup query: {e}")))
                })
                .collect()
        })
        .collect()
}

/// Rows of a simple query as text; the first result set only.
async fn lookup(client: &tokio_postgres::Client, sql: &str) -> Result<Vec<Vec<String>>, DbError> {
    let messages = client
        .simple_query(sql)
        .await
        .map_err(|e| DbError::Other(crate::describe(&e)))?;
    let mut rows = Vec::new();
    for m in messages {
        match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => rows.push(
                (0..r.len())
                    .map(|i| r.get(i).unwrap_or("").to_owned())
                    .collect(),
            ),
            tokio_postgres::SimpleQueryMessage::CommandComplete(_) => break,
            _ => {}
        }
    }
    Ok(rows)
}
