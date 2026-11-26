//! Database layer: connection tasks (tokio-postgres, D-006, D-011), TLS (D-033),
//! `.pgpass` (D-008), command-line connection parameters (D-031), query execution
//! (D-019), object definitions (D-037), the structure model (D-048) and execution plans
//! (D-075).

pub mod connection;
pub mod conninfo;
pub mod definition;
mod error;
pub mod explain;
pub mod model;
pub mod pgpass;
pub mod query;
mod tls;

pub use connection::{
    Cancel, ConnId, ConnectParams, ConnectionHandle, DbEvent, END_GRACE, EventSink, NodeRow,
    cancel_query, spawn,
};
pub use error::{DbError, describe};
pub use explain::{ExplainRequest, Plan};
pub use query::{
    CatalogQuery, CatalogRequest, Column, ExecuteRequest, QueryEvent, ServerMessage, TxStatus,
    Value,
};
