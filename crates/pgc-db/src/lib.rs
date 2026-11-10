//! Database layer: connection tasks (tokio-postgres, D-006, D-011), TLS (D-033),
//! `.pgpass` (D-008), command-line connection parameters (D-031). Catalog queries and
//! object definitions follow in Phases 3 and 4.

pub mod connection;
pub mod conninfo;
pub mod definition;
mod error;
pub mod model;
pub mod pgpass;
pub mod query;
mod tls;

pub use connection::{
    Cancel, ConnId, ConnectParams, ConnectionHandle, DbEvent, EventSink, NodeRow, cancel_query,
    spawn,
};
pub use error::{DbError, describe};
pub use query::{
    CatalogQuery, CatalogRequest, Column, ExecuteRequest, QueryEvent, ServerMessage, TxStatus,
    Value,
};
