//! Developer templates (D-013, D-046): `.pgtemplate` files with a TOML header between
//! `+++` lines and a Jinja body.
//!
//! Template directories have version folders (`default/`, `16_plus/`, ...); inside a
//! version folder, sub-folders are categories (`default/Functions/function.pgtemplate`).
//! Templates are read only from the user's templates folder; the embedded ones get
//! there by installing them (`--install-templates`, Settings → Install templates).

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use include_dir::{Dir, include_dir};
use minijinja::Environment;
use minijinja::value::{Object, ObjectRepr, Value};
use pgc_core::ServerVersion;
use serde::Deserialize;

use crate::pick_folder;

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
    /// Every source, in the order the template editor offers them (D-074).
    pub const ALL: [Source; 13] = [
        Source::Schemas,
        Source::Tables,
        Source::Views,
        Source::MaterializedViews,
        Source::Sequences,
        Source::Functions,
        Source::Procedures,
        Source::Types,
        Source::UserTypes,
        Source::Domains,
        Source::Indexes,
        Source::Triggers,
        Source::Roles,
    ];

    /// The name in a template header (`source = "tables"`).
    pub fn name(self) -> &'static str {
        match self {
            Source::Schemas => "schemas",
            Source::Tables => "tables",
            Source::Views => "views",
            Source::MaterializedViews => "materialized_views",
            Source::Sequences => "sequences",
            Source::Functions => "functions",
            Source::Procedures => "procedures",
            Source::Types => "types",
            Source::UserTypes => "user_types",
            Source::Domains => "domains",
            Source::Indexes => "indexes",
            Source::Triggers => "triggers",
            Source::Roles => "roles",
        }
    }

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

impl ParamType {
    pub const ALL: [ParamType; 7] = [
        ParamType::Text,
        ParamType::Identifier,
        ParamType::Number,
        ParamType::Bool,
        ParamType::Choice,
        ParamType::Sql,
        ParamType::List,
    ];

    /// The name in a template header (`type = "choice"`).
    pub fn name(self) -> &'static str {
        match self {
            ParamType::Text => "text",
            ParamType::Identifier => "identifier",
            ParamType::Number => "number",
            ParamType::Bool => "bool",
            ParamType::Choice => "choice",
            ParamType::Sql => "sql",
            ParamType::List => "list",
        }
    }
}

/// One parameter of a template.
#[derive(Debug, Clone, PartialEq, Deserialize)]
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
    /// `sql`: the editor box takes the free rows of the form (D-013).
    #[serde(default)]
    pub fill: bool,
}

impl Param {
    /// A parameter with only its name and type set.
    pub fn new(name: impl Into<String>, kind: ParamType) -> Self {
        Self {
            name: name.into(),
            kind,
            label: None,
            required: false,
            default: None,
            help: None,
            options: Vec::new(),
            source: None,
            query: None,
            fields: Vec::new(),
            fill: false,
        }
    }

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
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateFile {
    pub name: Option<String>,
    pub description: String,
    pub params: Vec<Param>,
    pub body: String,
}

/// Parses a template file: `+++`, TOML header, `+++`, Jinja body.
pub fn parse(text: &str) -> Result<TemplateFile, String> {
    let (header, body) = split(text)?;
    let (name, description, params) = parse_header(&header)?;
    Ok(TemplateFile {
        name,
        description,
        params,
        body,
    })
}

/// The two parts of a template file as text: the TOML header (without the `+++`
/// lines) and the body.
pub fn split(text: &str) -> Result<(String, String), String> {
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
    Ok((header, lines.collect()))
}

/// A header and a body put together as the text of a template file.
pub fn join(header: &str, body: &str) -> String {
    let mut out = String::from("+++\n");
    out.push_str(header);
    if !header.is_empty() && !header.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("+++\n");
    out.push_str(body);
    out
}

/// Parses the TOML header: name, description, parameters.
pub fn parse_header(header: &str) -> Result<(Option<String>, String, Vec<Param>), String> {
    let h: Header = toml::from_str(header).map_err(|e| format!("header: {e}"))?;
    check_params(&h.params, false)?;
    Ok((
        h.name.filter(|n| !n.trim().is_empty()),
        h.description.unwrap_or_default(),
        h.params,
    ))
}

/// Checks parameter declarations as [`parse_header`] does (`in_list`: the fields of
/// a list).
pub fn check_declarations(params: &[Param], in_list: bool) -> Result<(), String> {
    check_params(params, in_list)
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
        // pgtc's own variables of a template: a parameter of that name would be
        // replaced by them without a word (D-046)
        if !in_list && CONTEXT_VARIABLES.iter().any(|(v, _)| *v == n) {
            return Err(format!(
                "parameter name '{n}' is taken: it is pgtc's variable of that name"
            ));
        }
        seen.push(n);
        if p.fill && (p.kind != ParamType::Sql || in_list) {
            return Err(format!(
                "`fill` of '{n}' is only for sql parameters of the form"
            ));
        }
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
    // built once: the form renders its preview after every key
    static ENV: LazyLock<Environment<'static>> = LazyLock::new(crate::environment);
    let env = &*ENV;
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
    // (their names: CONTEXT_VARIABLES)
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

// ---- writing and checking (D-074) ---------------------------------------------------

fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

fn write_param(out: &mut String, p: &Param, field: bool) {
    let (table, indent) = if field {
        ("  [[param.fields]]", "  ")
    } else {
        ("[[param]]", "")
    };
    out.push_str(table);
    out.push('\n');
    let mut line = |key: &str, value: String| {
        out.push_str(&format!("{indent}{key} = {value}\n"));
    };
    line("name", toml_str(&p.name));
    if let Some(label) = &p.label {
        line("label", toml_str(label));
    }
    line("type", toml_str(p.kind.name()));
    if p.required {
        line("required", "true".to_owned());
    }
    if let Some(default) = &p.default {
        line("default", default.to_string());
    }
    if let Some(help) = &p.help {
        line("help", toml_str(help));
    }
    if !p.options.is_empty() {
        let items: Vec<String> = p.options.iter().map(|o| toml_str(o)).collect();
        line("options", format!("[{}]", items.join(", ")));
    }
    if let Some(source) = p.source {
        line("source", toml_str(source.name()));
    }
    if let Some(query) = &p.query {
        line("query", toml_str(query));
    }
    if p.fill {
        line("fill", "true".to_owned());
    }
    for f in &p.fields {
        write_param(out, f, true);
    }
}

/// The TOML header of a template, as the template editor writes it (D-074): always
/// the same keys in the same order, so the same template gives the same text.
pub fn header_toml(name: &str, description: &str, params: &[Param]) -> String {
    let mut out = String::new();
    if !name.trim().is_empty() {
        out.push_str(&format!("name = {}\n", toml_str(name.trim())));
    }
    if !description.is_empty() {
        out.push_str(&format!("description = {}\n", toml_str(description)));
    }
    for p in params {
        if !out.is_empty() {
            out.push('\n');
        }
        write_param(&mut out, p, false);
    }
    out
}

/// The text of a template file ([`parse`] reads it back as the same template).
pub fn write(template: &TemplateFile) -> String {
    let header = header_toml(
        template.name.as_deref().unwrap_or(""),
        &template.description,
        &template.params,
    );
    join(&header, &template.body)
}

/// What the template editor says about a body (D-074).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BodyCheck {
    /// A Jinja syntax error and its line (1-based) in the body.
    pub error: Option<(String, Option<usize>)>,
    /// Variables the body uses that are neither parameters nor context variables.
    pub undeclared: Vec<String>,
    /// Parameters the body never uses.
    pub unused: Vec<String>,
}

impl BodyCheck {
    pub fn is_clean(&self) -> bool {
        self.error.is_none() && self.undeclared.is_empty() && self.unused.is_empty()
    }
}

/// Variables every body gets besides its parameters.
pub const CONTEXT_VARIABLES: [(&str, &str); 2] = [
    (
        "server_version",
        "the server's version as a number (170002)",
    ),
    ("database", "the name of the current database"),
];

/// Checks the Jinja body against the declared parameters.
pub fn check_body(body: &str, params: &[Param]) -> BodyCheck {
    static ENV: LazyLock<Environment<'static>> = LazyLock::new(crate::environment);
    let env = &*ENV;
    let template = match env.template_from_str(body) {
        Ok(t) => t,
        Err(e) => {
            let text = e.detail().map_or_else(|| e.to_string(), str::to_owned);
            return BodyCheck {
                error: Some((text, e.line())),
                ..BodyCheck::default()
            };
        }
    };
    let used = template.undeclared_variables(false);
    let known = |name: &str| {
        params.iter().any(|p| p.name == name)
            || CONTEXT_VARIABLES.iter().any(|(v, _)| *v == name)
            || env.globals().any(|(g, _)| g == name)
    };
    let mut undeclared: Vec<String> = used.iter().filter(|n| !known(n)).cloned().collect();
    undeclared.sort();
    let unused = params
        .iter()
        .filter(|p| !used.contains(&p.name))
        .map(|p| p.name.clone())
        .collect();
    BodyCheck {
        error: None,
        undeclared,
        unused,
    }
}

/// A filter the template editor offers: name, the text inserted after a value, what
/// it does.
pub const FILTERS: [(&str, &str, &str); 11] = [
    (
        "ident",
        "|ident",
        "quote as an identifier (\"Order\"; schema.name of a chosen object)",
    ),
    (
        "qtLiteral",
        "|qtLiteral",
        "quote as a string literal ('O''Brien'; NULL when empty)",
    ),
    (
        "qtTypeIdent",
        "|qtTypeIdent",
        "quote a type name (double precision and int[] stay as they are)",
    ),
    (
        "indent",
        "|indent(4, true)",
        "indent every line by 4 spaces (also the first)",
    ),
    (
        "default",
        "|default(\"\")",
        "another value when this one is not defined",
    ),
    ("upper", "|upper", "upper case"),
    ("lower", "|lower", "lower case"),
    ("trim", "|trim", "without the spaces at both ends"),
    (
        "replace",
        "|replace(\"a\", \"b\")",
        "replace a text by another",
    ),
    (
        "join",
        "|join(\", \")",
        "the items of a list in one line, separated",
    ),
    (
        "length",
        "|length",
        "number of characters, or of rows of a list",
    ),
];

/// How a parameter is written into a body (D-074): `{{ name }}`, with `ident` for
/// names, a block for a checkbox, the whole loop for a list.
pub fn param_snippet(p: &Param) -> String {
    let value = |prefix: &str, p: &Param| {
        let quoted =
            p.kind == ParamType::Identifier || (p.kind == ParamType::Choice && p.source.is_some());
        let filter = if quoted { "|ident" } else { "" };
        format!("{{{{ {prefix}{}{filter} }}}}", p.name)
    };
    match p.kind {
        ParamType::Bool => format!("{{% if {} %}}\n\n{{% endif %}}\n", p.name),
        ParamType::List => {
            let fields: Vec<String> = p.fields.iter().map(|f| value("row.", f)).collect();
            format!(
                "{{% for row in {} %}}\n    {}{{% if not loop.last %}},{{% endif %}}\n\n{{% endfor %}}\n",
                p.name,
                fields.join(" ")
            )
        }
        _ => value("", p),
    }
}

/// Values for the template editor's preview (D-074): every parameter's default; a
/// required one without a default gets its own name; a list gets one row.
pub fn sample_values(params: &[Param]) -> BTreeMap<String, ParamValue> {
    fn sample(p: &Param, always: bool) -> ParamValue {
        let default = p.default_text();
        let text = if default.trim().is_empty() && (p.required || always) {
            if p.kind == ParamType::Number {
                "1".to_owned()
            } else {
                p.name.clone()
            }
        } else {
            default
        };
        match p.kind {
            ParamType::Bool => ParamValue::Bool(p.default_bool()),
            ParamType::List => ParamValue::List(vec![
                p.fields
                    .iter()
                    .map(|f| (f.name.clone(), sample(f, true)))
                    .collect(),
            ]),
            ParamType::Choice => match p.source.map(Source::shape) {
                Some(Shape::Name) | None => ParamValue::Text(text),
                Some(_) if text.is_empty() => ParamValue::Text(text),
                Some(shape) => {
                    // a `types` default without a schema is a built-in type name
                    let none = if p.source == Some(Source::Types) {
                        ""
                    } else {
                        "schema"
                    };
                    let (schema, name) = text.split_once('.').unwrap_or((none, &text));
                    ParamValue::Object(QualName {
                        args: (shape == Shape::WithArgs).then(String::new),
                        table: (shape == Shape::OnTable).then(|| "table".to_owned()),
                        ..QualName::new(schema, name)
                    })
                }
            },
            _ => ParamValue::Text(text),
        }
    }
    params
        .iter()
        .map(|p| (p.name.clone(), sample(p, false)))
        .collect()
}

/// A file name for a template called `name`: lower case letters, digits and `_`.
pub fn file_stem_for(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_end_matches('_').to_owned();
    if out.is_empty() {
        "template".to_owned()
    } else {
        out
    }
}

/// A name of a section or of a template file must work as a file name everywhere.
pub fn check_file_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("The name is empty.".to_owned());
    }
    if name.starts_with('.') || name.ends_with('.') || name.ends_with(' ') {
        return Err("The name cannot start or end with a dot.".to_owned());
    }
    if let Some(c) = name
        .chars()
        .find(|c| c.is_control() || "\\/:*?\"<>|".contains(*c))
    {
        return Err(format!("The name cannot contain '{c}'."));
    }
    if is_version_folder(name) {
        return Err(format!("'{name}' is the name of a version folder."));
    }
    Ok(())
}

/// The path of a template below the templates folder (`default/Tables/table.pgtemplate`)
/// is one of the templates that come with pgtc: installing overwrites it (D-013).
pub fn is_builtin(rel_path: &str) -> bool {
    let rel = rel_path.replace('\\', "/");
    embedded_files().iter().any(|(path, _)| *path == rel)
}

/// A change of the template manager in the templates folder (D-074). Sections and
/// templates are named by their path below the version folders (`Tables`,
/// `Tables/table.pgtemplate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryOp {
    /// A new, empty section (in `default/`).
    NewSection(String),
    /// The section in every version folder gets a new name (its last part).
    RenameSection { section: String, name: String },
    /// Removes the section where it is empty; refused when it has files.
    DeleteSection(String),
    /// Every version of the template gets the file name `stem`.
    Rename { rel: String, stem: String },
    /// Every version of the template is copied to the file name `stem`.
    Copy { rel: String, stem: String },
    /// Deletes one file of the template: the one in version folder `folder`.
    Delete { rel: String, folder: String },
}

fn version_dirs(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter(|e| e.path().is_dir() && is_version_folder(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect()
}

/// A rename that changes only the case of the name (`tables` → `Tables`): where file
/// names ignore case (Windows, macOS) the new name "exists" — it is the item itself.
fn only_case_differs(from: &str, to: &str) -> bool {
    from != to && from.to_lowercase() == to.to_lowercase()
}

fn sibling(rel: &str, name: &str) -> String {
    match rel.rsplit_once('/') {
        Some((parent, _)) => format!("{parent}/{name}"),
        None => name.to_owned(),
    }
}

impl LibraryOp {
    /// Does the change in the templates folder `dir`; the error is a text for the user.
    pub fn run(&self, dir: &Path) -> Result<(), String> {
        let io = |what: &str, e: std::io::Error| format!("Cannot {what}:\n{e}");
        match self {
            LibraryOp::NewSection(section) => {
                for part in section.split('/') {
                    check_file_name(part)?;
                }
                let path = dir.join("default").join(section);
                if path.exists() {
                    return Err(format!("The section {section} exists already."));
                }
                std::fs::create_dir_all(&path).map_err(|e| io("create the section", e))
            }
            LibraryOp::RenameSection { section, name } => {
                check_file_name(name)?;
                let to = sibling(section, name.trim());
                let dirs = version_dirs(dir);
                if !only_case_differs(section, &to) && dirs.iter().any(|d| d.join(&to).exists()) {
                    return Err(format!("The section {to} exists already."));
                }
                for d in dirs {
                    let from = d.join(section);
                    if from.is_dir() {
                        std::fs::rename(&from, d.join(&to))
                            .map_err(|e| io("rename the section", e))?;
                    }
                }
                Ok(())
            }
            LibraryOp::DeleteSection(section) => {
                let dirs: Vec<_> = version_dirs(dir)
                    .into_iter()
                    .map(|d| d.join(section))
                    .filter(|d| d.is_dir())
                    .collect();
                for d in &dirs {
                    let empty = std::fs::read_dir(d)
                        .map(|mut r| r.next().is_none())
                        .unwrap_or(false);
                    if !empty {
                        return Err(format!(
                            "The section {section} is not empty.\nDelete its templates first."
                        ));
                    }
                }
                for d in dirs {
                    std::fs::remove_dir(&d).map_err(|e| io("delete the section", e))?;
                }
                Ok(())
            }
            LibraryOp::Rename { rel, stem } | LibraryOp::Copy { rel, stem } => {
                check_file_name(stem)?;
                let to = sibling(rel, &format!("{}.{EXTENSION}", stem.trim()));
                let dirs = version_dirs(dir);
                let copy = matches!(self, LibraryOp::Copy { .. });
                if !(only_case_differs(rel, &to) && !copy)
                    && dirs.iter().any(|d| d.join(&to).exists())
                {
                    return Err(format!(
                        "A template with the file name {to} exists already."
                    ));
                }
                for d in dirs {
                    let from = d.join(rel);
                    if !from.is_file() {
                        continue;
                    }
                    if copy {
                        std::fs::copy(&from, d.join(&to))
                            .map(|_| ())
                            .map_err(|e| io("copy the template", e))?;
                    } else {
                        std::fs::rename(&from, d.join(&to))
                            .map_err(|e| io("rename the template", e))?;
                    }
                }
                Ok(())
            }
            LibraryOp::Delete { rel, folder } => std::fs::remove_file(dir.join(folder).join(rel))
                .map_err(|e| io("delete the template", e)),
        }
    }
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

/// One file of a template: its version folder, its text and its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant<'a> {
    pub folder: &'a str,
    pub text: &'a str,
    pub path: &'a str,
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

    /// Every file of the template, `default` first, then by version.
    pub fn variants(&self) -> Vec<Variant<'_>> {
        let mut out: Vec<Variant<'_>> = self
            .variants
            .iter()
            .map(|v| Variant {
                folder: &v.0,
                text: &v.1,
                path: &v.2,
            })
            .collect();
        out.sort_by_key(|v| folder_major(v.folder).unwrap_or(0));
        out
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
    /// Every section (folder below a version folder, `/` between nested ones), also
    /// the empty ones the picker does not show (D-074).
    pub sections: Vec<String>,
}

/// `default` or `NN_plus`.
pub fn is_version_folder(name: &str) -> bool {
    name == "default" || folder_major(name).is_some()
}

/// The first major version of a `NN_plus` folder.
pub fn folder_major(name: &str) -> Option<u32> {
    let n = name.strip_suffix("_plus")?;
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
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
        let mut sections = Vec::new();
        scan_dir(dir, &mut add, &mut sections);
        let mut entries: Vec<TemplateEntry> = found
            .into_iter()
            .map(|(rel, variants)| TemplateEntry { rel, variants })
            .collect();
        entries.sort_by_key(|e| e.rel.to_lowercase());
        sections.sort_by_key(|s| s.to_lowercase());
        sections.dedup();
        Self { entries, sections }
    }
}

fn scan_dir(
    root: &Path,
    add: &mut impl FnMut(String, &str, String, String),
    sections: &mut Vec<String>,
) {
    let Ok(read) = std::fs::read_dir(root) else {
        return;
    };
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if e.path().is_dir() && is_version_folder(&name) {
            walk(
                &e.path(),
                "",
                &mut |rel, path| {
                    if let Ok(text) = std::fs::read_to_string(path) {
                        add(rel, &name, text, path.display().to_string());
                    }
                },
                sections,
            );
        }
    }
}

fn walk(dir: &Path, prefix: &str, f: &mut impl FnMut(String, &Path), sections: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if path.is_dir() {
            sections.push(format!("{prefix}{name}"));
            walk(&path, &format!("{prefix}{name}/"), f, sections);
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
        // pgtc's own variables are no parameter names
        for name in ["database", "server_version"] {
            let text = format!("+++\n[[param]]\nname = \"{name}\"\ntype = \"text\"\n+++\n");
            assert!(parse(&text).unwrap_err().contains("is taken"), "{name}");
        }
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
        assert!(
            parse(
                "+++
[[param]]
name = \"a\"
type = \"text\"
fill = true
+++
"
            )
            .unwrap_err()
            .contains("fill")
        );
        assert!(
            parse(
                "+++
[[param]]
name = \"a\"
type = \"sql\"
fill = true
+++
"
            )
            .is_ok()
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

    // ---- the template editor (D-074) -------------------------------------------------

    #[test]
    fn embedded_templates_survive_writing() {
        // what the editor writes is read back as the same template, and writing that
        // again gives the same text
        for (path, content) in embedded_files() {
            let text = std::str::from_utf8(content).unwrap();
            let t = parse(text).unwrap_or_else(|e| panic!("{path}: {e}"));
            let written = write(&t);
            let again = parse(&written).unwrap_or_else(|e| panic!("{path}: {e}\n{written}"));
            assert_eq!(t, again, "{path}");
            assert_eq!(write(&again), written, "{path}");
            // the built-in templates have no problems of their own
            let check = check_body(&t.body, &t.params);
            assert!(check.is_clean(), "{path}: {check:?}");
        }
    }

    #[test]
    fn header_is_written_in_a_fixed_order() {
        let mut list = Param::new("cols", ParamType::List);
        list.fields = vec![Param::new("col", ParamType::Identifier)];
        let mut owner = Param::new("owner", ParamType::Choice);
        owner.source = Some(Source::Roles);
        owner.label = Some("Owner \"role\"".to_owned());
        owner.required = true;
        owner.default = Some(toml::Value::String("postgres".to_owned()));
        let mut fill = Param::new("fill", ParamType::Number);
        fill.default = Some(toml::Value::Integer(90));
        let header = header_toml("Demo", "A test", &[owner, fill, list]);
        let (name, description, params) = parse_header(&header).unwrap();
        assert_eq!(name.as_deref(), Some("Demo"));
        assert_eq!(description, "A test");
        assert_eq!(params.len(), 3);
        assert_eq!(params[0].label.as_deref(), Some("Owner \"role\""));
        assert_eq!(params[1].default, Some(toml::Value::Integer(90)));
        assert_eq!(params[2].fields[0].name, "col");
        // keys in their order, fields indented
        let lines: Vec<&str> = header.lines().collect();
        assert_eq!(lines[0], "name = \"Demo\"");
        assert_eq!(lines[1], "description = \"A test\"");
        assert_eq!(lines[3], "[[param]]");
        assert_eq!(lines[4], "name = \"owner\"");
        assert!(lines[5].starts_with("label = "));
        assert_eq!(lines[6], "type = \"choice\"");
        assert_eq!(lines[7], "required = true");
        assert_eq!(lines[8], "default = \"postgres\"");
        assert_eq!(lines[9], "source = \"roles\"");
        assert!(
            header.ends_with("  [[param.fields]]\n  name = \"col\"\n  type = \"identifier\"\n")
        );
    }

    #[test]
    fn split_and_join() {
        let (header, body) = split(FUNCTION).unwrap();
        assert!(header.starts_with("name"));
        assert!(body.starts_with("CREATE"));
        assert_eq!(join(&header, &body), FUNCTION);
        assert_eq!(join("a = 1", "x"), "+++\na = 1\n+++\nx");
    }

    #[test]
    fn body_checks() {
        let params = vec![
            Param::new("name", ParamType::Identifier),
            Param::new("extra", ParamType::Text),
        ];
        let check = check_body(
            "CREATE {{ name|ident }} {{ nme }} {% if has(\"Merge\") %}{{ database }}{% endif %}\n\
             {% for r in rows %}{{ r.x }}{{ loop.index }}{% endfor %}",
            &params,
        );
        assert_eq!(check.error, None);
        assert_eq!(check.undeclared, vec!["nme", "rows"]);
        assert_eq!(check.unused, vec!["extra"]);
        assert!(!check.is_clean());

        let check = check_body("line one\n{% if name %}\nno end", &params);
        let (message, line) = check.error.expect("a syntax error");
        assert!(!message.is_empty());
        assert_eq!(line, Some(3));
        assert!(check_body("{{ name }}{{ extra }}", &params).is_clean());
    }

    #[test]
    fn snippets_for_parameters() {
        assert_eq!(
            param_snippet(&Param::new("comment", ParamType::Text)),
            "{{ comment }}"
        );
        assert_eq!(
            param_snippet(&Param::new("name", ParamType::Identifier)),
            "{{ name|ident }}"
        );
        let mut table = Param::new("table", ParamType::Choice);
        table.source = Some(Source::Tables);
        assert_eq!(param_snippet(&table), "{{ table|ident }}");
        let mut lang = Param::new("language", ParamType::Choice);
        lang.options = vec!["sql".to_owned()];
        assert_eq!(param_snippet(&lang), "{{ language }}");
        assert_eq!(
            param_snippet(&Param::new("unique", ParamType::Bool)),
            "{% if unique %}\n\n{% endif %}\n"
        );
        let mut args = Param::new("args", ParamType::List);
        args.fields = vec![
            Param::new("arg_name", ParamType::Identifier),
            Param::new("arg_type", ParamType::Text),
        ];
        let snippet = param_snippet(&args);
        assert_eq!(
            snippet,
            "{% for row in args %}\n    {{ row.arg_name|ident }} {{ row.arg_type }}\
             {% if not loop.last %},{% endif %}\n\n{% endfor %}\n"
        );
        // the loop it writes works
        let t = TemplateFile {
            name: None,
            description: String::new(),
            params: vec![args.clone()],
            body: snippet,
        };
        let sql = render(&t, &sample_values(&t.params), &ctx()).unwrap();
        assert_eq!(sql, "    arg_name arg_type\n");
    }

    #[test]
    fn sample_values_render_every_embedded_template() {
        for (path, content) in embedded_files() {
            let t = parse(std::str::from_utf8(content).unwrap()).unwrap();
            let values = sample_values(&t.params);
            let sql = render(&t, &values, &ctx()).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(!sql.trim().is_empty(), "{path}");
        }
    }

    #[test]
    fn file_names() {
        assert_eq!(file_stem_for("Create table"), "create_table");
        assert_eq!(file_stem_for("  Drop  (IF EXISTS)! "), "drop_if_exists");
        assert_eq!(file_stem_for("***"), "template");
        assert!(check_file_name("Tables").is_ok());
        for bad in [
            "", " ", "a/b", "a\\b", "x:y", ".hidden", "default", "16_plus",
        ] {
            assert!(check_file_name(bad).is_err(), "{bad}");
        }
        assert!(is_builtin("default/Tables/table.pgtemplate"));
        assert!(is_builtin("default\\Tables\\table.pgtemplate"));
        assert!(!is_builtin("default/Tables/my_table.pgtemplate"));
    }

    #[test]
    fn library_operations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let write_file = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write_file("default/Tables/t.pgtemplate", "+++\n+++\ndefault");
        write_file("16_plus/Tables/t.pgtemplate", "+++\n+++\n16");

        LibraryOp::NewSection("Mine".into()).run(root).unwrap();
        assert!(LibraryOp::NewSection("Mine".into()).run(root).is_err());
        assert!(LibraryOp::NewSection("a:b".into()).run(root).is_err());
        let lib = TemplateLibrary::scan(root);
        assert_eq!(lib.sections, vec!["Mine", "Tables"]);
        assert_eq!(lib.entries.len(), 1);
        let folders: Vec<&str> = lib.entries[0].variants().iter().map(|v| v.folder).collect();
        assert_eq!(folders, vec!["default", "16_plus"]);

        LibraryOp::Copy {
            rel: "Tables/t.pgtemplate".into(),
            stem: "t2".into(),
        }
        .run(root)
        .unwrap();
        assert!(root.join("16_plus/Tables/t2.pgtemplate").is_file());
        // an existing name is refused
        let taken = LibraryOp::Rename {
            rel: "Tables/t.pgtemplate".into(),
            stem: "t2".into(),
        };
        assert!(taken.run(root).is_err());
        LibraryOp::Rename {
            rel: "Tables/t.pgtemplate".into(),
            stem: "first".into(),
        }
        .run(root)
        .unwrap();
        assert!(root.join("default/Tables/first.pgtemplate").is_file());
        assert!(root.join("16_plus/Tables/first.pgtemplate").is_file());
        assert!(!root.join("default/Tables/t.pgtemplate").exists());

        LibraryOp::Delete {
            rel: "Tables/first.pgtemplate".into(),
            folder: "16_plus".into(),
        }
        .run(root)
        .unwrap();
        assert!(!root.join("16_plus/Tables/first.pgtemplate").exists());
        assert!(root.join("default/Tables/first.pgtemplate").is_file());

        // a section with templates is not deleted; an empty one is
        assert!(LibraryOp::DeleteSection("Tables".into()).run(root).is_err());
        LibraryOp::RenameSection {
            section: "Mine".into(),
            name: "Own".into(),
        }
        .run(root)
        .unwrap();
        assert!(root.join("default/Own").is_dir());
        // only the case of a name changes: not "exists already" where file names
        // ignore case
        LibraryOp::RenameSection {
            section: "Own".into(),
            name: "OWN".into(),
        }
        .run(root)
        .unwrap();
        LibraryOp::Rename {
            rel: "Tables/first.pgtemplate".into(),
            stem: "First".into(),
        }
        .run(root)
        .unwrap();
        let names: Vec<String> = std::fs::read_dir(root.join("default/Tables"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"First.pgtemplate".to_owned()), "{names:?}");
        LibraryOp::RenameSection {
            section: "OWN".into(),
            name: "Own".into(),
        }
        .run(root)
        .unwrap();
        LibraryOp::DeleteSection("Own".into()).run(root).unwrap();
        assert!(!root.join("default/Own").exists());
    }
}
