//! Reading the structure model (D-012, D-048) from a live database with the queries in
//! `sql/model/`. All queries run in one read-only, repeatable-read transaction with
//! `search_path = ''`, so they see one snapshot and every name in the output is
//! schema-qualified (as pg_dump does).

use std::collections::BTreeMap;

use pgc_core::ServerVersion;
use pgc_model::{
    Database, Extension, Function, ObjKind, ObjRef, Schema, Sequence, Table, Type, View,
};
use pgc_sql::{RenderContext, SqlCatalog, SqlError};
use serde::de::DeserializeOwned;

use crate::DbError;

/// The rendered model queries for one server version.
#[derive(Debug, Clone)]
pub struct ModelQueries {
    /// `(file, sql)`; every query returns `schema`, `kind`, `data` (JSON).
    objects: Vec<(&'static str, String)>,
    depends: String,
}

const OBJECT_FILES: &[&str] = &[
    "schemas.sql",
    "types.sql",
    "sequences.sql",
    "tables.sql",
    "views.sql",
    "functions.sql",
];

impl ModelQueries {
    pub fn render(catalog: &SqlCatalog, version: ServerVersion) -> Result<Self, SqlError> {
        let ctx = RenderContext {
            version,
            show_system: false,
        };
        let objects = OBJECT_FILES
            .iter()
            .map(|f| Ok((*f, catalog.render("model", f, ctx)?)))
            .collect::<Result<_, SqlError>>()?;
        Ok(Self {
            objects,
            depends: catalog.render("model", "depends.sql", ctx)?,
        })
    }
}

fn other(file: &str, e: impl std::fmt::Display) -> DbError {
    DbError::Other(format!("structure query {file}: {e}"))
}

/// Reads the structure of the connected database.
pub async fn read(
    client: &tokio_postgres::Client,
    queries: &ModelQueries,
) -> Result<Database, DbError> {
    client
        .batch_execute(
            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL search_path = ''",
        )
        .await
        .map_err(|e| DbError::from_pg(&e))?;
    let result = read_in_transaction(client, queries).await;
    // read only: nothing to keep
    let _ = client.batch_execute("ROLLBACK").await;
    result
}

async fn read_in_transaction(
    client: &tokio_postgres::Client,
    queries: &ModelQueries,
) -> Result<Database, DbError> {
    let mut db = Database::default();
    let mut schemas: BTreeMap<String, Schema> = BTreeMap::new();
    let mut rows_of = Vec::new();
    for (file, sql) in &queries.objects {
        let rows = client
            .query(sql.as_str(), &[])
            .await
            .map_err(|e| other(file, crate::describe(&e)))?;
        for r in rows {
            let get = |c: &str| -> Result<String, DbError> {
                r.try_get::<_, Option<String>>(c)
                    .map_err(|e| other(file, e))
                    .map(Option::unwrap_or_default)
            };
            rows_of.push((*file, get("schema")?, get("kind")?, get("data")?));
        }
    }
    // schemas first, so objects find theirs
    for (file, schema, kind, data) in &rows_of {
        match kind.as_str() {
            "extension" => db.extensions.push(parse::<Extension>(file, data)?),
            "schema" => {
                schemas.insert(schema.clone(), parse(file, data)?);
            }
            _ => {}
        }
    }
    for (file, schema, kind, data) in rows_of {
        let Some(s) = schemas.get_mut(&schema) else {
            continue;
        };
        match kind.as_str() {
            "type" => s.types.push(parse::<Type>(file, &data)?),
            "sequence" => s.sequences.push(parse::<Sequence>(file, &data)?),
            "table" => s.tables.push(parse::<Table>(file, &data)?),
            "view" => s.views.push(parse::<View>(file, &data)?),
            "materialized_view" => s.materialized_views.push(parse::<View>(file, &data)?),
            "function" => s.functions.push(parse::<Function>(file, &data)?),
            _ => {}
        }
    }
    let deps = client
        .query(queries.depends.as_str(), &[])
        .await
        .map_err(|e| other("depends.sql", crate::describe(&e)))?;
    for r in deps {
        let text = |c: &str| -> Option<String> { r.try_get::<_, Option<String>>(c).ok().flatten() };
        let (Some(from), Some(to)) = (
            obj_ref(text("kind"), text("schema"), text("name"), text("args")),
            obj_ref(
                text("ref_kind"),
                text("ref_schema"),
                text("ref_name"),
                text("ref_args"),
            ),
        ) else {
            continue;
        };
        if let Some(s) = schemas.get_mut(&from.schema) {
            add_dependency(s, &from, to);
        }
    }
    db.schemas = schemas.into_values().collect();
    db.normalize();
    Ok(db)
}

fn parse<T: DeserializeOwned>(file: &str, data: &str) -> Result<T, DbError> {
    serde_json::from_str(data).map_err(|e| other(file, format!("{e} in {data}")))
}

fn obj_ref(
    kind: Option<String>,
    schema: Option<String>,
    name: Option<String>,
    args: Option<String>,
) -> Option<ObjRef> {
    let kind = match kind?.as_str() {
        "type" => ObjKind::Type,
        "sequence" => ObjKind::Sequence,
        "table" => ObjKind::Table,
        "view" => ObjKind::View,
        "materialized_view" => ObjKind::MaterializedView,
        "function" => ObjKind::Function,
        _ => return None,
    };
    Some(ObjRef {
        kind,
        schema: schema?,
        name: name?,
        args,
    })
}

/// Records that `from` depends on `to` (objects that are not in the model, e.g.
/// extension members, are skipped).
fn add_dependency(s: &mut Schema, from: &ObjRef, to: ObjRef) {
    /// The dependency list of the object of `items` that `is` picks.
    fn deps_of<T>(
        items: &mut [T],
        is: impl Fn(&T) -> bool,
        deps: impl for<'a> Fn(&'a mut T) -> &'a mut Vec<ObjRef>,
    ) -> Option<&mut Vec<ObjRef>> {
        items.iter_mut().find(|t| is(t)).map(deps)
    }
    let name = from.name.as_str();
    let list = match from.kind {
        ObjKind::Type => deps_of(&mut s.types, |t| t.name == name, |t| &mut t.depends_on),
        ObjKind::Table => deps_of(&mut s.tables, |t| t.name == name, |t| &mut t.depends_on),
        ObjKind::View => deps_of(&mut s.views, |t| t.name == name, |t| &mut t.depends_on),
        ObjKind::MaterializedView => deps_of(
            &mut s.materialized_views,
            |t| t.name == name,
            |t| &mut t.depends_on,
        ),
        ObjKind::Function => deps_of(
            &mut s.functions,
            |f| f.name == name && Some(&f.args) == from.args.as_ref(),
            |f| &mut f.depends_on,
        ),
        ObjKind::Sequence => None,
    };
    if let Some(list) = list {
        list.push(to);
    }
}
