//! psql-style commands (D-054): the command list, the parser (with psql patterns),
//! the catalog queries of a command and pgtc's generate commands (`\gts` / `\gti` /
//! `\gtu` / `\gtd`).

use pgc_core::quote::quote_ident;
use pgc_db::CatalogQuery;
use pgc_sql::{RenderContext, SqlCatalog};

/// (command, arguments, description) — shown while typing and by `\?`.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "\\d",
        "[name]",
        "describe a table, view, sequence; without a name: list them",
    ),
    ("\\l", "[pattern]", "list databases"),
    ("\\dn", "[pattern]", "list schemas"),
    ("\\dt", "[pattern]", "list tables"),
    ("\\dv", "[pattern]", "list views"),
    ("\\dm", "[pattern]", "list materialized views"),
    ("\\di", "[pattern]", "list indexes"),
    ("\\ds", "[pattern]", "list sequences"),
    ("\\df", "[pattern]", "list functions"),
    ("\\dT", "[pattern]", "list data types"),
    ("\\dD", "[pattern]", "list domains"),
    ("\\du", "[pattern]", "list roles"),
    ("\\dg", "[pattern]", "list roles"),
    ("\\dx", "[pattern]", "list extensions"),
    ("\\db", "[pattern]", "list tablespaces"),
    ("\\dp", "[pattern]", "list access privileges"),
    ("\\conninfo", "", "information about the connection"),
    ("\\?", "", "this list"),
];

/// pgtc's own commands (listed separately by `\?`).
pub const GENERATE: &[(&str, &str, &str)] = &[
    ("\\gts", "table", "generate SELECT (inserted at the cursor)"),
    ("\\gti", "table", "generate INSERT (inserted at the cursor)"),
    ("\\gtu", "table", "generate UPDATE (inserted at the cursor)"),
    ("\\gtd", "table", "generate DELETE (inserted at the cursor)"),
];

/// Modifiers: `+` (more columns), `S` (system objects).
const MODIFIERS: &[char] = &['+', 'S'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gen {
    Select,
    Insert,
    Update,
    Delete,
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
}

/// Rows of the F4 list: every command, pgtc's own after a separator (`None`).
pub fn list_rows() -> Vec<Option<(&'static str, &'static str, &'static str)>> {
    let mut rows: Vec<_> = COMMANDS.iter().copied().map(Some).collect();
    rows.push(None);
    rows.extend(GENERATE.iter().copied().map(Some));
    rows
}

/// A generate command typed without its table: the input waits for the name.
pub fn needs_table(input: &str) -> bool {
    let input = input.trim();
    let word = input.strip_prefix('\\').unwrap_or(input);
    matches!(word, "gts" | "gti" | "gtu" | "gtd")
}

/// Matching commands for the typed command word (first match = the list row).
pub fn matching(input: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    let word = input.split_whitespace().next().unwrap_or("");
    let word = word.strip_prefix('\\').unwrap_or(word);
    let word = word.trim_end_matches(MODIFIERS);
    COMMANDS
        .iter()
        .chain(GENERATE)
        .filter(|(c, _, _)| c[1..].starts_with(word))
        .copied()
        .collect()
}

/// Parses a command, typed with or without the backslash.
pub fn parse(input: &str) -> Result<Cmd, String> {
    let input = input.trim();
    let input = input.strip_prefix('\\').unwrap_or(input);
    let (word, arg) = match input.find(char::is_whitespace) {
        Some(i) => (&input[..i], input[i..].trim()),
        None => (input, ""),
    };
    let arg = (!arg.is_empty()).then(|| arg.to_owned());
    let unknown = || format!("Unknown command \\{word} — \\? lists the commands");
    match word {
        "" => return Err("Type a command, e.g. \\dt — \\? lists them".to_owned()),
        "?" => return Ok(Cmd::Help),
        "conninfo" => return Ok(Cmd::ConnInfo),
        "gts" | "gti" | "gtu" | "gtd" => {
            let what = match word {
                "gts" => Gen::Select,
                "gti" => Gen::Insert,
                "gtu" => Gen::Update,
                _ => Gen::Delete,
            };
            let Some(arg) = arg else {
                return Err(format!(
                    "\\{word} needs a table name, e.g. \\{word} public.orders"
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
                    "\\d describes one name; for a pattern use \\dt, \\dv, ... (e.g. \\dt ord*)"
                        .to_owned(),
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
        for (c, args, desc) in list {
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
        title: "\\?".to_owned(),
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
        ]: [String; 11] = r.try_into().ok()?;
        let t = table.get_or_insert_with(|| GenTable {
            schema,
            name,
            columns: Vec::new(),
            unique: (!uk_name.is_empty()).then_some(uk_name),
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
    let where_lines = |out: &mut Vec<(String, String)>, end: &str| {
        if key.is_empty() {
            out.push((
                format!("WHERE [condition]{end}"),
                "no primary key: write a condition".to_owned(),
            ));
            return;
        }
        for (i, c) in key.iter().enumerate() {
            let lead = if i == 0 { "WHERE" } else { "    AND" };
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
                .filter(|c| !key.iter().any(|k| k.name == c.name))
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
            where_lines(&mut lines, ";");
        }
        Gen::Delete => {
            lines.push((format!("DELETE FROM {table}"), String::new()));
            where_lines(&mut lines, ";");
        }
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
        }
    }

    #[test]
    fn parse_commands_and_modifiers() {
        assert_eq!(parse("\\?"), Ok(Cmd::Help));
        assert_eq!(parse("conninfo"), Ok(Cmd::ConnInfo));
        let Ok(Cmd::List {
            file,
            kinds,
            pattern,
            plus,
            system,
            ..
        }) = parse("\\dtvS+ public.*")
        else {
            panic!()
        };
        assert_eq!(file, "list_relations.sql");
        assert_eq!(kinds.as_deref(), Some("rpv"));
        assert_eq!(pattern.as_deref(), Some("public.*"));
        assert!(plus && system);
        assert!(matches!(parse("l+"), Ok(Cmd::List { plus: true, .. })));
        assert_eq!(
            parse("\\d+ sales.\"Order Items\""),
            Ok(Cmd::Describe {
                name: (Some("sales".into()), "Order Items".into()),
                plus: true
            })
        );
        assert!(matches!(parse("d"), Ok(Cmd::List { kinds: Some(k), .. }) if k == "rpvmSf"));
        assert!(parse("\\d ord*").is_err());
        assert!(
            parse("\\xyz")
                .unwrap_err()
                .contains("Unknown command \\xyz")
        );
        assert_eq!(
            parse("gtu Orders"),
            Ok(Cmd::Generate {
                what: Gen::Update,
                table: (None, "orders".into())
            })
        );
        assert!(parse("\\gti").is_err());
        assert!(parse("\\gti a.b.c").is_err());
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
    fn matching_commands_for_the_hint_list() {
        let m: Vec<&str> = matching("\\d").iter().map(|c| c.0).collect();
        assert!(m.contains(&"\\dt") && m.contains(&"\\d") && !m.contains(&"\\l"));
        let m: Vec<&str> = matching("gt").iter().map(|c| c.0).collect();
        assert_eq!(m, ["\\gts", "\\gti", "\\gtu", "\\gtd"]);
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
                "public", "orders", c, "integer", "true", def, "", "", pk, "", "",
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
