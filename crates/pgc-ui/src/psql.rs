//! psql-style commands (D-054): the command list, the parser (with psql patterns),
//! the catalog queries of a command and pgtc's generate commands (`gts` / `gti` /
//! `gtu` / `gtd` / `gtc` / `gto` / `gtsi` / `gtm` / `gtt` / `gtv`). Commands are psql's
//! backslash commands, typed without the backslash.

use pgc_core::quote::quote_ident;
use pgc_db::CatalogQuery;
use pgc_sql::{RenderContext, SqlCatalog};

/// Object kinds the F4 input completes (D-054), as `sql/completion` names them.
const RELATIONS: &[&str] = &[
    "table",
    "view",
    "materialized view",
    "foreign table",
    "sequence",
];
const SELECTABLE: &[&str] = &["table", "view", "foreign table"];
const TABLES: &[&str] = &["table"];

/// A command row: command, arguments, description (shown while typing and by `?`)
/// and the object kinds autocomplete offers for its argument (empty = none, D-054).
pub type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
);

pub const COMMANDS: &[Row] = &[
    (
        "d",
        "[name]",
        "describe a table, view, sequence; without a name: list them",
        RELATIONS,
    ),
    ("l", "[pattern]", "list databases", &[]),
    ("dn", "[pattern]", "list schemas", &[]),
    ("dt", "[pattern]", "list tables", &["table"]),
    ("dv", "[pattern]", "list views", &["view"]),
    (
        "dm",
        "[pattern]",
        "list materialized views",
        &["materialized view"],
    ),
    ("di", "[pattern]", "list indexes", &[]),
    ("ds", "[pattern]", "list sequences", &["sequence"]),
    ("df", "[pattern]", "list functions", &[]),
    ("dT", "[pattern]", "list data types", &[]),
    ("dD", "[pattern]", "list domains", &[]),
    ("du", "[pattern]", "list roles", &[]),
    ("dg", "[pattern]", "list roles", &[]),
    ("dx", "[pattern]", "list extensions", &[]),
    ("db", "[pattern]", "list tablespaces", &[]),
    ("dp", "[pattern]", "list access privileges", RELATIONS),
    ("conninfo", "", "information about the connection", &[]),
    ("?", "", "this list", &[]),
];

/// pgtc's own commands (listed separately by `?`).
pub const GENERATE: &[Row] = &[
    (
        "gts",
        "table",
        "generate SELECT (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gti",
        "table",
        "generate INSERT (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gtu",
        "table",
        "generate UPDATE (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gtd",
        "table",
        "generate DELETE (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gtc",
        "table",
        "generate PL/pgSQL cursor loop (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gto",
        "table",
        "generate INSERT ... ON CONFLICT upsert (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gtsi",
        "table",
        "generate PL/pgSQL variables + SELECT INTO by key (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gtv",
        "table ...",
        "generate a view joining the tables on foreign keys (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gtm",
        "table",
        "generate MERGE, PostgreSQL 15+ (inserted at the cursor)",
        TABLES,
    ),
    (
        "gtt",
        "table",
        "generate trigger function + row trigger (inserted at the cursor)",
        TABLES,
    ),
];

/// Modifiers: `+` (more columns), `S` (system objects).
const MODIFIERS: &[char] = &['+', 'S'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gen {
    Select,
    Insert,
    Update,
    Delete,
    Cursor,
    Upsert,
    Vars,
    Merge,
    Trigger,
}

/// A schema-qualified or plain name, folded like PostgreSQL.
pub type Name = (Option<String>, String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// A list: SQL file of `sql/psql/`, relkinds for `list_relations.sql`.
    List {
        file: &'static str,
        kinds: Option<String>,
        /// Lists of schema objects take `schema.name` patterns.
        qualified: bool,
        pattern: Option<String>,
        plus: bool,
        system: bool,
    },
    Describe {
        name: Name,
        plus: bool,
    },
    ConnInfo,
    Help,
    Generate {
        what: Gen,
        table: Name,
    },
    /// `gtv`: a view over one or more tables, in the order typed.
    View {
        tables: Vec<Name>,
    },
}

/// Rows of the F4 list: every command, pgtc's own after a separator (`None`).
pub fn list_rows() -> Vec<Option<(&'static str, &'static str, &'static str)>> {
    let shown = |r: &Row| Some((r.0, r.1, r.2));
    let mut rows: Vec<_> = COMMANDS.iter().map(shown).collect();
    rows.push(None);
    rows.extend(GENERATE.iter().map(shown));
    rows
}

/// A generate command typed without its table: the input waits for the name.
pub fn needs_table(input: &str) -> bool {
    matches!(
        without_backslash(input),
        "gts" | "gti" | "gtu" | "gtd" | "gtc" | "gto" | "gtsi" | "gtv" | "gtm" | "gtt"
    )
}

/// The typed text without surrounding spaces and a leading psql backslash.
pub fn without_backslash(input: &str) -> &str {
    let input = input.trim();
    input.strip_prefix('\\').unwrap_or(input)
}

/// Matching commands for the typed command word (first match = the list row).
pub fn matching(input: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    let word = without_backslash(input)
        .split_whitespace()
        .next()
        .unwrap_or("");
    let word = word.trim_end_matches(MODIFIERS);
    COMMANDS
        .iter()
        .chain(GENERATE)
        .filter(|(c, ..)| c.starts_with(word))
        .map(|r| (r.0, r.1, r.2))
        .collect()
}

/// The argument of `input` and the object kinds autocomplete offers for it (D-054):
/// the command's own list; combined letters (`dtv`) add up. `None` when the input
/// has no argument yet (no space after the command) or the command offers nothing.
pub fn completion_target(input: &str) -> Option<(&str, Vec<&'static str>)> {
    let input = input.trim_start();
    let input = input.strip_prefix('\\').unwrap_or(input);
    let i = input.find(char::is_whitespace)?;
    let (word, arg) = (&input[..i], input[i..].trim_start());
    let base = word.trim_end_matches(MODIFIERS);
    let row = |c: &str| COMMANDS.iter().chain(GENERATE).find(|r| r.0 == c);
    let mut kinds: Vec<&'static str> = match row(base) {
        Some(r) => r.3.to_vec(),
        None if base.len() > 2
            && base.starts_with('d')
            && base[1..].chars().all(|c| "tvmisE".contains(c)) =>
        {
            let mut kinds = Vec::new();
            for c in base[1..].chars() {
                match c {
                    'E' => kinds.push("foreign table"),
                    c => kinds.extend(row(&format!("d{c}")).map_or(&[][..], |r| r.3)),
                }
            }
            kinds
        }
        None => Vec::new(),
    };
    kinds.sort_unstable();
    kinds.dedup();
    // gtv takes several names: only the last one is completed
    let arg = if base == "gtv" { last_arg(arg) } else { arg };
    (!kinds.is_empty()).then_some((arg, kinds))
}

/// Parses a command. Commands are typed without psql's backslash (F4 already opened
/// the command line); a backslash typed out of psql habit is ignored.
pub fn parse(input: &str) -> Result<Cmd, String> {
    let input = without_backslash(input);
    let (word, arg) = match input.find(char::is_whitespace) {
        Some(i) => (&input[..i], input[i..].trim()),
        None => (input, ""),
    };
    let arg = (!arg.is_empty()).then(|| arg.to_owned());
    let unknown = || format!("Unknown command {word} — ? lists the commands");
    match word {
        "" => return Err("Type a command, e.g. dt — ? lists them".to_owned()),
        "?" => return Ok(Cmd::Help),
        "conninfo" => return Ok(Cmd::ConnInfo),
        "gtv" => {
            let names = split_args(arg.as_deref().unwrap_or(""));
            if names.is_empty() {
                return Err(
                    "gtv needs one or more table names, e.g. gtv orders customers".to_owned(),
                );
            }
            let tables = names
                .iter()
                .map(|n| parse_name(n))
                .collect::<Result<_, _>>()?;
            return Ok(Cmd::View { tables });
        }
        "gts" | "gti" | "gtu" | "gtd" | "gtc" | "gto" | "gtsi" | "gtm" | "gtt" => {
            let what = match word {
                "gts" => Gen::Select,
                "gti" => Gen::Insert,
                "gtu" => Gen::Update,
                "gtc" => Gen::Cursor,
                "gto" => Gen::Upsert,
                "gtsi" => Gen::Vars,
                "gtm" => Gen::Merge,
                "gtt" => Gen::Trigger,
                _ => Gen::Delete,
            };
            let Some(arg) = arg else {
                return Err(format!(
                    "{word} needs a table name, e.g. {word} public.orders"
                ));
            };
            return Ok(Cmd::Generate {
                what,
                table: parse_name(&arg)?,
            });
        }
        _ => {}
    }
    // \l and \d… with + / S at the end
    let base = word.trim_end_matches(MODIFIERS);
    let mods = &word[base.len()..];
    let plus = mods.contains('+');
    let system = mods.contains('S');
    let list = |file, kinds: Option<&str>, qualified| Cmd::List {
        file,
        kinds: kinds.map(str::to_owned),
        qualified,
        pattern: arg.clone(),
        plus,
        system,
    };
    Ok(match base {
        "l" => list("list_databases.sql", None, false),
        "d" => match &arg {
            Some(a) if a.contains(['*', '?']) => {
                return Err(
                    "d describes one name; for a pattern use dt, dv, ... (e.g. dt ord*)".to_owned(),
                );
            }
            Some(a) => Cmd::Describe {
                name: parse_name(a)?,
                plus,
            },
            None => list("list_relations.sql", Some("rpvmSf"), true),
        },
        "dn" => list("list_schemas.sql", None, false),
        "df" => list("list_functions.sql", None, true),
        "dT" => list("list_types.sql", None, true),
        "dD" => list("list_domains.sql", None, true),
        "du" | "dg" => list("list_roles.sql", None, false),
        "dx" => list("list_extensions.sql", None, false),
        "db" => list("list_tablespaces.sql", None, false),
        "dp" => list("list_privileges.sql", None, true),
        b if b.len() > 1 && b.starts_with('d') && b[1..].chars().all(|c| "tvmisE".contains(c)) => {
            let mut kinds = String::new();
            for c in b[1..].chars() {
                kinds.push_str(match c {
                    't' => "rp",
                    'v' => "v",
                    'm' => "m",
                    'i' => "iI",
                    's' => "S",
                    _ => "f",
                });
            }
            Cmd::List {
                file: "list_relations.sql",
                kinds: Some(kinds),
                qualified: true,
                pattern: arg.clone(),
                plus,
                system,
            }
        }
        _ => return Err(unknown()),
    })
}

/// Arguments separated by unquoted white space (`orders "Order Items" s.x`).
fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in text.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The last argument of `text`: what follows its last unquoted white space.
fn last_arg(text: &str) -> &str {
    let mut in_quotes = false;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => start = i + c.len_utf8(),
            _ => {}
        }
    }
    &text[start..]
}

/// `schema.name`, `name`, `"Quoted Name"` → folded parts.
pub fn parse_name(text: &str) -> Result<Name, String> {
    let parts = split_parts(text.trim());
    let bad = || format!("Not a valid name: {text}");
    let fold = |(s, quoted): &(String, bool)| {
        if *quoted { s.clone() } else { s.to_lowercase() }
    };
    match parts.as_slice() {
        [n] if !n.0.is_empty() => Ok((None, fold(n))),
        [s, n] if !s.0.is_empty() && !n.0.is_empty() => Ok((Some(fold(s)), fold(n))),
        _ => Err(bad()),
    }
}

/// Splits on unquoted dots; each part with "contained quoted text".
fn split_parts(text: &str) -> Vec<(String, bool)> {
    let mut parts = vec![(String::new(), false)];
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;
    while let Some(c) = chars.next() {
        let cur = parts.last_mut().expect("one part");
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                chars.next();
                cur.0.push('"');
            }
            '"' => {
                in_quotes = !in_quotes;
                cur.1 = true;
            }
            '.' if !in_quotes => parts.push((String::new(), false)),
            c => cur.0.push(c),
        }
    }
    parts
}

/// A psql pattern part as an anchored regular expression: `*` → `.*`, `?` → `.`,
/// unquoted text in lower case, quoted text exact.
pub fn pattern_regex(part: &str) -> String {
    let mut out = String::from("^(");
    let mut chars = part.chars().peekable();
    let mut in_quotes = false;
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                chars.next();
                out.push('"');
            }
            '"' => in_quotes = !in_quotes,
            '*' if !in_quotes => out.push_str(".*"),
            '?' if !in_quotes => out.push('.'),
            c => {
                let c = if in_quotes {
                    c.to_string()
                } else {
                    c.to_lowercase().to_string()
                };
                for ch in c.chars() {
                    if "\\.^$|()[]{}+*?".contains(ch) {
                        out.push('\\');
                    }
                    out.push(ch);
                }
            }
        }
    }
    out.push_str(")$");
    out
}

/// (schema regex, name regex) of a pattern; a pattern without a dot only names.
fn pattern_params(pattern: &str, qualified: bool) -> (Option<String>, Option<String>) {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in pattern.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            '.' if !in_quotes && qualified => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    let re = |p: &str| (!p.is_empty()).then(|| pattern_regex(p));
    match parts.as_slice() {
        [n] => (None, re(n)),
        [s, n] => (re(s), re(n)),
        // database.schema.name: the database part is ignored
        [.., s, n] => (re(s), re(n)),
        [] => (None, None),
    }
}

/// The catalog queries of a list or describe command.
pub fn queries(
    cmd: &Cmd,
    title: &str,
    sql: &SqlCatalog,
    ctx: RenderContext,
) -> Result<Vec<CatalogQuery>, String> {
    let render = |file: &str, vars: &[(&str, bool)]| {
        sql.render_with("psql", file, ctx, vars)
            .map_err(|e| e.to_string())
    };
    match cmd {
        Cmd::List {
            file,
            kinds,
            qualified,
            pattern,
            plus,
            system,
        } => {
            let hide_system = !system && pattern.is_none();
            let indexes = kinds.as_deref().is_some_and(|k| k.contains('i'));
            let text = render(
                file,
                &[
                    ("plus", *plus),
                    ("hide_system", hide_system),
                    ("indexes", indexes),
                ],
            )?;
            let (schema, name) = pattern
                .as_deref()
                .map_or((None, None), |p| pattern_params(p, *qualified));
            let mut params = if *qualified {
                vec![schema, name]
            } else {
                vec![name]
            };
            if let Some(k) = kinds {
                params.push(Some(k.clone()));
            }
            Ok(vec![CatalogQuery {
                title: title.to_owned(),
                sql: text,
                params,
                stop_if_empty: None,
                skip_empty: false,
            }])
        }
        Cmd::Describe { name, plus } => {
            let params = vec![name.0.clone(), Some(name.1.clone())];
            let shown = match &name.0 {
                Some(s) => format!("{s}.{}", name.1),
                None => name.1.clone(),
            };
            let mut out = Vec::new();
            for (file, section) in [
                ("describe.sql", "Relation"),
                ("describe_columns.sql", "Columns"),
                ("describe_indexes.sql", "Indexes"),
                ("describe_constraints.sql", "Constraints"),
                ("describe_triggers.sql", "Triggers"),
                ("describe_definition.sql", "Definition"),
            ] {
                let first = out.is_empty();
                out.push(CatalogQuery {
                    title: section.to_owned(),
                    sql: render(file, &[("plus", *plus)])?,
                    params: params.clone(),
                    stop_if_empty: first
                        .then(|| format!("Did not find any relation named \"{shown}\".")),
                    skip_empty: !first,
                });
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

/// The `\?` result: SQL over constant rows (no user values).
pub fn help_query() -> CatalogQuery {
    let mut rows = Vec::new();
    for (group, list) in [("psql", COMMANDS), ("pgtc", GENERATE)] {
        for (c, args, desc, _) in list {
            let cmd = if args.is_empty() {
                (*c).to_owned()
            } else {
                format!("{c} {args}")
            };
            rows.push(format!(
                "({}, {}, {})",
                pgc_core::quote::quote_literal(group),
                pgc_core::quote::quote_literal(&cmd),
                pgc_core::quote::quote_literal(desc)
            ));
        }
    }
    CatalogQuery {
        title: "?".to_owned(),
        sql: format!(
            "SELECT * FROM (VALUES {}) AS c(\"Group\", \"Command\", \"Description\")",
            rows.join(", ")
        ),
        params: Vec::new(),
        stop_if_empty: None,
        skip_empty: false,
    }
}

// ---- generate commands --------------------------------------------------------------

/// A column of a `generate.sql` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenColumn {
    pub name: String,
    pub type_name: String,
    pub not_null: bool,
    /// The default expression (`0`, `nextval('orders_id_seq'::regclass)`, ...).
    pub default: Option<String>,
    /// `a` (ALWAYS), `d` (BY DEFAULT) or empty.
    pub identity: String,
    pub generated: bool,
    pub pk_pos: Option<u32>,
    pub uk_pos: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<GenColumn>,
    /// Name of the unique constraint used when there is no primary key.
    pub unique: Option<String>,
    /// `pg_class.relkind`: `r` / `p` tables, `v` views, `f` foreign tables.
    pub kind: String,
}

impl GenTable {
    pub fn is_table(&self) -> bool {
        matches!(self.kind.as_str(), "r" | "p")
    }
}

/// `generate.sql` rows → the table (`None`: not found).
pub fn gen_table(rows: Vec<Vec<String>>) -> Option<GenTable> {
    let mut table: Option<GenTable> = None;
    for r in rows {
        let [
            schema,
            name,
            col,
            type_name,
            not_null,
            default,
            identity,
            generated,
            pk,
            uk,
            uk_name,
            kind,
        ]: [String; 12] = r.try_into().ok()?;
        let t = table.get_or_insert_with(|| GenTable {
            schema,
            name,
            columns: Vec::new(),
            unique: (!uk_name.is_empty()).then_some(uk_name),
            kind,
        });
        t.columns.push(GenColumn {
            name: col,
            type_name,
            not_null: not_null == "true",
            default: (!default.is_empty()).then_some(default),
            identity,
            generated: !generated.is_empty(),
            pk_pos: pk.parse().ok(),
            uk_pos: uk.parse().ok(),
        });
    }
    table
}

impl GenColumn {
    /// The database fills the column from a sequence (`serial` and the like): INSERT
    /// leaves it out.
    fn filled_by_sequence(&self) -> bool {
        self.default
            .as_deref()
            .is_some_and(|d| d.contains("nextval("))
    }
}

/// Comment of an INSERT column: type, NOT NULL and the default, if it has one (the
/// column can then be left out).
fn insert_comment(c: &GenColumn) -> String {
    let mut comment = type_comment(c);
    if let Some(d) = &c.default {
        // one line: the expression may hold a line break inside a string
        let d = d.replace(['\r', '\n'], " ");
        comment.push_str(&format!(", default {d}"));
    }
    comment
}

/// Comment of a value line: the type and NOT NULL.
fn type_comment(c: &GenColumn) -> String {
    if c.not_null {
        format!("{}, NOT NULL", c.type_name)
    } else {
        c.type_name.clone()
    }
}

/// Lines with comments aligned after the longest code part.
fn aligned(lines: &[(String, String)]) -> String {
    let width = lines
        .iter()
        .filter(|(_, c)| !c.is_empty())
        .map(|(code, _)| code.chars().count())
        .max()
        .unwrap_or(0);
    lines
        .iter()
        .map(|(code, comment)| {
            if comment.is_empty() {
                code.clone()
            } else {
                let pad = width.saturating_sub(code.chars().count());
                format!("{code}{}  -- {comment}", " ".repeat(pad))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The statement of a generate command; values are `[column]` placeholders (D-054).
pub fn generate(what: Gen, t: &GenTable) -> String {
    let table = format!("{}.{}", quote_ident(&t.schema), quote_ident(&t.name));
    let ph = |c: &GenColumn| format!("[{}]", c.name);
    // the key used in WHERE: primary key, else the unique constraint
    let mut key: Vec<&GenColumn> = t.columns.iter().filter(|c| c.pk_pos.is_some()).collect();
    key.sort_by_key(|c| c.pk_pos);
    let mut key_kind = "primary key".to_owned();
    if key.is_empty() {
        key = t.columns.iter().filter(|c| c.uk_pos.is_some()).collect();
        key.sort_by_key(|c| c.uk_pos);
        if let Some(u) = &t.unique {
            key_kind = format!("unique key {u} (no primary key)");
        }
    }
    let is_key = |c: &GenColumn| key.iter().any(|k| k.name == c.name);
    let where_lines = |out: &mut Vec<(String, String)>, indent: &str, end: &str| {
        if key.is_empty() {
            out.push((
                format!("{indent}WHERE [condition]{end}"),
                "no primary key: write a condition".to_owned(),
            ));
            return;
        }
        for (i, c) in key.iter().enumerate() {
            let lead = if i == 0 {
                format!("{indent}WHERE")
            } else {
                format!("{indent}    AND")
            };
            let last = i + 1 == key.len();
            out.push((
                format!(
                    "{lead} {} = {}{}",
                    quote_ident(&c.name),
                    ph(c),
                    if last { end } else { "" }
                ),
                format!("{key_kind}: {}", c.type_name),
            ));
        }
    };
    let writable = |c: &&GenColumn| !c.generated && c.identity != "a";
    // gto / gtm: as INSERT, but the key is always listed (else the row never matches)
    let upsert_cols: Vec<&GenColumn> = t
        .columns
        .iter()
        .filter(|c| !c.generated)
        .filter(|c| is_key(c) || (c.identity.is_empty() && !c.filled_by_sequence()))
        .collect();
    let overriding = key.iter().any(|k| k.identity == "a");
    let names = |cols: &[&GenColumn], prefix: &str| {
        cols.iter()
            .map(|c| format!("{prefix}{}", quote_ident(&c.name)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut lines: Vec<(String, String)> = Vec::new();
    match what {
        Gen::Select => {
            lines.push(("SELECT".to_owned(), String::new()));
            let n = t.columns.len();
            for (i, c) in t.columns.iter().enumerate() {
                let comma = if i + 1 < n { "," } else { "" };
                lines.push((
                    format!("    {}{comma}", quote_ident(&c.name)),
                    String::new(),
                ));
            }
            lines.push((format!("FROM {table};"), String::new()));
        }
        Gen::Insert => {
            let cols: Vec<&GenColumn> = t
                .columns
                .iter()
                .filter(writable)
                .filter(|c| c.identity.is_empty() && !c.filled_by_sequence())
                .collect();
            lines.push((format!("INSERT INTO {table} ("), String::new()));
            for (i, c) in cols.iter().enumerate() {
                let comma = if i + 1 < cols.len() { "," } else { "" };
                lines.push((
                    format!("    {}{comma}", quote_ident(&c.name)),
                    insert_comment(c),
                ));
            }
            lines.push((") VALUES (".to_owned(), String::new()));
            for (i, c) in cols.iter().enumerate() {
                let comma = if i + 1 < cols.len() { "," } else { "" };
                lines.push((format!("    {}{comma}", ph(c)), String::new()));
            }
            lines.push((");".to_owned(), String::new()));
        }
        Gen::Update => {
            let mut set: Vec<&GenColumn> = t
                .columns
                .iter()
                .filter(writable)
                .filter(|c| !is_key(c))
                .collect();
            if set.is_empty() {
                // only key columns: they are what can be set
                set = key.clone();
            }
            lines.push((format!("UPDATE {table} SET"), String::new()));
            for (i, c) in set.iter().enumerate() {
                let comma = if i + 1 < set.len() { "," } else { "" };
                lines.push((
                    format!("    {} = {}{comma}", quote_ident(&c.name), ph(c)),
                    type_comment(c),
                ));
            }
            where_lines(&mut lines, "", ";");
        }
        Gen::Delete => {
            lines.push((format!("DELETE FROM {table}"), String::new()));
            where_lines(&mut lines, "", ";");
        }
        Gen::Cursor => return cursor_loop(&table, t),
        Gen::Upsert => {
            let cols = &upsert_cols;
            lines.push((format!("INSERT INTO {table} ("), String::new()));
            for (i, c) in cols.iter().enumerate() {
                let comma = if i + 1 < cols.len() { "," } else { "" };
                lines.push((
                    format!("    {}{comma}", quote_ident(&c.name)),
                    insert_comment(c),
                ));
            }
            let values = if overriding {
                ") OVERRIDING SYSTEM VALUE VALUES ("
            } else {
                ") VALUES ("
            };
            lines.push((values.to_owned(), String::new()));
            for (i, c) in cols.iter().enumerate() {
                let comma = if i + 1 < cols.len() { "," } else { "" };
                lines.push((format!("    {}{comma}", ph(c)), String::new()));
            }
            lines.push((")".to_owned(), String::new()));
            if key.is_empty() {
                lines.push((
                    "ON CONFLICT DO NOTHING;".to_owned(),
                    "no primary key: write a conflict target to update".to_owned(),
                ));
            } else {
                let target = key
                    .iter()
                    .map(|c| quote_ident(&c.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let set: Vec<&&GenColumn> = cols.iter().filter(|c| !is_key(c)).collect();
                if set.is_empty() {
                    lines.push((
                        format!("ON CONFLICT ({target}) DO NOTHING;"),
                        key_kind.clone(),
                    ));
                } else {
                    lines.push((
                        format!("ON CONFLICT ({target}) DO UPDATE SET"),
                        key_kind.clone(),
                    ));
                    for (i, c) in set.iter().enumerate() {
                        let end = if i + 1 < set.len() { "," } else { ";" };
                        let n = quote_ident(&c.name);
                        lines.push((format!("    {n} = EXCLUDED.{n}{end}"), String::new()));
                    }
                }
            }
        }
        Gen::Merge => {
            let cols = &upsert_cols;
            lines.push((format!("MERGE INTO {table} AS t"), String::new()));
            lines.push(("USING (VALUES (".to_owned(), String::new()));
            for (i, c) in cols.iter().enumerate() {
                let comma = if i + 1 < cols.len() { "," } else { "" };
                lines.push((
                    format!("    {}::{}{comma}", ph(c), c.type_name),
                    insert_comment(c),
                ));
            }
            lines.push((format!(")) AS s ({})", names(cols, "")), String::new()));
            if key.is_empty() {
                lines.push((
                    "ON [condition]".to_owned(),
                    "no primary key: write a condition".to_owned(),
                ));
            } else {
                for (i, c) in key.iter().enumerate() {
                    let lead = if i == 0 { "ON" } else { "    AND" };
                    let n = quote_ident(&c.name);
                    let comment = if i == 0 {
                        key_kind.clone()
                    } else {
                        String::new()
                    };
                    lines.push((format!("{lead} t.{n} = s.{n}"), comment));
                }
            }
            lines.push(("WHEN MATCHED THEN".to_owned(), String::new()));
            let set: Vec<&&GenColumn> = cols.iter().filter(|c| !is_key(c)).collect();
            if set.is_empty() {
                lines.push(("    DO NOTHING".to_owned(), String::new()));
            } else {
                lines.push(("    UPDATE SET".to_owned(), String::new()));
                for (i, c) in set.iter().enumerate() {
                    let comma = if i + 1 < set.len() { "," } else { "" };
                    let n = quote_ident(&c.name);
                    lines.push((format!("        {n} = s.{n}{comma}"), String::new()));
                }
            }
            lines.push(("WHEN NOT MATCHED THEN".to_owned(), String::new()));
            lines.push((format!("    INSERT ({})", names(cols, "")), String::new()));
            if overriding {
                lines.push(("    OVERRIDING SYSTEM VALUE".to_owned(), String::new()));
            }
            lines.push((
                format!("    VALUES ({});", names(cols, "s.")),
                String::new(),
            ));
        }
        Gen::Trigger => return trigger(&table, t),
        Gen::Vars => {
            let vars = var_names(&t.columns);
            let width = vars.iter().map(|v| v.chars().count()).max().unwrap_or(0);
            lines.push(("DECLARE".to_owned(), String::new()));
            for (v, c) in vars.iter().zip(&t.columns) {
                let pad = " ".repeat(width - v.chars().count());
                lines.push((
                    format!("    {v}{pad}  {table}.{}%TYPE;", quote_ident(&c.name)),
                    String::new(),
                ));
            }
            lines.push(("BEGIN".to_owned(), String::new()));
            lines.push(("    SELECT".to_owned(), String::new()));
            let n = t.columns.len();
            for (i, c) in t.columns.iter().enumerate() {
                let comma = if i + 1 < n { "," } else { "" };
                lines.push((
                    format!("        {}{comma}", quote_ident(&c.name)),
                    String::new(),
                ));
            }
            lines.push(("    INTO STRICT".to_owned(), String::new()));
            for (i, v) in vars.iter().enumerate() {
                let comma = if i + 1 < n { "," } else { "" };
                lines.push((format!("        {v}{comma}"), String::new()));
            }
            lines.push((format!("    FROM {table}"), String::new()));
            where_lines(&mut lines, "    ", ";");
            lines.push(("    --PROCESS THE ROW HERE".to_owned(), String::new()));
            lines.push(("END;".to_owned(), String::new()));
        }
    }
    aligned(&lines)
}

/// `gtt`: a trigger function with `TG_OP` branches and a row trigger (D-054). The
/// function (in the table's schema) has a placeholder name: the script does not run
/// until it is named.
fn trigger(table: &str, t: &GenTable) -> String {
    let function = format!("{}.[function_name]", quote_ident(&t.schema));
    let mut out = vec![
        format!("CREATE OR REPLACE FUNCTION {function}()"),
        "RETURNS trigger".to_owned(),
        "LANGUAGE plpgsql".to_owned(),
        "AS $$".to_owned(),
        "BEGIN".to_owned(),
    ];
    let fields: Vec<String> = t
        .columns
        .iter()
        .map(|c| format!("NEW. / OLD.{}", quote_ident(&c.name)))
        .collect();
    let width = fields.iter().map(|f| f.chars().count()).max().unwrap_or(0);
    for (f, c) in fields.iter().zip(&t.columns) {
        let pad = " ".repeat(width - f.chars().count());
        out.push(format!("    -- {f}{pad}  {}", type_comment(c)));
    }
    for line in [
        "    IF TG_OP = 'INSERT' THEN",
        "        --PROCESS NEW HERE",
        "        RETURN NEW;",
        "    ELSIF TG_OP = 'UPDATE' THEN",
        "        --PROCESS OLD AND NEW HERE",
        "        RETURN NEW;",
        "    ELSIF TG_OP = 'DELETE' THEN",
        "        --PROCESS OLD HERE",
        "        RETURN OLD;",
        "    END IF;",
        "    RETURN NULL;",
        "END;",
        "$$;",
        "",
    ] {
        out.push(line.to_owned());
    }
    out.push(format!(
        "CREATE TRIGGER {}",
        quote_ident(&format!("{}_trg", t.name))
    ));
    out.push(format!("    BEFORE INSERT OR UPDATE OR DELETE ON {table}"));
    out.push(format!("    FOR EACH ROW EXECUTE FUNCTION {function}();"));
    out.join("\n")
}

/// `gtsi` variable names: `v_` + the column name in lower case, every character other
/// than a letter, digit or `_` turned into `_`; a repeated name gets `_2`, `_3`, ….
fn var_names(columns: &[GenColumn]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in columns {
        let base: String = c
            .name
            .to_lowercase()
            .chars()
            .map(|ch| {
                if ch.is_alphanumeric() || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        let base = format!("v_{base}");
        let mut name = base.clone();
        let mut n = 2;
        while out.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        out.push(name);
    }
    out
}

/// `gtc`: a bound cursor over every column and an OPEN / FETCH loop (D-054). No `DO`
/// wrapper: it runs in PL/pgSQL mode (D-020) and fits into a function body.
fn cursor_loop(table: &str, t: &GenTable) -> String {
    let mut out = vec![
        "DECLARE".to_owned(),
        "    cur CURSOR FOR".to_owned(),
        "        SELECT".to_owned(),
    ];
    let n = t.columns.len();
    for (i, c) in t.columns.iter().enumerate() {
        let comma = if i + 1 < n { "," } else { "" };
        out.push(format!("            {}{comma}", quote_ident(&c.name)));
    }
    out.push(format!("        FROM {table};"));
    out.push("    rec record;".to_owned());
    out.push("BEGIN".to_owned());
    out.push("    OPEN cur;".to_owned());
    out.push("    LOOP".to_owned());
    out.push("        FETCH cur INTO rec;".to_owned());
    out.push("        EXIT WHEN NOT FOUND;".to_owned());
    let fields: Vec<String> = t
        .columns
        .iter()
        .map(|c| format!("rec.{}", quote_ident(&c.name)))
        .collect();
    let width = fields.iter().map(|f| f.chars().count()).max().unwrap_or(0);
    for (f, c) in fields.iter().zip(&t.columns) {
        let pad = width - f.chars().count();
        out.push(format!(
            "        -- {f}{}  {}",
            " ".repeat(pad),
            type_comment(c)
        ));
    }
    out.push("        --PROCESS THE ROW HERE".to_owned());
    out.push("    END LOOP;".to_owned());
    out.push("    CLOSE cur;".to_owned());
    out.push("END;".to_owned());
    out.join("\n")
}

// ---- gtv: a view over several tables ------------------------------------------------

/// A table of a `gtv` view, from `view.sql`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<String>,
}

/// A foreign key between two tables of a view (positions in the typed order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFk {
    pub from: usize,
    pub to: usize,
    pub name: String,
    /// (referencing column, referenced column) in key order.
    pub pairs: Vec<(String, String)>,
    /// A referencing column allows NULL.
    pub nullable: bool,
}

/// `$1` / `$2` of `view.sql`: schemas and names as text in array syntax (bind
/// parameters, cast to `text[]` there).
pub fn view_params(tables: &[Name]) -> Vec<Option<String>> {
    let element = |v: Option<&str>| match v {
        Some(v) => format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")),
        None => "NULL".to_owned(),
    };
    let array = |items: Vec<String>| format!("{{{}}}", items.join(","));
    vec![
        Some(array(
            tables.iter().map(|t| element(t.0.as_deref())).collect(),
        )),
        Some(array(tables.iter().map(|t| element(Some(&t.1))).collect())),
    ]
}

/// A foreign key column pair row of `view.sql`.
struct FkRow {
    from: usize,
    to: usize,
    name: String,
    ord: i64,
    col: String,
    rcol: String,
    nullable: bool,
}

/// `view.sql` rows → the tables in the typed order and the foreign keys between
/// them; `Err(position)` (0-based) for the first name that was not found.
pub fn view_tables(
    rows: Vec<Vec<String>>,
    count: usize,
) -> Result<(Vec<ViewTable>, Vec<ViewFk>), usize> {
    let mut tables: Vec<Option<ViewTable>> = vec![None; count];
    let mut columns: Vec<Vec<(i64, String)>> = vec![Vec::new(); count];
    let mut pairs: Vec<FkRow> = Vec::new();
    let pos = |s: &str| {
        s.parse::<usize>()
            .ok()
            .and_then(|p| p.checked_sub(1))
            .filter(|p| *p < count)
    };
    for r in rows {
        let Some(i) = r.get(1).and_then(|s| pos(s)) else {
            continue;
        };
        match r.first().map(String::as_str) {
            Some("t") if r.len() >= 4 => {
                tables[i] = Some(ViewTable {
                    schema: r[2].clone(),
                    name: r[3].clone(),
                    columns: Vec::new(),
                });
            }
            Some("c") if r.len() >= 4 => {
                columns[i].push((r[2].parse().unwrap_or(0), r[3].clone()));
            }
            Some("f") if r.len() >= 8 => {
                if let Some(j) = pos(&r[2]) {
                    pairs.push(FkRow {
                        from: i,
                        to: j,
                        name: r[3].clone(),
                        ord: r[4].parse().unwrap_or(0),
                        col: r[5].clone(),
                        rcol: r[6].clone(),
                        nullable: r[7] != "true",
                    });
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::with_capacity(count);
    for (i, t) in tables.into_iter().enumerate() {
        let mut t = t.ok_or(i)?;
        columns[i].sort();
        t.columns = columns[i].drain(..).map(|(_, c)| c).collect();
        out.push(t);
    }
    pairs.sort_by(|a, b| (a.from, a.to, &a.name, a.ord).cmp(&(b.from, b.to, &b.name, b.ord)));
    let mut fks: Vec<ViewFk> = Vec::new();
    for p in pairs {
        match fks.last_mut() {
            Some(f) if f.from == p.from && f.to == p.to && f.name == p.name => {
                f.pairs.push((p.col, p.rcol));
                f.nullable |= p.nullable;
            }
            _ => fks.push(ViewFk {
                from: p.from,
                to: p.to,
                name: p.name,
                pairs: vec![(p.col, p.rcol)],
                nullable: p.nullable,
            }),
        }
    }
    Ok((out, fks))
}

/// `gtv`: `CREATE VIEW <schema>.[view_name]` over the tables, joined in the typed
/// order on a foreign key to an earlier table (D-054).
pub fn generate_view(tables: &[ViewTable], fks: &[ViewFk]) -> String {
    // column name prefixes: the table name; the same table again gets _2, _3, ...
    let mut prefixes: Vec<String> = Vec::new();
    for t in tables {
        let mut prefix = t.name.clone();
        let mut n = 2;
        while prefixes.contains(&prefix) {
            prefix = format!("{}_{n}", t.name);
            n += 1;
        }
        prefixes.push(prefix);
    }
    // aliases T1, T2, ... (unquoted: easy to type in joins and conditions)
    let alias = |i: usize| format!("T{}", i + 1);
    let col = |i: usize, c: &str| format!("{}.{}", alias(i), quote_ident(c));
    let from = |i: usize| {
        let t = &tables[i];
        format!(
            "{}.{} AS {}",
            quote_ident(&t.schema),
            quote_ident(&t.name),
            alias(i)
        )
    };
    let mut lines: Vec<(String, String)> = Vec::new();
    let schema = tables.first().map_or("public", |t| t.schema.as_str());
    lines.push((
        format!("CREATE VIEW {}.[view_name] AS", quote_ident(schema)),
        String::new(),
    ));
    lines.push(("SELECT".to_owned(), String::new()));
    let all: Vec<(usize, &String)> = tables
        .iter()
        .enumerate()
        .flat_map(|(i, t)| t.columns.iter().map(move |c| (i, c)))
        .collect();
    for (k, (i, c)) in all.iter().enumerate() {
        let comma = if k + 1 < all.len() { "," } else { "" };
        let mut line = format!("    {}", col(*i, c));
        // the first one keeps the name; later ones get the table name in front
        if all[..k].iter().any(|(j, d)| j < i && d == c) {
            let name = format!("{}_{c}", prefixes[*i]);
            line.push_str(&format!(" AS {}", quote_ident(&name)));
        }
        lines.push((format!("{line}{comma}"), String::new()));
    }
    lines.push((format!("FROM {}", from(0)), String::new()));
    for k in 1..tables.len() {
        // foreign keys between this table and an earlier one, either direction
        let mut found: Vec<&ViewFk> = fks
            .iter()
            .filter(|f| (f.from == k && f.to < k) || (f.to == k && f.from < k))
            .collect();
        found.sort_by(|a, b| (&a.name, a.to == k).cmp(&(&b.name, b.to == k)));
        let Some(fk) = found.first() else {
            lines.push((
                format!("JOIN {} ON 1=1", from(k)),
                "no foreign key: write the join condition".to_owned(),
            ));
            continue;
        };
        // LEFT JOIN keeps the earlier table's rows that have no parent
        let join = if fk.to == k && fk.nullable {
            "LEFT JOIN"
        } else {
            "JOIN"
        };
        let mut comment = fk.name.clone();
        let others: Vec<&str> = found[1..].iter().map(|f| f.name.as_str()).collect();
        if !others.is_empty() {
            comment.push_str(&format!("; also: {}", others.join(", ")));
        }
        for (n, (c, r)) in fk.pairs.iter().enumerate() {
            // this table's column first
            let (a, b) = if fk.from == k {
                (col(fk.from, c), col(fk.to, r))
            } else {
                (col(fk.to, r), col(fk.from, c))
            };
            if n == 0 {
                lines.push((format!("{join} {} ON {a} = {b}", from(k)), comment.clone()));
            } else {
                lines.push((format!("    AND {a} = {b}"), String::new()));
            }
        }
    }
    if let Some(last) = lines.last_mut() {
        last.0.push(';');
    }
    aligned(&lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, not_null: bool) -> GenColumn {
        GenColumn {
            name: name.into(),
            type_name: ty.into(),
            not_null,
            default: None,
            identity: String::new(),
            generated: false,
            pk_pos: None,
            uk_pos: None,
        }
    }

    fn orders() -> GenTable {
        GenTable {
            schema: "public".into(),
            name: "orders".into(),
            columns: vec![
                GenColumn {
                    default: Some("nextval('orders_id_seq'::regclass)".into()),
                    pk_pos: Some(1),
                    ..col("id", "integer", true)
                },
                col("customer_id", "integer", true),
                col("note", "text", false),
                GenColumn {
                    generated: true,
                    ..col("total", "numeric", false)
                },
            ],
            unique: None,
            kind: "r".into(),
        }
    }

    #[test]
    fn parse_commands_and_modifiers() {
        assert_eq!(parse("?"), Ok(Cmd::Help));
        assert_eq!(parse("conninfo"), Ok(Cmd::ConnInfo));
        // a backslash typed out of psql habit is ignored
        assert_eq!(parse(" \\conninfo"), Ok(Cmd::ConnInfo));
        let Ok(Cmd::List {
            file,
            kinds,
            pattern,
            plus,
            system,
            ..
        }) = parse("dtvS+ public.*")
        else {
            panic!()
        };
        assert_eq!(file, "list_relations.sql");
        assert_eq!(kinds.as_deref(), Some("rpv"));
        assert_eq!(pattern.as_deref(), Some("public.*"));
        assert!(plus && system);
        assert!(matches!(parse("l+"), Ok(Cmd::List { plus: true, .. })));
        assert_eq!(
            parse("d+ sales.\"Order Items\""),
            Ok(Cmd::Describe {
                name: (Some("sales".into()), "Order Items".into()),
                plus: true
            })
        );
        assert!(matches!(parse("d"), Ok(Cmd::List { kinds: Some(k), .. }) if k == "rpvmSf"));
        assert!(parse("d ord*").is_err());
        assert!(parse("xyz").unwrap_err().contains("Unknown command xyz"));
        assert_eq!(
            parse("gtu Orders"),
            Ok(Cmd::Generate {
                what: Gen::Update,
                table: (None, "orders".into())
            })
        );
        assert!(parse("gti").is_err());
        assert!(parse("gti a.b.c").is_err());
    }

    #[test]
    fn psql_patterns() {
        assert_eq!(pattern_regex("ord*"), "^(ord.*)$");
        assert_eq!(pattern_regex("Ord?r"), "^(ord.r)$");
        assert_eq!(pattern_regex("\"My*T\""), "^(My\\*T)$");
        assert_eq!(pattern_regex("a.b"), "^(a\\.b)$");
        assert_eq!(
            pattern_params("public.*", true),
            (Some("^(public)$".into()), Some("^(.*)$".into()))
        );
        assert_eq!(
            pattern_params("ord*", true),
            (None, Some("^(ord.*)$".into()))
        );
        assert_eq!(
            pattern_params("\"a.b\"", true),
            (None, Some("^(a\\.b)$".into()))
        );
        assert_eq!(
            pattern_params("public.", true),
            (Some("^(public)$".into()), None)
        );
    }

    #[test]
    fn completion_targets_of_commands() {
        let t = |s| completion_target(s).map(|(a, k)| (a.to_owned(), k));
        assert_eq!(t("dt ord"), Some(("ord".into(), vec!["table"])));
        assert_eq!(t("\\dt+ "), Some(("".into(), vec!["table"])));
        assert_eq!(
            t("dtvE x"),
            Some(("x".into(), vec!["foreign table", "table", "view"]))
        );
        assert_eq!(t("d+ x").unwrap().1, RELATIONS_SORTED);
        assert_eq!(t("gts o").unwrap().1, ["foreign table", "table", "view"]);
        assert_eq!(t("gtm o").unwrap().1, ["table"]);
        assert_eq!(t("gtt o").unwrap().1, ["table"]);
        assert_eq!(t("di x"), None);
        assert_eq!(t("du x"), None);
        assert_eq!(t("dt"), None, "no argument yet");
        assert_eq!(t("xyz a"), None);
    }

    const RELATIONS_SORTED: [&str; 5] = [
        "foreign table",
        "materialized view",
        "sequence",
        "table",
        "view",
    ];

    #[test]
    fn matching_commands_for_the_hint_list() {
        let m: Vec<&str> = matching("d").iter().map(|c| c.0).collect();
        assert!(m.contains(&"dt") && m.contains(&"d") && !m.contains(&"l"));
        let m: Vec<&str> = matching("gt").iter().map(|c| c.0).collect();
        assert_eq!(
            m,
            [
                "gts", "gti", "gtu", "gtd", "gtc", "gto", "gtsi", "gtv", "gtm", "gtt"
            ]
        );
        assert_eq!(matching("\\gt").len(), 10);
        assert_eq!(matching("dt+ x").len(), 1);
    }

    #[test]
    fn generate_statements() {
        let t = orders();
        assert_eq!(
            generate(Gen::Select, &t),
            "SELECT\n    id,\n    customer_id,\n    note,\n    total\nFROM public.orders;"
        );
        assert_eq!(
            generate(Gen::Insert, &t),
            "INSERT INTO public.orders (\n    customer_id,  -- integer, NOT NULL\n    note          -- text\n) VALUES (\n    [customer_id],\n    [note]\n);"
        );
        assert_eq!(
            generate(Gen::Update, &t),
            "UPDATE public.orders SET\n    customer_id = [customer_id],  -- integer, NOT NULL\n    note = [note]                 -- text\nWHERE id = [id];                  -- primary key: integer"
        );
        assert_eq!(
            generate(Gen::Delete, &t),
            "DELETE FROM public.orders\nWHERE id = [id];  -- primary key: integer"
        );
    }

    #[test]
    fn generate_cursor_loop() {
        assert_eq!(
            parse("gtc sales.\"Order Items\""),
            Ok(Cmd::Generate {
                what: Gen::Cursor,
                table: (Some("sales".into()), "Order Items".into())
            })
        );
        assert!(needs_table("gtc"));
        let mut t = orders();
        t.columns.push(col("Due Date", "date", false));
        let expected = [
            "DECLARE",
            "    cur CURSOR FOR",
            "        SELECT",
            "            id,",
            "            customer_id,",
            "            note,",
            "            total,",
            "            \"Due Date\"",
            "        FROM public.orders;",
            "    rec record;",
            "BEGIN",
            "    OPEN cur;",
            "    LOOP",
            "        FETCH cur INTO rec;",
            "        EXIT WHEN NOT FOUND;",
            "        -- rec.id           integer, NOT NULL",
            "        -- rec.customer_id  integer, NOT NULL",
            "        -- rec.note         text",
            "        -- rec.total        numeric",
            "        -- rec.\"Due Date\"   date",
            "        --PROCESS THE ROW HERE",
            "    END LOOP;",
            "    CLOSE cur;",
            "END;",
        ];
        assert_eq!(generate(Gen::Cursor, &t), expected.join("\n"));
    }

    #[test]
    fn generate_upsert() {
        assert!(matches!(
            parse("gto orders"),
            Ok(Cmd::Generate {
                what: Gen::Upsert,
                ..
            })
        ));
        let expected = [
            "INSERT INTO public.orders (",
            "    id,                         -- integer, NOT NULL, default nextval('orders_id_seq'::regclass)",
            "    customer_id,                -- integer, NOT NULL",
            "    note                        -- text",
            ") VALUES (",
            "    [id],",
            "    [customer_id],",
            "    [note]",
            ")",
            "ON CONFLICT (id) DO UPDATE SET  -- primary key",
            "    customer_id = EXCLUDED.customer_id,",
            "    note = EXCLUDED.note;",
        ];
        assert_eq!(generate(Gen::Upsert, &orders()), expected.join("\n"));
        // identity ALWAYS key: OVERRIDING SYSTEM VALUE; only key columns: DO NOTHING
        let t = GenTable {
            schema: "public".into(),
            name: "tags".into(),
            columns: vec![GenColumn {
                identity: "a".into(),
                pk_pos: Some(1),
                ..col("id", "bigint", true)
            }],
            unique: None,
            kind: "r".into(),
        };
        let u = generate(Gen::Upsert, &t);
        assert!(u.contains(") OVERRIDING SYSTEM VALUE VALUES ("), "{u}");
        assert!(
            u.ends_with("ON CONFLICT (id) DO NOTHING;  -- primary key"),
            "{u}"
        );
        // no key at all
        let t = GenTable {
            columns: vec![col("a", "text", false)],
            ..t
        };
        let u = generate(Gen::Upsert, &t);
        assert!(
            u.ends_with(
                "ON CONFLICT DO NOTHING;  -- no primary key: write a conflict target to update"
            ),
            "{u}"
        );
    }

    #[test]
    fn generate_merge() {
        assert!(matches!(
            parse("gtm orders"),
            Ok(Cmd::Generate {
                what: Gen::Merge,
                ..
            })
        ));
        let expected = [
            "MERGE INTO public.orders AS t",
            "USING (VALUES (",
            "    [id]::integer,           -- integer, NOT NULL, default nextval('orders_id_seq'::regclass)",
            "    [customer_id]::integer,  -- integer, NOT NULL",
            "    [note]::text             -- text",
            ")) AS s (id, customer_id, note)",
            "ON t.id = s.id               -- primary key",
            "WHEN MATCHED THEN",
            "    UPDATE SET",
            "        customer_id = s.customer_id,",
            "        note = s.note",
            "WHEN NOT MATCHED THEN",
            "    INSERT (id, customer_id, note)",
            "    VALUES (s.id, s.customer_id, s.note);",
        ];
        assert_eq!(
            generate(Gen::Merge, &orders()),
            expected.join(
                "
"
            )
        );
        // identity ALWAYS key, nothing else: DO NOTHING and OVERRIDING SYSTEM VALUE
        let t = GenTable {
            schema: "public".into(),
            name: "tags".into(),
            columns: vec![GenColumn {
                identity: "a".into(),
                pk_pos: Some(1),
                ..col("id", "bigint", true)
            }],
            unique: None,
            kind: "r".into(),
        };
        let m = generate(Gen::Merge, &t);
        assert!(
            m.contains(
                "WHEN MATCHED THEN
    DO NOTHING
"
            ),
            "{m}"
        );
        assert!(
            m.contains(
                "    OVERRIDING SYSTEM VALUE
    VALUES (s.id);"
            ),
            "{m}"
        );
        // no key
        let t = GenTable {
            columns: vec![col("a", "text", false)],
            ..t
        };
        let m = generate(Gen::Merge, &t);
        assert!(
            m.contains("ON [condition]  -- no primary key: write a condition"),
            "{m}"
        );
        assert!(
            m.contains(
                "        a = s.a
"
            ),
            "{m}"
        );
    }

    #[test]
    fn generate_trigger() {
        assert!(needs_table("gtt"));
        let expected = [
            "CREATE OR REPLACE FUNCTION public.[function_name]()",
            "RETURNS trigger",
            "LANGUAGE plpgsql",
            "AS $$",
            "BEGIN",
            "    -- NEW. / OLD.id           integer, NOT NULL",
            "    -- NEW. / OLD.customer_id  integer, NOT NULL",
            "    -- NEW. / OLD.note         text",
            "    -- NEW. / OLD.total        numeric",
            "    IF TG_OP = 'INSERT' THEN",
            "        --PROCESS NEW HERE",
            "        RETURN NEW;",
            "    ELSIF TG_OP = 'UPDATE' THEN",
            "        --PROCESS OLD AND NEW HERE",
            "        RETURN NEW;",
            "    ELSIF TG_OP = 'DELETE' THEN",
            "        --PROCESS OLD HERE",
            "        RETURN OLD;",
            "    END IF;",
            "    RETURN NULL;",
            "END;",
            "$$;",
            "",
            "CREATE TRIGGER orders_trg",
            "    BEFORE INSERT OR UPDATE OR DELETE ON public.orders",
            "    FOR EACH ROW EXECUTE FUNCTION public.[function_name]();",
        ];
        assert_eq!(
            generate(Gen::Trigger, &orders()),
            expected.join(
                "
"
            )
        );
        // names quoted when needed
        let t = GenTable {
            schema: "Sales".into(),
            name: "Order Items".into(),
            ..orders()
        };
        let g = generate(Gen::Trigger, &t);
        assert!(
            g.starts_with("CREATE OR REPLACE FUNCTION \"Sales\".[function_name]()"),
            "{g}"
        );
        assert!(g.contains("CREATE TRIGGER \"Order Items_trg\""), "{g}");
        assert!(
            g.ends_with("EXECUTE FUNCTION \"Sales\".[function_name]();"),
            "{g}"
        );
        assert!(
            !GenTable {
                kind: "v".into(),
                ..orders()
            }
            .is_table()
        );
    }

    #[test]
    fn view_command_arguments() {
        assert_eq!(
            parse("gtv orders sales.\"Order Items\"  Customers"),
            Ok(Cmd::View {
                tables: vec![
                    (None, "orders".into()),
                    (Some("sales".into()), "Order Items".into()),
                    (None, "customers".into()),
                ]
            })
        );
        assert!(parse("gtv").is_err());
        assert!(needs_table("gtv"));
        // autocomplete: only the last name
        let t = |s| completion_target(s).map(|(a, k)| (a.to_owned(), k));
        assert_eq!(
            t("gtv orders cu"),
            Some(("cu".into(), vec!["foreign table", "table", "view"]))
        );
        assert_eq!(t("gtv orders ").unwrap().0, "");
        assert_eq!(t("gtv \"Order Items\" x").unwrap().0, "x");
        assert_eq!(t("gtv \"Order It").unwrap().0, "\"Order It");
        // bind parameters in array syntax
        assert_eq!(
            view_params(&[(None, "a\"b".into()), (Some("s".into()), "c\\d".into())]),
            vec![
                Some("{NULL,\"s\"}".to_owned()),
                Some("{\"a\\\"b\",\"c\\\\d\"}".to_owned())
            ]
        );
    }

    fn vt(name: &str, cols: &[&str]) -> ViewTable {
        ViewTable {
            schema: "public".into(),
            name: name.into(),
            columns: cols.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    fn fk(from: usize, to: usize, name: &str, pairs: &[(&str, &str)], nullable: bool) -> ViewFk {
        ViewFk {
            from,
            to,
            name: name.into(),
            pairs: pairs
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect(),
            nullable,
        }
    }

    #[test]
    fn view_joins_on_foreign_keys() {
        let tables = [
            vt("orders", &["id", "customer_id", "note"]),
            vt("customers", &["id", "name", "note"]),
        ];
        // orders (earlier) holds a nullable foreign key: LEFT JOIN
        let fks = [fk(
            0,
            1,
            "orders_customer_id_fkey",
            &[("customer_id", "id")],
            true,
        )];
        let expected = [
            "CREATE VIEW public.[view_name] AS",
            "SELECT",
            "    T1.id,",
            "    T1.customer_id,",
            "    T1.note,",
            "    T2.id AS customers_id,",
            "    T2.name,",
            "    T2.note AS customers_note",
            "FROM public.orders AS T1",
            "LEFT JOIN public.customers AS T2 ON T2.id = T1.customer_id;  -- orders_customer_id_fkey",
        ];
        assert_eq!(generate_view(&tables, &fks), expected.join("\n"));
        // the other order: the new table holds the key, a plain JOIN
        let tables = [tables[1].clone(), tables[0].clone()];
        let fks = [fk(
            1,
            0,
            "orders_customer_id_fkey",
            &[("customer_id", "id")],
            true,
        )];
        let v = generate_view(&tables, &fks);
        assert!(
            v.contains("JOIN public.orders AS T2 ON T2.customer_id = T1.id;"),
            "{v}"
        );
        assert!(!v.contains("LEFT"), "{v}");
    }

    #[test]
    fn view_without_keys_several_keys_and_self_join() {
        // no foreign key: ON 1=1 with a comment
        let tables = [vt("a", &["x"]), vt("b", &["y"])];
        let v = generate_view(&tables, &[]);
        assert!(
            v.ends_with("JOIN public.b AS T2 ON 1=1;  -- no foreign key: write the join condition"),
            "{v}"
        );
        // two foreign keys to the same table: the first by name, the other in the comment;
        // a key of several columns: AND
        let tables = [
            vt("orders", &["bill_id", "bill_no", "ship_id"]),
            vt("addresses", &["id", "no"]),
        ];
        let fks = [
            fk(0, 1, "orders_ship_fkey", &[("ship_id", "id")], false),
            fk(
                0,
                1,
                "orders_bill_fkey",
                &[("bill_id", "id"), ("bill_no", "no")],
                false,
            ),
        ];
        let v = generate_view(&tables, &fks);
        assert!(
            v.contains("JOIN public.addresses AS T2 ON T2.id = T1.bill_id  -- orders_bill_fkey; also: orders_ship_fkey\n    AND T2.no = T1.bill_no;"),
            "{v}"
        );
        // the same table twice: the later names numbered
        let tables = [
            vt("employees", &["id", "manager_id"]),
            vt("employees", &["id", "manager_id"]),
        ];
        let fks = [
            fk(
                0,
                0,
                "employees_manager_fkey",
                &[("manager_id", "id")],
                true,
            ),
            fk(
                0,
                1,
                "employees_manager_fkey",
                &[("manager_id", "id")],
                true,
            ),
            fk(
                1,
                0,
                "employees_manager_fkey",
                &[("manager_id", "id")],
                true,
            ),
            fk(
                1,
                1,
                "employees_manager_fkey",
                &[("manager_id", "id")],
                true,
            ),
        ];
        let v = generate_view(&tables, &fks);
        assert!(v.contains("    T1.id,\n    T1.manager_id,\n"), "{v}");
        assert!(v.contains("    T2.id AS employees_2_id,"), "{v}");
        assert!(
            v.contains("JOIN public.employees AS T2 ON T2.manager_id = T1.id;"),
            "{v}"
        );
    }

    #[test]
    fn view_rows_to_tables_and_keys() {
        let row = |cells: &[&str]| -> Vec<String> {
            let mut r: Vec<String> = cells.iter().map(|c| (*c).to_owned()).collect();
            r.resize(9, String::new());
            r
        };
        let rows = vec![
            row(&["t", "2", "public", "customers", "r"]),
            row(&["t", "1", "public", "orders", "r"]),
            row(&["c", "1", "2", "customer_id"]),
            row(&["c", "1", "1", "id"]),
            row(&["c", "2", "1", "id"]),
            row(&["f", "1", "2", "o_fk", "2", "b", "y", "true"]),
            row(&["f", "1", "2", "o_fk", "1", "a", "x", "false"]),
        ];
        let (tables, fks) = view_tables(rows.clone(), 2).unwrap();
        assert_eq!(tables[0].name, "orders");
        assert_eq!(tables[0].columns, ["id", "customer_id"]);
        assert_eq!(tables[1].columns, ["id"]);
        assert_eq!(fks, [fk(0, 1, "o_fk", &[("a", "x"), ("b", "y")], true)]);
        // the third name was not found
        assert_eq!(view_tables(rows, 3), Err(2));
    }

    #[test]
    fn generate_variables_and_select_into() {
        assert!(needs_table("gtsi"));
        let mut t = orders();
        t.columns.push(col("Due Date", "date", false));
        t.columns.push(col("due-date", "date", false));
        let expected = [
            "DECLARE",
            "    v_id           public.orders.id%TYPE;",
            "    v_customer_id  public.orders.customer_id%TYPE;",
            "    v_note         public.orders.note%TYPE;",
            "    v_total        public.orders.total%TYPE;",
            "    v_due_date     public.orders.\"Due Date\"%TYPE;",
            "    v_due_date_2   public.orders.\"due-date\"%TYPE;",
            "BEGIN",
            "    SELECT",
            "        id,",
            "        customer_id,",
            "        note,",
            "        total,",
            "        \"Due Date\",",
            "        \"due-date\"",
            "    INTO STRICT",
            "        v_id,",
            "        v_customer_id,",
            "        v_note,",
            "        v_total,",
            "        v_due_date,",
            "        v_due_date_2",
            "    FROM public.orders",
            "    WHERE id = [id];  -- primary key: integer",
            "    --PROCESS THE ROW HERE",
            "END;",
        ];
        assert_eq!(generate(Gen::Vars, &t), expected.join("\n"));
    }

    #[test]
    fn insert_lists_columns_with_defaults_but_not_what_the_database_fills() {
        let with_default = |name: &str, ty: &str, def: &str| GenColumn {
            default: Some(def.into()),
            ..col(name, ty, true)
        };
        let t = GenTable {
            schema: "public".into(),
            name: "bugs".into(),
            columns: vec![
                GenColumn {
                    pk_pos: Some(1),
                    ..with_default("id", "integer", "nextval('bugs_id_seq'::regclass)")
                },
                with_default("status", "smallint", "10"),
                with_default("summary", "character varying(128)", "''::character varying"),
                with_default("note", "text", "'two\nlines'::text"),
                GenColumn {
                    identity: "d".into(),
                    ..col("seq", "bigint", true)
                },
                GenColumn {
                    generated: true,
                    ..col("total", "numeric", false)
                },
            ],
            unique: None,
            kind: "r".into(),
        };
        // every column has a default: only what the database fills itself is left out
        // (the sequence key, the identity and the generated column)
        let expected = [
            "INSERT INTO public.bugs (",
            "    status,   -- smallint, NOT NULL, default 10",
            "    summary,  -- character varying(128), NOT NULL, default ''::character varying",
            "    note      -- text, NOT NULL, default 'two lines'::text",
            ") VALUES (",
            "    [status],",
            "    [summary],",
            "    [note]",
            ");",
        ];
        assert_eq!(generate(Gen::Insert, &t), expected.join("\n"));
        // UPDATE comments stay as they were: type and NOT NULL
        let u = generate(Gen::Update, &t);
        assert!(u.contains("status = [status],"), "{u}");
        assert!(!u.contains("default"), "{u}");
    }

    #[test]
    fn generate_keys_unique_and_none() {
        let mut t = GenTable {
            schema: "s".into(),
            name: "Order Items".into(),
            columns: vec![
                GenColumn {
                    uk_pos: Some(2),
                    ..col("b", "text", true)
                },
                GenColumn {
                    uk_pos: Some(1),
                    ..col("a", "integer", true)
                },
                GenColumn {
                    identity: "a".into(),
                    ..col("n", "bigint", true)
                },
            ],
            unique: Some("items_a_b_key".into()),
            kind: "r".into(),
        };
        let d = generate(Gen::Delete, &t);
        assert!(
            d.starts_with("DELETE FROM s.\"Order Items\"\nWHERE a = [a]"),
            "{d}"
        );
        assert!(d.contains("    AND b = [b];"), "{d}");
        assert!(
            d.contains("-- unique key items_a_b_key (no primary key): integer"),
            "{d}"
        );
        // identity ALWAYS columns are not set
        let u = generate(Gen::Update, &t);
        assert!(!u.contains("n = [n]"), "{u}");
        // no key at all
        for c in &mut t.columns {
            c.uk_pos = None;
        }
        t.unique = None;
        let d = generate(Gen::Delete, &t);
        assert!(
            d.ends_with("WHERE [condition];  -- no primary key: write a condition"),
            "{d}"
        );
    }

    #[test]
    fn generate_rows_to_table() {
        let row = |c: &str, pk: &str, def: &str| -> Vec<String> {
            [
                "public", "orders", c, "integer", "true", def, "", "", pk, "", "", "r",
            ]
            .map(str::to_owned)
            .to_vec()
        };
        let serial = "nextval('orders_id_seq'::regclass)";
        let t = gen_table(vec![row("id", "1", serial), row("x", "", "")]).unwrap();
        assert_eq!(t.name, "orders");
        assert_eq!(t.columns[0].default.as_deref(), Some(serial));
        assert_eq!(t.columns[1].default, None);
        assert_eq!(t.columns[0].pk_pos, Some(1));
        assert_eq!(t.columns[1].pk_pos, None);
        assert!(t.unique.is_none());
        assert!(gen_table(Vec::new()).is_none());
    }
}
