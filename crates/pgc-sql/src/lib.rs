//! Versioned SQL templates (D-010): resolver, minijinja environment with the pgAdmin
//! filters, SQL files embedded from `sql/`. Developer templates (D-013, D-046) are in
//! [`devtemplate`].
//!
//! Layout: `sql/<object>/<version folder>/<file>.sql`, version folders `default` and
//! `NN_plus` (first major version that needs the file). For a server, the newest folder
//! not newer than the server that contains the file wins; `default` is the fallback.
//!
//! Template context: `server_version` (number), `show_system` (bool) and whatever the
//! caller adds. Functions: `has("Feature")` (D-011), `_()` (translation, identity).
//! Filters: `qtIdent`, `qtLiteral`, `qtTypeIdent`, `hasAny`, and `ident` (D-013).
//! Values from the user are passed as bind parameters (`$1`), never via templates.

pub mod devtemplate;

use include_dir::{Dir, include_dir};
use minijinja::value::{Rest, Value};
use minijinja::{Environment, Error as JinjaError, ErrorKind, State, syntax::SyntaxConfig};
use pgc_core::quote::{quote_ident, quote_literal};
use pgc_core::{Feature, ServerVersion};
use thiserror::Error;

static SQL_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../sql");

#[derive(Debug, Error)]
pub enum SqlError {
    #[error("no SQL template {object}/{file} for PostgreSQL {version}")]
    NotFound {
        object: String,
        file: String,
        version: ServerVersion,
    },
    #[error("SQL template {path}: {source}")]
    Render {
        path: String,
        #[source]
        source: JinjaError,
    },
}

/// Variables for rendering a template.
#[derive(Debug, Clone, Copy)]
pub struct RenderContext {
    pub version: ServerVersion,
    pub show_system: bool,
}

/// The embedded SQL templates with a configured minijinja environment.
pub struct SqlCatalog {
    env: Environment<'static>,
}

impl Default for SqlCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlCatalog {
    pub fn new() -> Self {
        let mut env = environment();
        env.set_loader(|name| Ok(SQL_DIR.get_file(name).and_then(|f| f.contents_utf8())));
        Self { env }
    }

    /// Path of the template file for `object`/`file` on a server of `version`.
    pub fn resolve(&self, object: &str, file: &str, version: ServerVersion) -> Option<String> {
        let dir = SQL_DIR.get_dir(object)?;
        let folders: Vec<&str> = dir
            .dirs()
            .filter_map(|d| d.path().file_name()?.to_str())
            .filter(|folder| {
                SQL_DIR
                    .get_file(format!("{object}/{folder}/{file}"))
                    .is_some()
            })
            .collect();
        let folder = pick_folder(&folders, version)?;
        Some(format!("{object}/{folder}/{file}"))
    }

    /// Renders `object`/`file` for the server described by `ctx`.
    pub fn render(&self, object: &str, file: &str, ctx: RenderContext) -> Result<String, SqlError> {
        self.render_with(object, file, ctx, &[])
    }

    /// [`Self::render`] with extra boolean switches in the context (e.g. `plus` for
    /// psql's `+`, D-054). Never user values: those are bind parameters.
    pub fn render_with(
        &self,
        object: &str,
        file: &str,
        ctx: RenderContext,
        vars: &[(&str, bool)],
    ) -> Result<String, SqlError> {
        let path = self
            .resolve(object, file, ctx.version)
            .ok_or_else(|| SqlError::NotFound {
                object: object.to_owned(),
                file: file.to_owned(),
                version: ctx.version,
            })?;
        let err = |source| SqlError::Render {
            path: path.clone(),
            source,
        };
        let template = self.env.get_template(&path).map_err(err)?;
        let mut context = std::collections::BTreeMap::new();
        context.insert("server_version".to_owned(), Value::from(ctx.version.0));
        context.insert("show_system".to_owned(), Value::from(ctx.show_system));
        for (name, on) in vars {
            context.insert((*name).to_owned(), Value::from(*on));
        }
        template.render(Value::from(context)).map_err(err)
    }

    /// All embedded template paths (for tests and diagnostics).
    pub fn all_paths() -> Vec<String> {
        fn walk(dir: &Dir<'static>, out: &mut Vec<String>) {
            for f in dir.files() {
                out.push(f.path().to_string_lossy().replace('\\', "/"));
            }
            for d in dir.dirs() {
                walk(d, out);
            }
        }
        let mut out = Vec::new();
        walk(&SQL_DIR, &mut out);
        out.sort();
        out
    }
}

/// Picks the folder for `version` from the folders that contain the file:
/// the highest `NN_plus` with `NN` ≤ the server's major version, else `default`.
pub fn pick_folder<'a>(folders: &[&'a str], version: ServerVersion) -> Option<&'a str> {
    let major = version.major();
    folders
        .iter()
        .filter_map(|f| {
            let n = devtemplate::folder_major(f)?;
            (n <= major).then_some((n, *f))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, f)| f)
        .or_else(|| folders.iter().copied().find(|f| *f == "default"))
}

/// A minijinja environment as every pgtc template gets it: block tags take their line
/// with them, and the filters and functions of [`register`] are known.
pub(crate) fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_syntax(
        SyntaxConfig::builder()
            .trim_blocks(true)
            .lstrip_blocks(true)
            .build()
            .expect("valid syntax config"),
    );
    register(&mut env);
    env
}

pub(crate) fn register(env: &mut Environment<'_>) {
    // pgAdmin filters (D-010). Extra arguments such as `conn` are accepted and ignored.
    env.add_filter("qtIdent", |value: Value, _rest: Rest<Value>| {
        qt_ident(&value)
    });
    env.add_filter("ident", |value: Value| qt_ident(&value));
    env.add_filter("qtLiteral", |value: Value, _rest: Rest<Value>| {
        if value.is_none() || value.is_undefined() {
            "NULL".to_owned()
        } else if value.kind() == minijinja::value::ValueKind::Bool {
            if value.is_true() { "true" } else { "false" }.to_owned()
        } else if value.kind() == minijinja::value::ValueKind::Number {
            value.to_string()
        } else {
            quote_literal(&value.to_string())
        }
    });
    env.add_filter("qtTypeIdent", |value: Value, _rest: Rest<Value>| {
        qt_type_ident(&value.to_string())
    });
    env.add_filter(
        "hasAny",
        |seq: Value, key: String| -> Result<bool, JinjaError> {
            for item in seq.try_iter()? {
                if item.get_attr(&key).map(|v| v.is_true()).unwrap_or(false) {
                    return Ok(true);
                }
            }
            Ok(false)
        },
    );
    // pgAdmin's qtIdent(conn, a, b, ...) function form: joins the quoted parts.
    env.add_function("qtIdent", |args: Rest<Value>| {
        args.iter()
            .skip(1)
            .filter(|v| !v.is_none() && !v.is_undefined())
            .map(qt_ident)
            .collect::<Vec<_>>()
            .join(".")
    });
    env.add_function("_", |text: String| text);
    env.add_function(
        "has",
        |state: &State, name: String| -> Result<bool, JinjaError> {
            let feature = Feature::from_name(&name).ok_or_else(|| {
                JinjaError::new(
                    ErrorKind::InvalidOperation,
                    format!("unknown feature '{name}'"),
                )
            })?;
            let version = state
                .lookup("server_version")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0);
            Ok(ServerVersion(version).has(feature))
        },
    );
}

fn qt_ident(value: &Value) -> String {
    // a schema-qualified object from a template combo: both parts (D-046)
    // with its arguments or its table: what DROP / ALTER need (D-050)
    if let Some(q) = value.downcast_object_ref::<devtemplate::QualName>() {
        // a built-in type name of the `types` source (the only one without a
        // schema): `integer`, `double precision` are written as they are (D-074)
        if q.schema.is_empty() && q.table.is_none() && q.args.is_none() {
            return q.name.clone();
        }
        let qualified = if q.schema.is_empty() {
            quote_ident(&q.name)
        } else {
            format!("{}.{}", quote_ident(&q.schema), quote_ident(&q.name))
        };
        return match (&q.table, &q.args) {
            (Some(table), _) => format!(
                "{} ON {}.{}",
                quote_ident(&q.name),
                quote_ident(&q.schema),
                quote_ident(table)
            ),
            (None, Some(args)) => format!("{qualified}({args})"),
            (None, None) => qualified,
        };
    }
    quote_ident(&value.to_string())
}

/// pgAdmin's `qtTypeIdent`: a type name is quoted as an identifier, except the names
/// the grammar knows — `integer`, `numeric`, `double precision`, `"char"` — and what
/// is quoted already; array suffixes and a type modifier (`numeric(10,2)`) are kept.
fn qt_type_ident(name: &str) -> String {
    // spelled with a space, or always quoted
    const UNQUOTED: &[&str] = &[
        "bit varying",
        "\"char\"",
        "character varying",
        "double precision",
        "timestamp without time zone",
        "timestamp with time zone",
        "time without time zone",
        "time with time zone",
        "\"trigger\"",
        "\"unknown\"",
    ];
    // the column-name keywords that are type names: quoted they would be other,
    // mostly missing, types ("integer" is not integer)
    const KEYWORD_TYPES: &[&str] = &[
        "bigint",
        "bit",
        "boolean",
        "char",
        "character",
        "dec",
        "decimal",
        "float",
        "int",
        "integer",
        "interval",
        "nchar",
        "numeric",
        "real",
        "smallint",
        "time",
        "timestamp",
        "varchar",
    ];
    let mut base = name;
    let mut suffix = String::new();
    while let Some(b) = base.strip_suffix("[]") {
        base = b;
        suffix.insert_str(0, "[]");
    }
    if base.ends_with(')')
        && let Some(i) = base.find('(')
    {
        suffix.insert_str(0, &base[i..]);
        base = base[..i].trim_end();
    }
    let lower = base.to_lowercase();
    let known = UNQUOTED.contains(&lower.as_str()) || KEYWORD_TYPES.contains(&lower.as_str());
    let quoted = base.len() >= 2 && base.starts_with('"') && base.ends_with('"');
    if known || quoted {
        format!("{base}{suffix}")
    } else {
        format!("{}{suffix}", quote_ident(base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_resolution() {
        let f = ["default", "12_plus", "16_plus"];
        assert_eq!(pick_folder(&f, ServerVersion(110005)), Some("default"));
        assert_eq!(pick_folder(&f, ServerVersion(120000)), Some("12_plus"));
        assert_eq!(pick_folder(&f, ServerVersion(150002)), Some("12_plus"));
        assert_eq!(pick_folder(&f, ServerVersion(190000)), Some("16_plus"));
        assert_eq!(pick_folder(&["13_plus"], ServerVersion(120000)), None);
        assert_eq!(
            pick_folder(&["junk", "default"], ServerVersion(120000)),
            Some("default")
        );
    }

    fn render_str(src: &str, version: u32) -> String {
        let mut env = Environment::new();
        register(&mut env);
        env.render_str(src, minijinja::context! { server_version => version })
            .unwrap()
    }

    #[test]
    fn ident_leaves_a_built_in_type_name_as_it_is() {
        let env = environment();
        let render = |q: devtemplate::QualName| {
            env.render_str(
                "{{ t|ident }}",
                minijinja::context! { t => Value::from_object(q) },
            )
            .unwrap()
        };
        assert_eq!(render(devtemplate::QualName::new("", "integer")), "integer");
        assert_eq!(
            render(devtemplate::QualName::new("", "double precision")),
            "double precision"
        );
        assert_eq!(
            render(devtemplate::QualName::new("sales", "Order Status")),
            "sales.\"Order Status\""
        );
    }

    #[test]
    fn filters_and_functions() {
        assert_eq!(render_str("{{ 'My T'|qtIdent }}", 0), "\"My T\"");
        assert_eq!(render_str("{{ 'abc'|qtIdent(conn) }}", 0), "abc");
        assert_eq!(
            render_str("{{ qtIdent(conn, 'public', 'T') }}", 0),
            "public.\"T\""
        );
        assert_eq!(render_str("{{ \"it's\"|qtLiteral(conn) }}", 0), "'it''s'");
        assert_eq!(render_str("{{ 5|qtLiteral }}", 0), "5");
        assert_eq!(render_str("{{ none|qtLiteral }}", 0), "NULL");
        assert_eq!(
            render_str("{{ 'double precision[]'|qtTypeIdent }}", 0),
            "double precision[]"
        );
        assert_eq!(render_str("{{ 'MyType'|qtTypeIdent }}", 0), "\"MyType\"");
        // names of the grammar, with array suffix and type modifier
        for (given, written) in [
            ("int[]", "int[]"),
            ("integer", "integer"),
            ("numeric(10,2)", "numeric(10,2)"),
            ("character varying(20)[]", "character varying(20)[]"),
            ("\"My Type\"", "\"My Type\""),
            ("My Type[]", "\"My Type\"[]"),
            ("text", "text"),
        ] {
            let template = format!("{{{{ '{given}'|qtTypeIdent }}}}");
            assert_eq!(render_str(&template, 0), written, "{given}");
        }
        assert_eq!(
            render_str("{% if [{'a': 0}, {'a': 1}]|hasAny('a') %}y{% endif %}", 0),
            "y"
        );
        assert_eq!(render_str("{{ _('Hello') }}", 0), "Hello");
        assert_eq!(
            render_str("{% if has('LastSeqScan') %}y{% endif %}", 160000),
            "y"
        );
        assert_eq!(
            render_str("{% if has('LastSeqScan') %}y{% endif %}", 150000),
            ""
        );
    }

    #[test]
    fn every_embedded_template_renders() {
        let catalog = SqlCatalog::new();
        let paths = SqlCatalog::all_paths();
        assert!(paths.iter().any(|p| p.ends_with("nodes.sql")));
        for path in paths
            .iter()
            .filter(|p| p.ends_with(".sql") && !p.starts_with("macros/"))
        {
            let template = catalog.env.get_template(path).unwrap();
            for show_system in [false, true] {
                let sql = template
                    .render(minijinja::context! { server_version => 170000, show_system })
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
                assert!(sql.trim_start().to_uppercase().contains("SELECT"), "{path}");
            }
        }
    }

    #[test]
    fn render_resolves_and_reports_missing() {
        let catalog = SqlCatalog::new();
        let ctx = RenderContext {
            version: ServerVersion(170000),
            show_system: false,
        };
        let sql = catalog.render("databases", "nodes.sql", ctx).unwrap();
        assert!(sql.contains("datistemplate"));
        let sql = catalog
            .render(
                "databases",
                "nodes.sql",
                RenderContext {
                    show_system: true,
                    ..ctx
                },
            )
            .unwrap();
        assert!(!sql.contains("datistemplate"));
        assert!(matches!(
            catalog.render("nope", "nodes.sql", ctx),
            Err(SqlError::NotFound { .. })
        ));
    }
}
