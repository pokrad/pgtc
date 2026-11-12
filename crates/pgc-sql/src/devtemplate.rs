//! Developer templates (D-013, D-046): `.pgtemplate` files with a TOML header between
//! `+++` lines and a Jinja body.
//!
//! Template directories have version folders (`default/`, `16_plus/`, ...); inside a
//! version folder, sub-folders are categories (`default/Functions/function.pgtemplate`).
//! Sources are scanned in precedence order — user directory, built-in directory, and
//! the embedded templates when the built-in directory does not exist — and for each
//! template and version folder the first source wins.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use include_dir::{Dir, include_dir};
use minijinja::Environment;
use minijinja::syntax::SyntaxConfig;
use minijinja::value::{Object, ObjectRepr, Value};
use pgc_core::ServerVersion;
use serde::Deserialize;

use crate::{pick_folder, register};

static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../templates");

/// File extension of developer templates.
pub const EXTENSION: &str = "pgtemplate";

/// Live list of database objects for a `choice` parameter (D-046, D-050).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Schemas,
    Tables,
    /// Plain views (materialized views have their own source).
    Views,
    MaterializedViews,
    Sequences,
    /// Functions with their argument types (`schema.name(args)`).
    Functions,
    Procedures,
    /// Data types for columns: common built-in types plus the database's own types.
    Types,
    /// Types created in the database (enum, composite, range, base); no domains.
    UserTypes,
    Domains,
    /// Indexes that can be dropped on their own (not those of constraints or
    /// partitions).
    Indexes,
    /// Triggers with their table (`name ON schema.table`).
    Triggers,
    Roles,
}

/// How the rows of a choice list become values (D-050).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One column: a plain name (schemas, roles, custom queries).
    Name,
    /// `schema`, `name`.
    Qualified,
    /// `schema`, `name`, argument types (functions, procedures).
    WithArgs,
    /// `schema`, `table`, `name` (triggers).
    OnTable,
}

/// Filter shared by the source queries: no system schemas.
macro_rules! user_ns {
    () => {
        " AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
         AND n.nspname !~ '^pg_(toast|temp)' "
    };
}

/// Not an extension member (`$catalog` and `$oid` name the object's row).
macro_rules! not_ext {
    ($catalog:literal, $oid:literal) => {
        concat!(
            "AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
             WHERE d.classid = '",
            $catalog,
            "'::pg_catalog.regclass AND d.objid = ",
            $oid,
            " AND d.deptype = 'e') "
        )
    };
}

macro_rules! relations {
    ($kinds:literal) => {
        concat!(
            "SELECT n.nspname, c.relname FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN (",
            $kinds,
            ")",
            user_ns!(),
            not_ext!("pg_catalog.pg_class", "c.oid"),
            "ORDER BY 1, 2"
        )
    };
}

macro_rules! routines {
    ($kind:literal) => {
        concat!(
            "SELECT n.nspname, p.proname, \
             pg_catalog.pg_get_function_identity_arguments(p.oid) \
             FROM pg_catalog.pg_proc p \
             JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
             WHERE p.prokind = '",
            $kind,
            "'",
            user_ns!(),
            not_ext!("pg_catalog.pg_proc", "p.oid"),
            "ORDER BY 1, 2, 3"
        )
    };
}

impl Source {
    /// The values are schema-qualified objects (`schema` + `name`).
    pub fn qualified(self) -> bool {
        self.shape() != Shape::Name
    }

    pub fn shape(self) -> Shape {
        match self {
            Source::Schemas | Source::Roles => Shape::Name,
            Source::Functions | Source::Procedures => Shape::WithArgs,
            Source::Triggers => Shape::OnTable,
            _ => Shape::Qualified,
        }
    }

    /// Query listing the values, columns as in [`Shape`]. Only objects the user can
    /// see; system schemas and extension members are left out.
    pub fn sql(self) -> &'static str {
        match self {
            Source::Schemas => concat!(
                "SELECT n.nspname FROM pg_catalog.pg_namespace n WHERE true",
                user_ns!(),
                not_ext!("pg_catalog.pg_namespace", "n.oid"),
                "ORDER BY 1"
            ),
            Source::Roles => "SELECT rolname FROM pg_catalog.pg_roles ORDER BY 1",
            Source::Tables => relations!("'r', 'p', 'f'"),
            Source::Views => relations!("'v'"),
            Source::MaterializedViews => relations!("'m'"),
            Source::Sequences => relations!("'S'"),
            Source::Functions => routines!("f"),
            Source::Procedures => routines!("p"),
            // common built-in types (no schema) plus the database's own types
            Source::Types => concat!(
                "SELECT '' AS nspname, t FROM unnest(ARRAY['bigint', 'boolean', \
                 'bytea', 'date', 'double precision', 'integer', 'interval', 'json', \
                 'jsonb', 'numeric', 'real', 'smallint', 'text', 'time', 'timestamp', \
                 'timestamptz', 'uuid', 'varchar']) t \
                 UNION ALL \
                 (SELECT n.nspname, t.typname FROM pg_catalog.pg_type t \
                 JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
                 WHERE t.typtype IN ('b', 'c', 'd', 'e', 'r') AND t.typelem = 0 \
                 AND (t.typrelid = 0 OR EXISTS (SELECT 1 FROM pg_catalog.pg_class c \
                 WHERE c.oid = t.typrelid AND c.relkind = 'c'))",
                user_ns!(),
                "ORDER BY 1, 2)"
            ),
            // range types own their multirange (14+, internal dependency): only the
            // range is listed
            Source::UserTypes => concat!(
                "SELECT n.nspname, t.typname FROM pg_catalog.pg_type t \
                 JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
                 WHERE t.typtype IN ('b', 'c', 'e', 'r', 'm') AND t.typelem = 0 \
                 AND (t.typrelid = 0 OR EXISTS (SELECT 1 FROM pg_catalog.pg_class c \
                 WHERE c.oid = t.typrelid AND c.relkind = 'c'))",
                user_ns!(),
                not_ext!("pg_catalog.pg_type", "t.oid"),
                "AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
                 WHERE d.classid = 'pg_catalog.pg_type'::pg_catalog.regclass \
                 AND d.objid = t.oid AND d.deptype = 'i') \
                 ORDER BY 1, 2"
            ),
            Source::Domains => concat!(
                "SELECT n.nspname, t.typname FROM pg_catalog.pg_type t \
                 JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
                 WHERE t.typtype = 'd'",
                user_ns!(),
                not_ext!("pg_catalog.pg_type", "t.oid"),
                "ORDER BY 1, 2"
            ),
            // not the index of a constraint and not a partition's part of an index
            Source::Indexes => concat!(
                "SELECT n.nspname, c.relname FROM pg_catalog.pg_index i \
                 JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k \
                 WHERE k.conindid = i.indexrelid AND k.contype IN ('p', 'u', 'x')) \
                 AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_inherits h \
                 WHERE h.inhrelid = i.indexrelid)",
                user_ns!(),
                not_ext!("pg_catalog.pg_class", "c.oid"),
                "ORDER BY 1, 2"
            ),
            // a partition's clone of a trigger depends on its parent ('P'; internal
            // before 13) and cannot be dropped on its own
            Source::Triggers => concat!(
                "SELECT n.nspname, c.relname, g.tgname FROM pg_catalog.pg_trigger g \
                 JOIN pg_catalog.pg_class c ON c.oid = g.tgrelid \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE NOT g.tgisinternal \
                 AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
                 WHERE d.classid = 'pg_catalog.pg_trigger'::pg_catalog.regclass \
                 AND d.objid = g.oid AND d.deptype = 'P')",
                user_ns!(),
                "ORDER BY 1, 2, 3"
            ),
        }
    }
}

/// Parameter types (D-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    Text,
    Identifier,
    Number,
    Bool,
    Choice,
    Sql,
    List,
}

/// One parameter of a template.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ParamType,
    pub label: Option<String>,
    #[serde(default)]
    pub required: bool,
    pub default: Option<toml::Value>,
    pub help: Option<String>,
    /// `choice`: fixed options.
    #[serde(default)]
    pub options: Vec<String>,
    /// `choice`: live list from the database.
    pub source: Option<Source>,
    /// `choice`: custom query; the first column holds the values.
    pub query: Option<String>,
    /// `list`: the fields of one row.
    #[serde(default)]
    pub fields: Vec<Param>,
}

impl Param {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// The default as text (for inputs and combos).
    pub fn default_text(&self) -> String {
        match &self.default {
            None => String::new(),
            Some(toml::Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
        }
    }

    pub fn default_bool(&self) -> bool {
        matches!(self.default, Some(toml::Value::Boolean(true)))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    name: Option<String>,
    description: Option<String>,
    /// Accepted for compatibility; the folder is the category (D-046).
    #[serde(default)]
    #[allow(dead_code)]
    category: Option<String>,
    #[serde(default, rename = "param")]
    params: Vec<Param>,
}

/// A parsed `.pgtemplate` file.
#[derive(Debug, Clone)]
pub struct TemplateFile {
    pub name: Option<String>,
    pub description: String,
    pub params: Vec<Param>,
    pub body: String,
}

/// Parses a template file: `+++`, TOML header, `+++`, Jinja body.
pub fn parse(text: &str) -> Result<TemplateFile, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "+++" {
        return Err("the file must start with a line `+++`".to_owned());
    }
    let mut header = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "+++" {
            closed = true;
            break;
        }
        header.push_str(line);
    }
    if !closed {
        return Err("the header has no closing line `+++`".to_owned());
    }
    let body: String = lines.collect();
    let h: Header = toml::from_str(&header).map_err(|e| format!("header: {e}"))?;
    check_params(&h.params, false)?;
    Ok(TemplateFile {
        name: h.name.filter(|n| !n.trim().is_empty()),
        description: h.description.unwrap_or_default(),
        params: h.params,
        body,
    })
}

fn check_params(params: &[Param], in_list: bool) -> Result<(), String> {
    let mut seen = Vec::new();
    for p in params {
        let n = p.name.trim();
        if n.is_empty() || !n.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(format!("parameter name '{}' is not a valid name", p.name));
        }
        if seen.contains(&n) {
            return Err(format!("parameter '{n}' is declared twice"));
        }
        seen.push(n);
        match p.kind {
            ParamType::Choice => {
                let kinds = usize::from(!p.options.is_empty())
                    + usize::from(p.source.is_some())
                    + usize::from(p.query.is_some());
                if kinds != 1 {
                    return Err(format!(
                        "choice '{n}' needs exactly one of `options`, `source`, `query`"
                    ));
                }
            }
            ParamType::List if in_list => {
                return Err(format!("list '{n}' cannot be inside another list"));
            }
            ParamType::List if p.fields.is_empty() => {
                return Err(format!("list '{n}' needs `fields`"));
            }
            ParamType::List => check_params(&p.fields, true)?,
            _ => {}
        }
    }
    Ok(())
}

// ---- values -------------------------------------------------------------------------

/// A schema-qualified object chosen in a combo (D-046): `{{ v.schema }}`,
/// `{{ v.name }}`; `{{ v|ident }}` quotes both parts; `{{ v }}` is `schema.name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualName {
    pub schema: String,
    pub name: String,
    /// Argument types of a function or procedure (`integer, text`), as the server
    /// prints them (D-050).
    pub args: Option<String>,
    /// Table of a trigger (same schema as `schema`).
    pub table: Option<String>,
}

impl QualName {
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
            args: None,
            table: None,
        }
    }
}

/// `schema.name`, `schema.name(args)` or `name ON schema.table`; also the label in
/// the dialog's list.
impl fmt::Display for QualName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(table) = &self.table {
            return write!(f, "{} ON {}.{}", self.name, self.schema, table);
        }
        if self.schema.is_empty() {
            f.write_str(&self.name)?;
        } else {
            write!(f, "{}.{}", self.schema, self.name)?;
        }
        if let Some(args) = &self.args {
            write!(f, "({args})")?;
        }
        Ok(())
    }
}

impl Object for QualName {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "schema" => Some(Value::from(self.schema.clone())),
            "name" => Some(Value::from(self.name.clone())),
            "args" => self.args.clone().map(Value::from),
            "table" => self.table.clone().map(Value::from),
            _ => None,
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

/// A value entered in the dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Text(String),
    Bool(bool),
    Object(QualName),
    List(Vec<BTreeMap<String, ParamValue>>),
}

impl ParamValue {
    fn to_jinja(&self, kind: ParamType) -> Value {
        match self {
            ParamValue::Text(s) if kind == ParamType::Number => {
                if let Ok(i) = s.trim().parse::<i64>() {
                    Value::from(i)
                } else if let Ok(f) = s.trim().parse::<f64>() {
                    Value::from(f)
                } else {
                    Value::from(s.clone())
                }
            }
            ParamValue::Text(s) => Value::from(s.clone()),
            ParamValue::Bool(b) => Value::from(*b),
            ParamValue::Object(q) => Value::from_object(q.clone()),
            ParamValue::List(rows) => Value::from(
                rows.iter()
                    .map(|row| {
                        Value::from(
                            row.iter()
                                .map(|(k, v)| (k.clone(), v.to_jinja(ParamType::Text)))
                                .collect::<BTreeMap<_, _>>(),
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            ParamValue::Text(s) => s.trim().is_empty(),
            ParamValue::Bool(_) => false,
            ParamValue::Object(q) => q.name.is_empty(),
            ParamValue::List(rows) => rows.is_empty(),
        }
    }
}

/// Checks required values, identifiers and numbers; the message names the field.
pub fn validate(params: &[Param], values: &BTreeMap<String, ParamValue>) -> Result<(), String> {
    for p in params {
        let v = values.get(&p.name);
        let empty = v.is_none_or(ParamValue::is_empty);
        if p.required && empty {
            return Err(format!("{} is required", p.label()));
        }
        match (p.kind, v) {
            (ParamType::Identifier, Some(ParamValue::Text(s))) if !s.is_empty() => {
                check_identifier(s).map_err(|e| format!("{}: {e}", p.label()))?;
            }
            (ParamType::Number, Some(ParamValue::Text(s)))
                if !s.trim().is_empty() && s.trim().parse::<f64>().is_err() =>
            {
                return Err(format!("{}: not a number", p.label()));
            }
            (ParamType::List, Some(ParamValue::List(rows))) => {
                for (i, row) in rows.iter().enumerate() {
                    validate(&p.fields, row)
                        .map_err(|e| format!("{} row {}: {e}", p.label(), i + 1))?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// A PostgreSQL name: not empty, no NUL, at most 63 bytes (D-013).
pub fn check_identifier(s: &str) -> Result<(), String> {
    if s.is_empty() {
        Err("empty name".to_owned())
    } else if s.contains('\0') {
        Err("a name cannot contain a NUL character".to_owned())
    } else if s.len() > 63 {
        Err("a name can have at most 63 bytes".to_owned())
    } else {
        Ok(())
    }
}

/// Variables every template body can use.
#[derive(Debug, Clone)]
pub struct TemplateContext {
    pub version: ServerVersion,
    pub database: String,
}

/// Renders a template with the entered values.
pub fn render(
    template: &TemplateFile,
    values: &BTreeMap<String, ParamValue>,
    ctx: &TemplateContext,
) -> Result<String, String> {
    validate(&template.params, values)?;
    let mut env = Environment::new();
    env.set_syntax(
        SyntaxConfig::builder()
            .trim_blocks(true)
            .lstrip_blocks(true)
            .build()
            .expect("valid syntax config"),
    );
    register(&mut env);
    let mut vars: BTreeMap<String, Value> = BTreeMap::new();
    for p in &template.params {
        let value = match values.get(&p.name) {
            Some(v) => v.to_jinja(p.kind),
            None if p.kind == ParamType::List => Value::from(Vec::<Value>::new()),
            None if p.kind == ParamType::Bool => Value::from(false),
            None => Value::from(""),
        };
        vars.insert(p.name.clone(), value);
    }
    vars.insert("server_version".to_owned(), Value::from(ctx.version.0));
    vars.insert("database".to_owned(), Value::from(ctx.database.clone()));
    env.render_str(&template.body, Value::from(vars))
        .map(|s| s.trim_end().to_owned() + "\n")
        .map_err(|e| {
            let mut msg = e.to_string();
            let mut source = std::error::Error::source(&e);
            while let Some(s) = source {
                msg.push_str(&format!(": {s}"));
                source = s.source();
            }
            msg
        })
}

// ---- library ------------------------------------------------------------------------

/// One template (all its version variants), identified by its path inside the
/// version folders, e.g. `Functions/function.pgtemplate`.
#[derive(Debug, Clone)]
pub struct TemplateEntry {
    pub rel: String,
    /// (version folder, file content, where it came from).
    variants: Vec<(String, String, String)>,
}

impl TemplateEntry {
    /// Category folders, e.g. `["Functions"]`.
    pub fn folders(&self) -> Vec<&str> {
        let mut parts: Vec<&str> = self.rel.split('/').collect();
        parts.pop();
        parts
    }

    /// File name without the extension.
    pub fn stem(&self) -> &str {
        let file = self.rel.rsplit('/').next().unwrap_or(&self.rel);
        file.strip_suffix(&format!(".{EXTENSION}")).unwrap_or(file)
    }

    /// The variant for a server version: (content, origin).
    pub fn resolve(&self, version: ServerVersion) -> Option<(&str, &str)> {
        let folders: Vec<&str> = self.variants.iter().map(|v| v.0.as_str()).collect();
        let folder = pick_folder(&folders, version)?;
        self.variants
            .iter()
            .find(|v| v.0 == folder)
            .map(|v| (v.1.as_str(), v.2.as_str()))
    }
}

/// All templates found, sorted by path.
#[derive(Debug, Clone, Default)]
pub struct TemplateLibrary {
    pub entries: Vec<TemplateEntry>,
}

fn is_version_folder(name: &str) -> bool {
    name == "default"
        || name
            .strip_suffix("_plus")
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

impl TemplateLibrary {
    /// Scans the templates folder — the only place templates are read from; the
    /// embedded ones get there by installing them (D-013).
    pub fn scan(dir: &Path) -> Self {
        let mut found: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
        let mut add = |rel: String, folder: &str, content: String, origin: String| {
            found
                .entry(rel)
                .or_default()
                .push((folder.to_owned(), content, origin));
        };
        scan_dir(dir, &mut add);
        let mut entries: Vec<TemplateEntry> = found
            .into_iter()
            .map(|(rel, variants)| TemplateEntry { rel, variants })
            .collect();
        entries.sort_by_key(|e| e.rel.to_lowercase());
        Self { entries }
    }
}

fn scan_dir(root: &Path, add: &mut impl FnMut(String, &str, String, String)) {
    let Ok(read) = std::fs::read_dir(root) else {
        return;
    };
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if e.path().is_dir() && is_version_folder(&name) {
            walk(&e.path(), "", &mut |rel, path| {
                if let Ok(text) = std::fs::read_to_string(path) {
                    add(rel, &name, text, path.display().to_string());
                }
            });
        }
    }
}

fn walk(dir: &Path, prefix: &str, f: &mut impl FnMut(String, &Path)) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if path.is_dir() {
            walk(&path, &format!("{prefix}{name}/"), f);
        } else if name.ends_with(&format!(".{EXTENSION}")) {
            f(format!("{prefix}{name}"), &path);
        }
    }
}

/// The embedded templates: (path below `templates/`, content), for scanning and for
/// `--install-templates`.
pub fn embedded_files() -> Vec<(String, &'static [u8])> {
    fn walk(dir: &'static Dir<'static>, out: &mut Vec<(String, &'static [u8])>) {
        for f in dir.files() {
            out.push((f.path().to_string_lossy().replace('\\', "/"), f.contents()));
        }
        for d in dir.dirs() {
            walk(d, out);
        }
    }
    let mut out = Vec::new();
    walk(&EMBEDDED, &mut out);
    out.sort();
    out
}

/// Copies the embedded templates into `dir` (`--install-templates`, Settings →
/// Install templates). Returns the number of files written.
pub fn install(dir: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    for (path, content) in embedded_files() {
        let target = dir.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FUNCTION: &str = "+++
name        = \"Function\"
description = \"Create a function\"

[[param]]
name    = \"schema\"
type    = \"choice\"
source  = \"schemas\"
default = \"public\"

[[param]]
name     = \"name\"
type     = \"identifier\"
required = true

[[param]]
name = \"args\"
type = \"list\"
  [[param.fields]]
  name = \"arg_name\"
  type = \"identifier\"
  [[param.fields]]
  name = \"arg_type\"
  type = \"text\"

[[param]]
name    = \"strict\"
type    = \"bool\"
+++
CREATE FUNCTION {{ schema|ident }}.{{ name|ident }}(
{%- for a in args %}{{ a.arg_name|ident }} {{ a.arg_type }}{% if not loop.last %}, {% endif %}{% endfor -%}
){% if strict %} STRICT{% endif %};
";

    fn ctx() -> TemplateContext {
        TemplateContext {
            version: ServerVersion(160000),
            database: "shop".into(),
        }
    }

    fn text(s: &str) -> ParamValue {
        ParamValue::Text(s.into())
    }

    #[test]
    fn parses_and_renders() {
        let t = parse(FUNCTION).unwrap();
        assert_eq!(t.name.as_deref(), Some("Function"));
        assert_eq!(t.params.len(), 4);
        assert_eq!(t.params[0].source, Some(Source::Schemas));
        assert_eq!(t.params[0].default_text(), "public");
        let mut values = BTreeMap::new();
        values.insert("schema".into(), text("Sales"));
        values.insert("name".into(), text("add_order"));
        values.insert(
            "args".into(),
            ParamValue::List(vec![
                BTreeMap::from([
                    ("arg_name".into(), text("p_id")),
                    ("arg_type".into(), text("integer")),
                ]),
                BTreeMap::from([
                    ("arg_name".into(), text("Note")),
                    ("arg_type".into(), text("text")),
                ]),
            ]),
        );
        values.insert("strict".into(), ParamValue::Bool(true));
        let sql = render(&t, &values, &ctx()).unwrap();
        assert_eq!(
            sql,
            "CREATE FUNCTION \"Sales\".add_order(p_id integer, \"Note\" text) STRICT;\n"
        );
        // required and identifier checks
        values.insert("name".into(), text(""));
        assert_eq!(render(&t, &values, &ctx()).unwrap_err(), "name is required");
        values.insert("name".into(), text(&"x".repeat(64)));
        assert!(
            render(&t, &values, &ctx())
                .unwrap_err()
                .contains("63 bytes")
        );
    }

    #[test]
    fn qualified_names() {
        let t = parse(
            "+++\n[[param]]\nname = \"t\"\ntype = \"choice\"\nsource = \"tables\"\n+++\n\
             {{ t|ident }} / {{ t.schema }} / {{ t.name|ident }} / {{ t }}",
        )
        .unwrap();
        let values = BTreeMap::from([(
            "t".to_owned(),
            ParamValue::Object(QualName::new("sales", "Order Lines")),
        )]);
        assert_eq!(
            render(&t, &values, &ctx()).unwrap(),
            "sales.\"Order Lines\" / sales / \"Order Lines\" / sales.Order Lines\n"
        );
    }

    #[test]
    fn functions_with_arguments_and_triggers_with_their_table() {
        let t = parse(
            "+++\n[[param]]\nname = \"f\"\ntype = \"choice\"\nsource = \"functions\"\n\
             [[param]]\nname = \"g\"\ntype = \"choice\"\nsource = \"triggers\"\n\
             [[param]]\nname = \"m\"\ntype = \"choice\"\nsource = \"materialized_views\"\n+++\n\
             DROP FUNCTION {{ f|ident }}; {{ f.args }}\nDROP TRIGGER {{ g|ident }}; {{ g.table }}",
        )
        .unwrap();
        assert_eq!(t.params[2].source, Some(Source::MaterializedViews));
        let values = BTreeMap::from([
            (
                "f".to_owned(),
                ParamValue::Object(QualName {
                    args: Some("n integer, text".into()),
                    ..QualName::new("s", "My Fn")
                }),
            ),
            (
                "g".to_owned(),
                ParamValue::Object(QualName {
                    table: Some("Orders".into()),
                    ..QualName::new("s", "trg")
                }),
            ),
        ]);
        assert_eq!(
            render(&t, &values, &ctx()).unwrap(),
            "DROP FUNCTION s.\"My Fn\"(n integer, text); n integer, text\n\
             DROP TRIGGER trg ON s.\"Orders\"; Orders\n"
        );
    }

    #[test]
    fn header_errors() {
        assert!(parse("CREATE").unwrap_err().contains("start with"));
        assert!(
            parse("+++\nname = \"x\"\n")
                .unwrap_err()
                .contains("closing")
        );
        assert!(
            parse("+++\n[[param]]\nname = \"a\"\ntype = \"choice\"\n+++\n")
                .unwrap_err()
                .contains("exactly one")
        );
        assert!(
            parse("+++\n[[param]]\nname = \"a\"\ntype = \"list\"\n+++\n")
                .unwrap_err()
                .contains("fields")
        );
        assert!(
            parse("+++\n[[param]]\nname = \"a\"\ntype = \"nope\"\n+++\n")
                .unwrap_err()
                .contains("header")
        );
        assert!(
            parse("+++\n[[param]]\nname = \"a\"\ntype = \"text\"\n[[param]]\nname = \"a\"\ntype = \"text\"\n+++\n")
                .unwrap_err()
                .contains("twice")
        );
    }

    #[test]
    fn scanning_one_folder_with_versions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("developer_templates");
        let write = |rel: &str, body: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(
                p,
                format!(
                    "+++
name = \"{body}\"
+++
{body}"
                ),
            )
            .unwrap();
        };
        write("default/Tables/table.pgtemplate", "table");
        write("default/Tables/Partitioned/part.pgtemplate", "part");
        write("default/Functions/function.pgtemplate", "f default");
        write("16_plus/Functions/function.pgtemplate", "f 16");
        write("junk/x.pgtemplate", "ignored");
        let lib = TemplateLibrary::scan(&root);
        let rels: Vec<&str> = lib.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(
            rels,
            vec![
                "Functions/function.pgtemplate",
                "Tables/Partitioned/part.pgtemplate",
                "Tables/table.pgtemplate"
            ]
        );
        let f = &lib.entries[0];
        assert_eq!(f.folders(), vec!["Functions"]);
        assert_eq!(f.stem(), "function");
        assert!(
            f.resolve(ServerVersion(150000))
                .unwrap()
                .0
                .ends_with("f default")
        );
        assert!(
            f.resolve(ServerVersion(170000))
                .unwrap()
                .0
                .ends_with("f 16")
        );
        assert_eq!(lib.entries[1].folders(), vec!["Tables", "Partitioned"]);
        // no folder: no templates (the embedded ones are only installed, D-013)
        assert!(
            TemplateLibrary::scan(&dir.path().join("missing"))
                .entries
                .is_empty()
        );
    }

    #[test]
    fn embedded_templates_parse_and_render() {
        let dir = tempfile::tempdir().unwrap();
        assert!(install(dir.path()).unwrap() > 0);
        let lib = TemplateLibrary::scan(dir.path());
        assert!(!lib.entries.is_empty());
        for e in &lib.entries {
            for version in [110000, 160000, 180000] {
                let (text, origin) = e.resolve(ServerVersion(version)).unwrap();
                let t = parse(text).unwrap_or_else(|err| panic!("{origin}: {err}"));
                assert!(t.name.is_some(), "{origin}: no name");
                // fill every parameter with a plausible value
                let mut values = BTreeMap::new();
                for p in &t.params {
                    values.insert(p.name.clone(), sample(p));
                }
                let sql = render(
                    &t,
                    &values,
                    &TemplateContext {
                        version: ServerVersion(version),
                        database: "db".into(),
                    },
                )
                .unwrap_or_else(|err| panic!("{origin}: {err}"));
                assert!(!sql.trim().is_empty(), "{origin}");
                if version == 160000 && std::env::var("PGTC_SHOW_TEMPLATES").is_ok() {
                    println!(
                        "---- {origin}
{sql}"
                    );
                }
            }
        }
    }

    fn sample(p: &Param) -> ParamValue {
        match p.kind {
            ParamType::Bool => ParamValue::Bool(true),
            ParamType::Number => text("10"),
            ParamType::List => ParamValue::List(vec![
                p.fields
                    .iter()
                    .map(|f| (f.name.clone(), sample(f)))
                    .collect(),
            ]),
            ParamType::Choice if p.source.is_some_and(Source::qualified) => {
                ParamValue::Object(QualName::new("public", "orders"))
            }
            ParamType::Choice if !p.options.is_empty() => text(&p.options[0]),
            ParamType::Sql => text("SELECT 1"),
            _ => text("x"),
        }
    }
}
