//! The execution plan of a logged statement (D-075): plain `EXPLAIN`, never
//! `ANALYZE` — the statement is planned, not executed.

use tokio_postgres::types::private::BytesMut;
use tokio_postgres::types::{Format, IsNull, ToSql, Type, to_sql_checked};

use crate::DbError;

/// What [`crate::ConnectionHandle::explain`] asks for.
#[derive(Debug)]
pub struct ExplainRequest {
    pub key: u64,
    /// One statement, as the server logged it.
    pub statement: String,
    /// The logged values of its `$n` parameters in text form (`None` = NULL); empty
    /// when the log has none.
    pub params: Vec<Option<String>>,
    /// The server knows `EXPLAIN (GENERIC_PLAN)` (PostgreSQL 16).
    pub generic: bool,
}

/// The plan of a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The lines of `EXPLAIN`'s output.
    pub lines: Vec<String>,
    /// A generic plan: the parameter values were not known.
    pub generic: bool,
}

/// A parameter value in the text form the server logged, for a parameter of any type:
/// the server reads it with the type's input function, as it would a literal.
#[derive(Debug)]
struct TextParam<'a>(Option<&'a str>);

impl ToSql for TextParam<'_> {
    fn to_sql(
        &self,
        _ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self.0 {
            Some(text) => {
                out.extend_from_slice(text.as_bytes());
                Ok(IsNull::No)
            }
            None => Ok(IsNull::Yes),
        }
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }

    fn encode_format(&self, _ty: &Type) -> Format {
        Format::Text
    }

    to_sql_checked!();
}

/// Plans `req.statement` in a read-only transaction that is rolled back.
///
/// The statement goes through the extended protocol first: the server refuses a text
/// with more than one statement there, and says how many parameters it has. With the
/// logged values the plan is the one for those values; without them (not logged, or
/// cut by `log_parameter_max_length`) it is the generic plan, where the server has
/// one.
pub(crate) async fn explain(
    client: &tokio_postgres::Client,
    req: &ExplainRequest,
) -> Result<Plan, DbError> {
    client
        .batch_execute("BEGIN READ ONLY")
        .await
        .map_err(|e| DbError::from_pg(&e))?;
    let result = plan(client, req).await;
    if let Err(e) = client.batch_execute("ROLLBACK").await {
        tracing::warn!("ending an EXPLAIN: {}", crate::describe(&e));
    }
    result
}

async fn plan(client: &tokio_postgres::Client, req: &ExplainRequest) -> Result<Plan, DbError> {
    let prepared = client
        .prepare(&format!("EXPLAIN {}", req.statement))
        .await
        .map_err(|e| DbError::from_pg(&e))?;
    let wanted = prepared.params().len();
    let known = wanted == 0 || (req.params.len() == wanted && !values_cut(client, req).await);
    if known {
        let values: Vec<TextParam> = req.params.iter().map(|p| TextParam(p.as_deref())).collect();
        let rows = client
            .query(&prepared, &crate::connection::bind(&values))
            .await
            .map_err(|e| DbError::from_pg(&e))?;
        let lines = rows
            .iter()
            .map(|r| r.try_get::<_, String>(0))
            .collect::<Result<_, _>>()
            .map_err(|e| DbError::Other(format!("EXPLAIN: {e}")))?;
        return Ok(Plan {
            lines,
            generic: false,
        });
    }
    if !req.generic {
        return Err(DbError::Other(
            "The statement has parameters and the log does not have their values (see \
             log_parameter_max_length). A plan without the values is the generic plan, \
             which needs PostgreSQL 16."
                .to_owned(),
        ));
    }
    // One statement, as the server has just accepted it as a prepared statement: the
    // simple protocol cannot run a second one here.
    let messages = client
        .simple_query(&format!("EXPLAIN (GENERIC_PLAN) {}", req.statement))
        .await
        .map_err(|e| DbError::from_pg(&e))?;
    let lines = messages
        .iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => Some(r.get(0).unwrap_or("").to_owned()),
            _ => None,
        })
        .collect();
    Ok(Plan {
        lines,
        generic: true,
    })
}

/// A logged value may be cut: `log_parameter_max_length` is not -1 and a value ends
/// with the `...` the server puts there. Such a value is not the one that was bound.
async fn values_cut(client: &tokio_postgres::Client, req: &ExplainRequest) -> bool {
    if !req
        .params
        .iter()
        .flatten()
        .any(|value| value.ends_with("..."))
    {
        return false;
    }
    // the setting is not there before PostgreSQL 13: nothing is cut
    let row = client
        .query_one(
            "SELECT current_setting('log_parameter_max_length', true)",
            &[],
        )
        .await;
    match row.map(|r| r.try_get::<_, Option<String>>(0)) {
        Ok(Ok(Some(limit))) => limit != "-1",
        Ok(Ok(None)) => false,
        other => {
            tracing::warn!("reading log_parameter_max_length: {other:?}");
            true
        }
    }
}
