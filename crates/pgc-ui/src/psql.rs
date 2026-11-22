//! psql-style commands (D-054): the command list, the parser (with psql patterns),
//! the catalog queries of a command and pgtc's generate commands (`gts` / `gti` /
//! `gtu` / `gtd` / `gtc` / `gto` / `gtsi` / `gtm` / `gtrb` / `gtra` / `gtsb` / `gtsa` /
//! `gtv`). Commands are psql's
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
        "gtrb",
        "table",
        "generate trigger function + BEFORE row trigger (inserted at the cursor)",
        TABLES,
    ),
    (
        "gtra",
        "table",
        "generate trigger function + AFTER row trigger (inserted at the cursor)",
        TABLES,
    ),
    (
        "gtsb",
        "table",
        "generate trigger function + BEFORE statement trigger (inserted at the cursor)",
        TABLES,
    ),
    (
        "gtsa",
        "table",
        "generate trigger function + AFTER statement trigger (inserted at the cursor)",
        TABLES,
    ),
    (
        "gtg",
        "table [rows]",
        "generate test data, 100 rows by default (inserted at the cursor)",
        TABLES,
    ),
    (
        "gtj",
        "table",
        "generate SELECT joined to the tables its foreign keys point to (inserted at the cursor)",
        TABLES,
    ),
    (
        "gtr",
        "table",
        "generate SELECTs of the rows that reference a row (inserted at the cursor)",
        TABLES,
    ),
    (
        "gtau",
        "table",
        "generate audit table + trigger function + trigger (inserted at the cursor)",
        TABLES,
    ),
    (
        "gp",
        "[schema]",
        "generate an empty PL/pgSQL procedure, default schema public (inserted at the cursor)",
        &["schema"],
    ),
    (
        "gpi",
        "table",
        "generate PL/pgSQL procedure: INSERT a row (inserted at the cursor)",
        TABLES,
    ),
    (
        "gpu",
        "table",
        "generate PL/pgSQL procedure: UPDATE a row by key (inserted at the cursor)",
        TABLES,
    ),
    (
        "gpd",
        "table",
        "generate PL/pgSQL procedure: DELETE a row by key (inserted at the cursor)",
        TABLES,
    ),
    (
        "gpo",
        "table",
        "generate PL/pgSQL procedure: upsert a row (inserted at the cursor)",
        TABLES,
    ),
    (
        "gf",
        "[schema]",
        "generate an empty PL/pgSQL function, default schema public (inserted at the cursor)",
        &["schema"],
    ),
    (
        "gfr",
        "table",
        "generate PL/pgSQL function: one row by key (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gfs",
        "table",
        "generate PL/pgSQL function: RETURNS SETOF the table (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gft",
        "table",
        "generate PL/pgSQL function: RETURNS TABLE with every column (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gfe",
        "table",
        "generate PL/pgSQL function: does a row with this key exist (inserted at the cursor)",
        SELECTABLE,
    ),
    (
        "gfj",
        "table",
        "generate PL/pgSQL function: one row by key as jsonb (inserted at the cursor)",
        SELECTABLE,
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
    /// `gtrb` / `gtra` / `gtsb` / `gtsa`: `FOR EACH ROW` or `STATEMENT`, `BEFORE` or `AFTER`.
    Trigger {
        row: bool,
        before: bool,
    },
    /// `gpi` / `gpu` / `gpd` / `gpo`: a PL/pgSQL procedure with one parameter per column.
    ProcInsert,
    ProcUpdate,
    ProcDelete,
    ProcUpsert,
    /// `gfr` / `gfs` / `gft` / `gfe` / `gfj`: a read-only PL/pgSQL function.
    FuncRow,
    FuncSet,
    FuncTable,
    FuncExists,
    FuncJson,
    /// `gtg table [rows]`: test data, a random parent for each foreign key.
    TestData {
        rows: u32,
    },
    /// `gtj`: SELECT joined to every table the table's foreign keys point to.
    JoinParents,
    /// `gtr`: one SELECT per foreign key that points to the table.
    Referencing,
    /// `gtau`: an audit table, its trigger function and trigger.
    Audit,
}

/// Rows `gtg` writes without a number.
pub const TEST_ROWS: u32 = 100;

/// The generate commands that take one table name, and what each generates: the one
/// list the parser, the messages and the input line go by. (`gtv` takes several
/// tables, `gtg` a number as well, `gp` / `gf` a schema: the parser has those.)
const TABLE_COMMANDS: &[(&str, Gen)] = &[
    ("gts", Gen::Select),
    ("gti", Gen::Insert),
    ("gtu", Gen::Update),
    ("gtd", Gen::Delete),
    ("gtc", Gen::Cursor),
    ("gto", Gen::Upsert),
    ("gtsi", Gen::Vars),
    ("gtm", Gen::Merge),
    (
        "gtrb",
        Gen::Trigger {
            row: true,
            before: true,
        },
    ),
    (
        "gtra",
        Gen::Trigger {
            row: true,
            before: false,
        },
    ),
    (
        "gtsb",
        Gen::Trigger {
            row: false,
            before: true,
        },
    ),
    (
        "gtsa",
        Gen::Trigger {
            row: false,
            before: false,
        },
    ),
    ("gpi", Gen::ProcInsert),
    ("gpu", Gen::ProcUpdate),
    ("gpd", Gen::ProcDelete),
    ("gpo", Gen::ProcUpsert),
    ("gfr", Gen::FuncRow),
    ("gfs", Gen::FuncSet),
    ("gft", Gen::FuncTable),
    ("gfe", Gen::FuncExists),
    ("gfj", Gen::FuncJson),
    ("gtj", Gen::JoinParents),
    ("gtr", Gen::Referencing),
    ("gtau", Gen::Audit),
];

impl Gen {
    /// The F4 command of a generate kind (for messages).
    pub fn command(self) -> &'static str {
        match self {
            Gen::TestData { .. } => "gtg",
            what => TABLE_COMMANDS
                .iter()
                .find(|(_, g)| *g == what)
                .map_or("", |(command, _)| command),
        }
    }

    /// Also needs the table's foreign keys (`foreign_keys.sql`) after its columns.
    pub fn needs_keys(self) -> bool {
        matches!(
            self,
            Gen::TestData { .. } | Gen::JoinParents | Gen::Referencing
        )
    }

    /// Works on tables only (not views or foreign tables): by the object kinds its
    /// row of [`GENERATE`] offers, so that the two cannot say different things.
    pub fn tables_only(self) -> bool {
        let command = self.command();
        GENERATE
            .iter()
            .find(|row| row.0 == command)
            .is_none_or(|row| row.3 != SELECTABLE)
    }
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
    /// `gp`: an empty procedure in a schema (`public` when none is typed).
    Procedure {
        schema: String,
    },
    /// `gf`: an empty function in a schema (`public` when none is typed).
    Function {
        schema: String,
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
    let word = without_backslash(input);
    matches!(word, "gtv" | "gtg") || TABLE_COMMANDS.iter().any(|(command, _)| *command == word)
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

/// `base` is a relation list command — `dt`, `dE`, `dtvm`: `d` and its letters. The
/// one rule for running it and for its autocomplete.
fn is_relation_list(base: &str) -> bool {
    base.strip_prefix('d')
        .is_some_and(|l| !l.is_empty() && l.chars().all(|c| "tvmisE".contains(c)))
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
        None if is_relation_list(base) => {
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

/// What the F4 autocomplete offers for the argument typed so far (D-054): the
/// argument, the object kinds of [`completion_target`], and whether schemas are
/// offered first (as `sales.`) — for every command whose argument can be
/// schema-qualified. `None`: nothing is offered.
pub fn completion_of(input: &str) -> Option<(&str, Vec<&'static str>, bool)> {
    if let Some((arg, kinds)) = completion_target(input) {
        // `gp` / `gf` take a schema itself
        let schemas = kinds.iter().any(|k| *k != "schema");
        return Some((arg, kinds, schemas));
    }
    let input = input.trim_start();
    let input = input.strip_prefix('\\').unwrap_or(input);
    let i = input.find(char::is_whitespace)?;
    let (word, arg) = (&input[..i], input[i..].trim_start());
    // lists of objects pgtc keeps no names of: only their schema is completed
    matches!(word.trim_end_matches(MODIFIERS), "di" | "df" | "dT" | "dD").then_some((
        arg,
        Vec::new(),
        true,
    ))
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
        "gp" | "gf" => {
            let schema = match arg.as_deref() {
                None => "public".to_owned(),
                Some(a) => match parse_name(a)? {
                    (None, s) => s,
                    _ => return Err(format!("{word} takes a schema name, e.g. {word} sales")),
                },
            };
            return Ok(if word == "gp" {
                Cmd::Procedure { schema }
            } else {
                Cmd::Function { schema }
            });
        }
        "gtg" => {
            let args = split_args(arg.as_deref().unwrap_or(""));
            let usage = || "gtg needs a table name and a number of rows, e.g. gtg orders 1000";
            let rows = match args.as_slice() {
                [_] => TEST_ROWS,
                [_, n] => match n.parse::<u32>() {
                    Ok(n) if n > 0 => n,
                    _ => return Err(usage().to_owned()),
                },
                _ => return Err(usage().to_owned()),
            };
            return Ok(Cmd::Generate {
                what: Gen::TestData { rows },
                table: parse_name(&args[0])?,
            });
        }
        _ if TABLE_COMMANDS.iter().any(|(command, _)| *command == word) => {
            let what = TABLE_COMMANDS
                .iter()
                .find(|(command, _)| *command == word)
                .map_or(Gen::Select, |(_, what)| *what);
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
            Some(a) if has_wildcard(a) => {
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
        b if is_relation_list(b) => {
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

// ---- double quotes --------------------------------------------------------------
// Two readings of a text with double quotes, which everything below goes by: where it
// is cut (`split_unquoted`, the text of the pieces stays as typed) and what it says
// (`dequote`, the quote marks gone).

/// The pieces of `text` between the characters `cut` picks, those inside double
/// quotes not counted; a piece keeps its quote marks. (`""` inside quotes leaves and
/// enters them again at once, which comes to the same.)
fn split_unquoted(text: &str, cut: impl Fn(char) -> bool) -> Vec<&str> {
    let mut pieces = Vec::new();
    let (mut in_quotes, mut start) = (false, 0);
    for (i, c) in text.char_indices() {
        if c == '"' {
            in_quotes = !in_quotes;
        } else if !in_quotes && cut(c) {
            pieces.push(&text[start..i]);
            start = i + c.len_utf8();
        }
    }
    pieces.push(&text[start..]);
    pieces
}

/// The characters `text` stands for, each with whether it was inside double quotes:
/// the quote marks are gone, `""` inside quotes is one `"`.
fn dequote(text: &str) -> Vec<(char, bool)> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                chars.next();
                out.push(('"', true));
            }
            '"' => in_quotes = !in_quotes,
            c => out.push((c, in_quotes)),
        }
    }
    out
}

/// Arguments separated by unquoted white space (`orders "Order Items" s.x`).
fn split_args(text: &str) -> Vec<String> {
    split_unquoted(text, char::is_whitespace)
        .into_iter()
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The last argument of `text`: what follows its last unquoted white space.
fn last_arg(text: &str) -> &str {
    split_unquoted(text, char::is_whitespace)
        .last()
        .copied()
        .unwrap_or("")
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
    split_unquoted(text, |c| c == '.')
        .into_iter()
        .map(|part| {
            let name = dequote(part).into_iter().map(|(c, _)| c).collect();
            (name, part.contains('"'))
        })
        .collect()
}

/// `arg` has a `*` or `?` outside double quotes: it is a pattern, not a name
/// (`"a*b"` is the name `a*b`).
fn has_wildcard(arg: &str) -> bool {
    dequote(arg)
        .into_iter()
        .any(|(c, quoted)| !quoted && matches!(c, '*' | '?'))
}

/// A psql pattern part as an anchored regular expression: `*` → `.*`, `?` → `.`,
/// unquoted text in lower case, quoted text exact.
pub fn pattern_regex(part: &str) -> String {
    let mut out = String::from("^(");
    for (c, in_quotes) in dequote(part) {
        match c {
            '*' if !in_quotes => out.push_str(".*"),
            '?' if !in_quotes => out.push('.'),
            c => {
                let c = if in_quotes {
                    c.to_string()
                } else {
                    c.to_lowercase().to_string()
                };
                // as psql: quoted text is taken as it is; outside quotes the other
                // characters of a regular expression keep their meaning —
                // `(a|b)`, `[0-9]`, `+` —, only `$`, `.` and `\` are literal
                let literal = if in_quotes { "\\.^$|()[]{}+*?" } else { "\\.$" };
                for ch in c.chars() {
                    if literal.contains(ch) {
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
    let parts = split_unquoted(pattern, |c| qualified && c == '.');
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

/// Adds the `WHERE` of a statement that finds a row by its key: a line for each term
/// — (left side, value, comment) — `AND` (as `and` writes it, with its indentation)
/// in front of every one but the first, `end` behind the last. Without a key: a
/// condition for the user to write.
fn where_by_key(
    lines: &mut Vec<(String, String)>,
    indent: &str,
    and: &str,
    end: &str,
    terms: Vec<(String, String, String)>,
) {
    if terms.is_empty() {
        lines.push((
            format!("{indent}WHERE [condition]{end}"),
            "no primary key: write a condition".to_owned(),
        ));
        return;
    }
    let last = terms.len() - 1;
    for (i, (left, value, comment)) in terms.into_iter().enumerate() {
        let lead = if i == 0 { "WHERE" } else { and };
        let end = if i == last { end } else { "" };
        lines.push((format!("{indent}{lead} {left} = {value}{end}"), comment));
    }
}

/// Adds `items` — a line each, with its comment — to `lines`, a comma behind every one
/// but the last: the column lists, value lists and SET lists of the generators.
fn listed(lines: &mut Vec<(String, String)>, items: impl IntoIterator<Item = (String, String)>) {
    let mut items = items.into_iter().peekable();
    while let Some((text, comment)) = items.next() {
        let comma = if items.peek().is_some() { "," } else { "" };
        lines.push((format!("{text}{comma}"), comment));
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

/// The key used in WHERE: the primary key, else the unique constraint (both may be
/// empty), and how comments name it.
fn key_of(t: &GenTable) -> (Vec<&GenColumn>, String) {
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
    (key, key_kind)
}

/// `gpu`: every writable column is part of the key — nothing to update.
pub fn nothing_to_update(t: &GenTable) -> bool {
    let (key, _) = key_of(t);
    !key.is_empty()
        && t.columns
            .iter()
            .filter(|c| !c.generated && c.identity != "a")
            .all(|c| key.iter().any(|k| k.name == c.name))
}

/// The statement of a generate command; values are `[column]` placeholders (D-054).
pub fn generate(what: Gen, t: &GenTable) -> String {
    let table = format!("{}.{}", quote_ident(&t.schema), quote_ident(&t.name));
    let ph = |c: &GenColumn| format!("[{}]", c.name);
    let (key, key_kind) = key_of(t);
    let is_key = |c: &GenColumn| key.iter().any(|k| k.name == c.name);
    let where_lines = |out: &mut Vec<(String, String)>, indent: &str, end: &str| {
        let terms = key.iter().map(|c| {
            let comment = format!("{key_kind}: {}", c.type_name);
            (quote_ident(&c.name), ph(c), comment)
        });
        where_by_key(out, indent, "    AND", end, terms.collect());
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
            listed(
                &mut lines,
                t.columns
                    .iter()
                    .map(|c| (format!("    {}", quote_ident(&c.name)), String::new())),
            );
            lines.push((format!("FROM {table};"), String::new()));
        }
        Gen::Insert => {
            let cols: Vec<&GenColumn> = t
                .columns
                .iter()
                .filter(writable)
                .filter(|c| c.identity.is_empty() && !c.filled_by_sequence())
                .collect();
            if cols.is_empty() {
                // every column is filled by the database
                lines.push((
                    format!("INSERT INTO {table} DEFAULT VALUES;"),
                    String::new(),
                ));
            } else {
                lines.push((format!("INSERT INTO {table} ("), String::new()));
                listed(
                    &mut lines,
                    cols.iter()
                        .map(|c| (format!("    {}", quote_ident(&c.name)), insert_comment(c))),
                );
                lines.push((") VALUES (".to_owned(), String::new()));
                listed(
                    &mut lines,
                    cols.iter()
                        .map(|c| (format!("    {}", ph(c)), String::new())),
                );
                lines.push((");".to_owned(), String::new()));
            }
        }
        Gen::Update => {
            let mut set: Vec<&GenColumn> = t
                .columns
                .iter()
                .filter(writable)
                .filter(|c| !is_key(c))
                .collect();
            if set.is_empty() {
                // only key columns: they are what can be set — those that can be
                set = key.iter().copied().filter(writable).collect();
            }
            if set.is_empty() {
                // (an UPDATE that sets a GENERATED ALWAYS column is refused)
                lines.push((
                    format!("-- {table} has no column that can be set: nothing to update"),
                    String::new(),
                ));
            } else {
                lines.push((format!("UPDATE {table} SET"), String::new()));
                listed(
                    &mut lines,
                    set.iter().map(|c| {
                        (
                            format!("    {} = {}", quote_ident(&c.name), ph(c)),
                            type_comment(c),
                        )
                    }),
                );
                where_lines(&mut lines, "", ";");
            }
        }
        Gen::Delete => {
            lines.push((format!("DELETE FROM {table}"), String::new()));
            where_lines(&mut lines, "", ";");
        }
        Gen::Cursor => return cursor_loop(&table, t),
        Gen::Upsert => {
            let cols = &upsert_cols;
            lines.push((format!("INSERT INTO {table} ("), String::new()));
            listed(
                &mut lines,
                cols.iter()
                    .map(|c| (format!("    {}", quote_ident(&c.name)), insert_comment(c))),
            );
            let values = if overriding {
                ") OVERRIDING SYSTEM VALUE VALUES ("
            } else {
                ") VALUES ("
            };
            lines.push((values.to_owned(), String::new()));
            listed(
                &mut lines,
                cols.iter()
                    .map(|c| (format!("    {}", ph(c)), String::new())),
            );
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
            listed(
                &mut lines,
                cols.iter()
                    .map(|c| (format!("    {}::{}", ph(c), c.type_name), insert_comment(c))),
            );
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
                listed(
                    &mut lines,
                    set.iter().map(|c| {
                        let n = quote_ident(&c.name);
                        (format!("        {n} = s.{n}"), String::new())
                    }),
                );
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
        Gen::Trigger { row, before } => return trigger(&table, t, row, before),
        Gen::Audit => return audit(&table, t),
        // read with the foreign keys: generate_related
        Gen::TestData { .. } | Gen::JoinParents | Gen::Referencing => {
            return generate_related(what, t, &Related::default()).unwrap_or_default();
        }
        Gen::ProcInsert
        | Gen::ProcUpdate
        | Gen::ProcDelete
        | Gen::ProcUpsert
        | Gen::FuncRow
        | Gen::FuncSet
        | Gen::FuncTable
        | Gen::FuncExists
        | Gen::FuncJson => {
            let p = ProcTable {
                table: &table,
                t,
                key: &key,
                key_kind: &key_kind,
                upsert_cols: &upsert_cols,
                overriding,
            };
            return if matches!(
                what,
                Gen::ProcInsert | Gen::ProcUpdate | Gen::ProcDelete | Gen::ProcUpsert
            ) {
                procedure(what, &p)
            } else {
                function(what, &p)
            };
        }
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
            listed(
                &mut lines,
                t.columns
                    .iter()
                    .map(|c| (format!("        {}", quote_ident(&c.name)), String::new())),
            );
            lines.push(("    INTO STRICT".to_owned(), String::new()));
            listed(
                &mut lines,
                vars.iter().map(|v| (format!("        {v}"), String::new())),
            );
            lines.push((format!("    FROM {table}"), String::new()));
            where_lines(&mut lines, "    ", ";");
            lines.push(("    --PROCESS THE ROW HERE".to_owned(), String::new()));
            lines.push(("END;".to_owned(), String::new()));
        }
    }
    aligned(&lines)
}

/// `gtrb` / `gtra` / `gtsb` / `gtsa`: a trigger function with `TG_OP` branches and
/// its trigger (D-054). The function (in the table's schema) has a placeholder name:
/// the script does not run until it is named.
fn trigger(table: &str, t: &GenTable, row: bool, before: bool) -> String {
    let function = format!("{}.[function_name]", quote_ident(&t.schema));
    let mut out = vec![
        format!("CREATE OR REPLACE FUNCTION {function}()"),
        "RETURNS trigger".to_owned(),
        "LANGUAGE plpgsql".to_owned(),
        "AS $$".to_owned(),
        "BEGIN".to_owned(),
    ];
    let push = |out: &mut Vec<String>, lines: &[&str]| {
        out.extend(lines.iter().map(|l| (*l).to_owned()));
    };
    if row {
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
    }
    match (row, before) {
        (true, true) => push(
            &mut out,
            &[
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
                "    RETURN NULL;  -- NULL skips the row",
            ],
        ),
        (true, false) => push(
            &mut out,
            &[
                "    IF TG_OP = 'INSERT' THEN",
                "        --PROCESS NEW HERE",
                "    ELSIF TG_OP = 'UPDATE' THEN",
                "        --PROCESS OLD AND NEW HERE",
                "    ELSIF TG_OP = 'DELETE' THEN",
                "        --PROCESS OLD HERE",
                "    END IF;",
                "    RETURN NULL;  -- ignored for AFTER triggers",
            ],
        ),
        (false, _) => push(
            &mut out,
            &[
                "    IF TG_OP = 'INSERT' THEN",
                "        --PROCESS INSERT HERE",
                "    ELSIF TG_OP = 'UPDATE' THEN",
                "        --PROCESS UPDATE HERE",
                "    ELSIF TG_OP = 'DELETE' THEN",
                "        --PROCESS DELETE HERE",
                "    ELSIF TG_OP = 'TRUNCATE' THEN",
                "        --PROCESS TRUNCATE HERE",
                "    END IF;",
                "    RETURN NULL;  -- ignored for statement triggers",
            ],
        ),
    }
    push(&mut out, &["END;", "$$;", ""]);
    let suffix = match (row, before) {
        (true, true) => "rb",
        (true, false) => "ra",
        (false, true) => "sb",
        (false, false) => "sa",
    };
    out.push(format!(
        "CREATE TRIGGER {}",
        quote_ident(&format!("{}_{suffix}_trg", t.name))
    ));
    let when = if before { "BEFORE" } else { "AFTER" };
    let events = if row {
        "INSERT OR UPDATE OR DELETE"
    } else {
        "INSERT OR UPDATE OR DELETE OR TRUNCATE"
    };
    out.push(format!("    {when} {events} ON {table}"));
    let each = if row { "ROW" } else { "STATEMENT" };
    out.push(format!(
        "    FOR EACH {each} EXECUTE FUNCTION {function}();"
    ));
    out.join(
        "
",
    )
}

/// `gtsi` variable names: `v_` + the column name in lower case, every character other
/// than a letter, digit or `_` turned into `_`; a repeated name gets `_2`, `_3`, ….
fn var_names(columns: &[GenColumn]) -> Vec<String> {
    let columns: Vec<&GenColumn> = columns.iter().collect();
    prefixed_names(&columns, "v_", &[])
}

/// Names `prefix` + the cleaned column name; a name already used or in `taken` gets
/// `_2`, `_3`, ….
fn prefixed_names(columns: &[&GenColumn], prefix: &str, taken: &[String]) -> Vec<String> {
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
        let base = format!("{prefix}{base}");
        let mut name = base.clone();
        let mut n = 2;
        while out.contains(&name) || taken.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        out.push(name);
    }
    out
}

/// What `procedure` needs from `generate`: the table and its key.
struct ProcTable<'a> {
    table: &'a str,
    t: &'a GenTable,
    key: &'a [&'a GenColumn],
    key_kind: &'a str,
    upsert_cols: &'a [&'a GenColumn],
    overriding: bool,
}

/// A parameter of a generated procedure.
struct Param<'a> {
    col: &'a GenColumn,
    inout: bool,
    comment: String,
}

/// Comment of a procedure parameter: NOT NULL and (for inserts) the default — the
/// type is on the line already.
fn param_comment(c: &GenColumn, with_default: bool) -> String {
    let mut parts = Vec::new();
    if c.not_null {
        parts.push("NOT NULL".to_owned());
    }
    if with_default && let Some(d) = &c.default {
        parts.push(format!("default {}", d.replace(['\r', '\n'], " ")));
    }
    parts.join(", ")
}

/// `gp`: an empty PL/pgSQL procedure in `schema` (D-054).
pub fn generate_procedure(schema: &str) -> String {
    [
        format!(
            "CREATE OR REPLACE PROCEDURE {}.[procedure_name]()",
            quote_ident(schema)
        ),
        "LANGUAGE plpgsql".to_owned(),
        "AS $$".to_owned(),
        "BEGIN".to_owned(),
        "    --PROCESS HERE".to_owned(),
        "END;".to_owned(),
        "$$;".to_owned(),
    ]
    .join("\n")
}

/// `gpi` / `gpu` / `gpd` / `gpo`: `CREATE OR REPLACE PROCEDURE <schema>.[procedure_name]`
/// with one `p_` parameter per column the statement uses (D-054).
fn procedure(what: Gen, p: &ProcTable) -> String {
    let (t, key) = (p.t, p.key);
    let is_key = |c: &GenColumn| key.iter().any(|k| k.name == c.name);
    let writable = |c: &&GenColumn| !c.generated && c.identity != "a";
    let key_params = || {
        key.iter().map(|c| Param {
            col: c,
            inout: false,
            comment: p.key_kind.to_owned(),
        })
    };
    let mut params: Vec<Param> = Vec::new();
    // gpi: the key columns the database fills come back through INOUT parameters
    let mut returned: Vec<&GenColumn> = Vec::new();
    match what {
        Gen::ProcInsert => {
            let cols = t
                .columns
                .iter()
                .filter(writable)
                .filter(|c| c.identity.is_empty() && !c.filled_by_sequence());
            params.extend(cols.map(|c| Param {
                col: c,
                inout: false,
                comment: param_comment(c, true),
            }));
            returned = key
                .iter()
                .copied()
                .filter(|k| !params.iter().any(|q| q.col.name == k.name))
                .collect();
            params.extend(returned.iter().map(|c| Param {
                col: c,
                inout: true,
                comment: format!("{}, filled by the database", p.key_kind),
            }));
        }
        Gen::ProcUpdate => {
            params.extend(key_params());
            params.extend(
                t.columns
                    .iter()
                    .filter(writable)
                    .filter(|c| !is_key(c))
                    .map(|c| Param {
                        col: c,
                        inout: false,
                        comment: param_comment(c, false),
                    }),
            );
        }
        Gen::ProcDelete => params.extend(key_params()),
        _ => {
            params.extend(p.upsert_cols.iter().map(|c| Param {
                col: c,
                inout: false,
                comment: if is_key(c) {
                    p.key_kind.to_owned()
                } else {
                    param_comment(c, true)
                },
            }));
        }
    }
    let columns: Vec<&GenColumn> = params.iter().map(|q| q.col).collect();
    let taken: Vec<String> = t.columns.iter().map(|c| c.name.to_lowercase()).collect();
    let names = prefixed_names(&columns, "p_", &taken);
    let name_of = |c: &GenColumn| {
        params
            .iter()
            .position(|q| q.col.name == c.name)
            .map(|i| names[i].clone())
            .unwrap_or_default()
    };
    let header = routine_header("PROCEDURE", &t.schema, &params, &names);
    let mut lines: Vec<(String, String)> = Vec::new();
    for l in ["LANGUAGE plpgsql", "AS $$", "BEGIN"] {
        lines.push((l.to_owned(), String::new()));
    }
    let list = |lines: &mut Vec<(String, String)>, items: Vec<String>, indent: &str| {
        let items = items.into_iter().map(|item| format!("{indent}{item}"));
        listed(lines, items.map(|line| (line, String::new())));
    };
    let where_key = |lines: &mut Vec<(String, String)>| {
        let terms = key
            .iter()
            .map(|c| (quote_ident(&c.name), name_of(c), String::new()));
        where_by_key(lines, "    ", "  AND", ";", terms.collect());
    };
    let not_found = |lines: &mut Vec<(String, String)>| {
        let shown = format!("{}.{}", t.schema, t.name).replace('%', "%%");
        let (text, args) = if key.is_empty() {
            (format!("{shown}: no row found"), String::new())
        } else {
            let cond = key
                .iter()
                .map(|c| format!("{} = %", c.name.replace('%', "%%")))
                .collect::<Vec<_>>()
                .join(", ");
            let args = key
                .iter()
                .map(|c| format!(", {}", name_of(c)))
                .collect::<String>();
            (format!("{shown}: no row with {cond}"), args)
        };
        lines.push(("    IF NOT FOUND THEN".to_owned(), String::new()));
        lines.push((
            format!(
                "        RAISE EXCEPTION {}{args};",
                pgc_core::quote::quote_literal(&text)
            ),
            String::new(),
        ));
        lines.push(("    END IF;".to_owned(), String::new()));
    };
    let table = p.table;
    match what {
        Gen::ProcInsert => {
            let cols: Vec<&Param> = params.iter().filter(|q| !q.inout).collect();
            let returning = (!returned.is_empty()).then(|| {
                format!(
                    "    RETURNING {} INTO {};",
                    returned
                        .iter()
                        .map(|c| quote_ident(&c.name))
                        .collect::<Vec<_>>()
                        .join(", "),
                    returned
                        .iter()
                        .map(|c| name_of(c))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
            if cols.is_empty() {
                let end = if returning.is_some() { "" } else { ";" };
                lines.push((
                    format!("    INSERT INTO {table} DEFAULT VALUES{end}"),
                    String::new(),
                ));
            } else {
                lines.push((format!("    INSERT INTO {table} ("), String::new()));
                list(
                    &mut lines,
                    cols.iter().map(|q| quote_ident(&q.col.name)).collect(),
                    "        ",
                );
                lines.push(("    ) VALUES (".to_owned(), String::new()));
                list(
                    &mut lines,
                    cols.iter().map(|q| name_of(q.col)).collect(),
                    "        ",
                );
                let end = if returning.is_some() { "" } else { ";" };
                lines.push((format!("    ){end}"), String::new()));
            }
            if let Some(r) = returning {
                lines.push((r, String::new()));
            }
        }
        Gen::ProcUpdate => {
            lines.push((format!("    UPDATE {table} SET"), String::new()));
            let set: Vec<String> = params
                .iter()
                .filter(|q| !is_key(q.col))
                .map(|q| format!("{} = {}", quote_ident(&q.col.name), name_of(q.col)))
                .collect();
            list(&mut lines, set, "        ");
            where_key(&mut lines);
            not_found(&mut lines);
        }
        Gen::ProcDelete => {
            lines.push((format!("    DELETE FROM {table}"), String::new()));
            where_key(&mut lines);
            not_found(&mut lines);
        }
        _ => {
            lines.push((format!("    INSERT INTO {table} ("), String::new()));
            list(
                &mut lines,
                params.iter().map(|q| quote_ident(&q.col.name)).collect(),
                "        ",
            );
            let values = if p.overriding {
                "    ) OVERRIDING SYSTEM VALUE VALUES ("
            } else {
                "    ) VALUES ("
            };
            lines.push((values.to_owned(), String::new()));
            list(
                &mut lines,
                params.iter().map(|q| name_of(q.col)).collect(),
                "        ",
            );
            lines.push(("    )".to_owned(), String::new()));
            let set: Vec<&Param> = params.iter().filter(|q| !is_key(q.col)).collect();
            if key.is_empty() {
                lines.push((
                    "    ON CONFLICT DO NOTHING;".to_owned(),
                    "no primary key: write a conflict target to update".to_owned(),
                ));
            } else {
                let target = key
                    .iter()
                    .map(|c| quote_ident(&c.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                if set.is_empty() {
                    lines.push((
                        format!("    ON CONFLICT ({target}) DO NOTHING;"),
                        p.key_kind.to_owned(),
                    ));
                } else {
                    lines.push((
                        format!("    ON CONFLICT ({target}) DO UPDATE SET"),
                        p.key_kind.to_owned(),
                    ));
                    for (i, q) in set.iter().enumerate() {
                        let end = if i + 1 < set.len() { "," } else { ";" };
                        let n = quote_ident(&q.col.name);
                        lines.push((format!("        {n} = EXCLUDED.{n}{end}"), String::new()));
                    }
                }
            }
        }
    }
    lines.push(("END;".to_owned(), String::new()));
    lines.push(("$$;".to_owned(), String::new()));
    format!("{header}\n{}", aligned(&lines))
}

/// `CREATE OR REPLACE <kind> <schema>.[<kind>_name](` + one aligned line per
/// parameter + `)`; `kind` is `PROCEDURE` or `FUNCTION`. The parameters' comments
/// are aligned on their own, not with the body.
fn routine_header(kind: &str, schema: &str, params: &[Param], names: &[String]) -> String {
    let head = format!(
        "CREATE OR REPLACE {kind} {}.[{}_name](",
        quote_ident(schema),
        kind.to_lowercase()
    );
    if params.is_empty() {
        return format!("{head})");
    }
    let mut lines = vec![(head, String::new())];
    let shown: Vec<String> = params
        .iter()
        .zip(names)
        .map(|(q, n)| {
            if q.inout {
                format!("INOUT {n}")
            } else {
                n.clone()
            }
        })
        .collect();
    let width = shown.iter().map(|s| s.chars().count()).max().unwrap_or(0);
    for (i, (q, s)) in params.iter().zip(&shown).enumerate() {
        let pad = " ".repeat(width - s.chars().count());
        let default = if q.inout { " DEFAULT NULL" } else { "" };
        let comma = if i + 1 < params.len() { "," } else { "" };
        lines.push((
            format!("    {s}{pad}  {}{default}{comma}", q.col.type_name),
            q.comment.clone(),
        ));
    }
    lines.push((")".to_owned(), String::new()));
    aligned(&lines)
}

/// `gf`: an empty PL/pgSQL function in `schema` (D-054).
pub fn generate_function(schema: &str) -> String {
    [
        format!(
            "CREATE OR REPLACE FUNCTION {}.[function_name]()",
            quote_ident(schema)
        ),
        "RETURNS [return_type]".to_owned(),
        "LANGUAGE plpgsql".to_owned(),
        "AS $$".to_owned(),
        "BEGIN".to_owned(),
        "    --PROCESS HERE".to_owned(),
        "    RETURN [value];".to_owned(),
        "END;".to_owned(),
        "$$;".to_owned(),
    ]
    .join("\n")
}

/// `gfr` / `gfs` / `gft` / `gfe` / `gfj`: a read-only (`STABLE`) PL/pgSQL function
/// over the table (view, foreign table), D-054. `gfr` / `gfe` / `gfj` take the key
/// as `p_` parameters; `gfs` / `gft` filter with `WHERE [condition]`.
fn function(what: Gen, p: &ProcTable) -> String {
    let (t, key, table) = (p.t, p.key, p.table);
    let by_key = matches!(what, Gen::FuncRow | Gen::FuncExists | Gen::FuncJson);
    let params: Vec<Param> = if by_key {
        key.iter()
            .map(|c| Param {
                col: c,
                inout: false,
                comment: p.key_kind.to_owned(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let columns: Vec<&GenColumn> = params.iter().map(|q| q.col).collect();
    let taken: Vec<String> = t.columns.iter().map(|c| c.name.to_lowercase()).collect();
    let names = prefixed_names(&columns, "p_", &taken);
    let header = routine_header("FUNCTION", &t.schema, &params, &names);
    let mut lines: Vec<(String, String)> = Vec::new();
    let push = |lines: &mut Vec<(String, String)>, code: String| {
        lines.push((code, String::new()));
    };
    // RETURNS
    match what {
        Gen::FuncRow => push(&mut lines, format!("RETURNS {table}")),
        Gen::FuncSet => push(&mut lines, format!("RETURNS SETOF {table}")),
        Gen::FuncExists => push(&mut lines, "RETURNS boolean".to_owned()),
        Gen::FuncJson => push(&mut lines, "RETURNS jsonb".to_owned()),
        _ => {
            push(&mut lines, "RETURNS TABLE (".to_owned());
            let width = t
                .columns
                .iter()
                .map(|c| quote_ident(&c.name).chars().count())
                .max()
                .unwrap_or(0);
            let n = t.columns.len();
            for (i, c) in t.columns.iter().enumerate() {
                let name = quote_ident(&c.name);
                let pad = " ".repeat(width - name.chars().count());
                let comma = if i + 1 < n { "," } else { "" };
                push(
                    &mut lines,
                    format!("    {name}{pad}  {}{comma}", c.type_name),
                );
            }
            push(&mut lines, ")".to_owned());
        }
    }
    for l in ["LANGUAGE plpgsql", "STABLE", "AS $$"] {
        push(&mut lines, l.to_owned());
    }
    // gfr's row variable: not a column name (a column of that name would be ambiguous)
    let mut row_var = "v_row".to_owned();
    let mut n = 2;
    while taken.contains(&row_var) {
        row_var = format!("v_row_{n}");
        n += 1;
    }
    if what == Gen::FuncRow {
        push(&mut lines, "DECLARE".to_owned());
        push(&mut lines, format!("    {row_var} {table};"));
    }
    push(&mut lines, "BEGIN".to_owned());
    // gft / gfj name the table T1: the output columns of RETURNS TABLE are variables
    let alias = matches!(what, Gen::FuncTable | Gen::FuncJson);
    let col = |c: &GenColumn| {
        if alias {
            format!("T1.{}", quote_ident(&c.name))
        } else {
            quote_ident(&c.name)
        }
    };
    let from = if alias {
        format!("FROM {table} AS T1")
    } else {
        format!("FROM {table}")
    };
    let where_lines = |lines: &mut Vec<(String, String)>, indent: &str, end: &str| {
        if !by_key {
            let comment = if alias {
                "write a condition; columns as T1.column (the output columns are variables)"
            } else {
                "write a condition"
            };
            lines.push((
                format!("{indent}WHERE [condition]{end}"),
                comment.to_owned(),
            ));
        } else {
            let terms = key
                .iter()
                .zip(&names)
                .map(|(c, n)| (col(c), n.clone(), String::new()));
            where_by_key(lines, indent, "  AND", end, terms.collect());
        }
    };
    let select_all = |lines: &mut Vec<(String, String)>, indent: &str| {
        lines.push((format!("{indent}SELECT"), String::new()));
        listed(
            lines,
            t.columns
                .iter()
                .map(|c| (format!("{indent}    {}", col(c)), String::new())),
        );
    };
    match what {
        Gen::FuncRow => {
            select_all(&mut lines, "    ");
            push(&mut lines, format!("    INTO {row_var}"));
            push(&mut lines, format!("    {from}"));
            where_lines(&mut lines, "    ", ";");
            for l in [
                "    IF NOT FOUND THEN",
                "        RETURN NULL;",
                "    END IF;",
            ] {
                push(&mut lines, l.to_owned());
            }
            push(&mut lines, format!("    RETURN {row_var};"));
        }
        Gen::FuncSet | Gen::FuncTable => {
            push(&mut lines, "    RETURN QUERY".to_owned());
            select_all(&mut lines, "    ");
            push(&mut lines, format!("    {from}"));
            where_lines(&mut lines, "    ", ";");
        }
        Gen::FuncExists => {
            push(&mut lines, "    RETURN EXISTS (".to_owned());
            push(&mut lines, "        SELECT 1".to_owned());
            push(&mut lines, format!("        {from}"));
            where_lines(&mut lines, "        ", "");
            push(&mut lines, "    );".to_owned());
        }
        _ => {
            // NULL when no row was found
            push(&mut lines, "    RETURN (".to_owned());
            push(&mut lines, "        SELECT to_jsonb(T1)".to_owned());
            push(&mut lines, format!("        {from}"));
            where_lines(&mut lines, "        ", "");
            push(&mut lines, "    );".to_owned());
        }
    }
    push(&mut lines, "END;".to_owned());
    push(&mut lines, "$$;".to_owned());
    format!("{header}\n{}", aligned(&lines))
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
    let schema = tables.first().map_or("public", |t| t.schema.as_str());
    let head = format!("CREATE VIEW {}.[view_name] AS", quote_ident(schema));
    joined_select(tables, fks, Some(head))
}

/// `SELECT` every column of `tables` (aliases `T1`, …) `FROM` the first one, each later
/// table joined on a foreign key to an earlier one (`gtv`, `gtj`); `head` goes first.
fn joined_select(tables: &[ViewTable], fks: &[ViewFk], head: Option<String>) -> String {
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
    if let Some(head) = head {
        lines.push((head, String::new()));
    }
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

// ---- gtg / gtj / gtr: foreign keys of one table --------------------------------------

/// A foreign key of the table, from `foreign_keys.sql`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelFk {
    /// The table references the other one (`gtj`, `gtg`); else the other one references
    /// the table (`gtr`).
    pub outgoing: bool,
    pub name: String,
    pub schema: String,
    pub table: String,
    /// (this table's column, the other table's column) in key order.
    pub pairs: Vec<(String, String)>,
    /// A referencing column allows NULL.
    pub nullable: bool,
}

/// The table's foreign keys (by direction, then constraint name) and the columns of
/// the tables on their other side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Related {
    pub fks: Vec<RelFk>,
    pub columns: Vec<((String, String), Vec<String>)>,
}

impl Related {
    fn columns_of(&self, schema: &str, table: &str) -> Vec<String> {
        self.columns
            .iter()
            .find(|((s, t), _)| s == schema && t == table)
            .map(|(_, c)| c.clone())
            .unwrap_or_default()
    }
}

/// `foreign_keys.sql` rows → the foreign keys and the other tables' columns.
pub fn related(rows: Vec<Vec<String>>) -> Related {
    struct Pair {
        outgoing: bool,
        name: String,
        ord: i64,
        schema: String,
        table: String,
        this: String,
        other: String,
        nullable: bool,
    }
    let mut pairs: Vec<Pair> = Vec::new();
    // (schema, table) → (attnum, column)
    type Columns = Vec<((String, String), Vec<(i64, String)>)>;
    let mut columns: Columns = Vec::new();
    for r in rows {
        match r.first().map(String::as_str) {
            Some("k") if r.len() >= 9 => pairs.push(Pair {
                outgoing: r[1] == "o",
                name: r[2].clone(),
                ord: r[3].parse().unwrap_or(0),
                schema: r[4].clone(),
                table: r[5].clone(),
                this: r[6].clone(),
                other: r[7].clone(),
                nullable: r[8] != "true",
            }),
            Some("c") if r.len() >= 5 => {
                let key = (r[1].clone(), r[2].clone());
                let entry = (r[3].parse().unwrap_or(0), r[4].clone());
                match columns.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, cols)) => cols.push(entry),
                    None => columns.push((key, vec![entry])),
                }
            }
            _ => {}
        }
    }
    // outgoing first; then by constraint name, other table, pair number
    pairs.sort_by(|a, b| {
        (!a.outgoing, &a.name, &a.schema, &a.table, a.ord).cmp(&(
            !b.outgoing,
            &b.name,
            &b.schema,
            &b.table,
            b.ord,
        ))
    });
    let mut fks: Vec<RelFk> = Vec::new();
    for p in pairs {
        match fks.last_mut() {
            Some(f)
                if f.outgoing == p.outgoing
                    && f.name == p.name
                    && f.schema == p.schema
                    && f.table == p.table =>
            {
                f.pairs.push((p.this, p.other));
                f.nullable |= p.nullable;
            }
            _ => fks.push(RelFk {
                outgoing: p.outgoing,
                name: p.name,
                schema: p.schema,
                table: p.table,
                pairs: vec![(p.this, p.other)],
                nullable: p.nullable,
            }),
        }
    }
    let columns = columns
        .into_iter()
        .map(|(k, mut cols)| {
            cols.sort();
            (k, cols.into_iter().map(|(_, c)| c).collect())
        })
        .collect();
    Related { fks, columns }
}

/// `gtg` / `gtj` / `gtr` from the table and its foreign keys (D-054). `Err`: nothing
/// to write (`gtr` for a table nothing references).
pub fn generate_related(what: Gen, t: &GenTable, rel: &Related) -> Result<String, String> {
    let table = format!("{}.{}", quote_ident(&t.schema), quote_ident(&t.name));
    match what {
        Gen::JoinParents => Ok(join_parents(t, rel)),
        Gen::Referencing => referencing(&table, t, rel),
        Gen::TestData { rows } => Ok(test_data(&table, t, rel, rows)),
        _ => Ok(generate(what, t)),
    }
}

/// `gtj`: every column of the table and of each table a foreign key points to, one
/// join per foreign key (the same table twice for two keys), by constraint name.
fn join_parents(t: &GenTable, rel: &Related) -> String {
    let mut tables = vec![ViewTable {
        schema: t.schema.clone(),
        name: t.name.clone(),
        columns: t.columns.iter().map(|c| c.name.clone()).collect(),
    }];
    let mut fks = Vec::new();
    for f in rel.fks.iter().filter(|f| f.outgoing) {
        fks.push(ViewFk {
            from: 0,
            to: tables.len(),
            name: f.name.clone(),
            pairs: f.pairs.clone(),
            nullable: f.nullable,
        });
        tables.push(ViewTable {
            schema: f.schema.clone(),
            name: f.table.clone(),
            columns: rel.columns_of(&f.schema, &f.table),
        });
    }
    joined_select(&tables, &fks, None)
}

/// `gtr`: for each foreign key that points to the table, the rows that reference one
/// row of it — `WHERE` on the referencing columns, a `[column]` placeholder for each
/// value of the table's key.
fn referencing(table: &str, t: &GenTable, rel: &Related) -> Result<String, String> {
    let incoming: Vec<&RelFk> = rel.fks.iter().filter(|f| !f.outgoing).collect();
    if incoming.is_empty() {
        return Err(format!(
            "No foreign key references {}.{}.",
            t.schema, t.name
        ));
    }
    let mut out = Vec::new();
    for f in incoming {
        let mut lines: Vec<(String, String)> = vec![(format!("-- {}", f.name), String::new())];
        lines.push(("SELECT".to_owned(), String::new()));
        let cols = rel.columns_of(&f.schema, &f.table);
        listed(
            &mut lines,
            cols.iter()
                .map(|c| (format!("    {}", quote_ident(c)), String::new())),
        );
        lines.push((
            format!("FROM {}.{}", quote_ident(&f.schema), quote_ident(&f.table)),
            String::new(),
        ));
        for (i, (this, other)) in f.pairs.iter().enumerate() {
            let lead = if i == 0 { "WHERE" } else { "    AND" };
            let end = if i + 1 == f.pairs.len() { ";" } else { "" };
            let ty = t
                .columns
                .iter()
                .find(|c| c.name == *this)
                .map(|c| format!("{table}.{}: {}", quote_ident(this), c.type_name))
                .unwrap_or_default();
            lines.push((format!("{lead} {} = [{this}]{end}", quote_ident(other)), ty));
        }
        out.push(aligned(&lines));
    }
    Ok(out.join("\n\n"))
}

/// A value of `gtg` for column `c`: an expression of `g` (1, 2, … the row number) that
/// fits its type, and the line's comment. Key columns get values not yet in the table.
fn test_value(table: &str, c: &GenColumn) -> (String, String) {
    let ty = c.type_name.to_lowercase();
    let unique = c.pk_pos.is_some() || c.uk_pos.is_some();
    let name = quote_ident(&c.name);
    let (base, args) = match ty.find('(') {
        Some(i) => {
            // the `)` of this `(`: a type may be named anything, `"a)b("` too
            let close = ty[i..].find(')').map_or(ty.len(), |j| i + j);
            let after = ty.get(close + 1..).unwrap_or("");
            (
                ty[..i].trim().to_owned() + after.trim_end(),
                ty[i + 1..close].to_owned(),
            )
        }
        None => (ty.clone(), String::new()),
    };
    let nums: Vec<u32> = args
        .split(',')
        .filter_map(|a| a.trim().parse().ok())
        .collect();
    let next = format!("(SELECT coalesce(max({name}), 0) FROM {table}) + g");
    let type_name = &c.type_name;
    let comment = |extra: &str| {
        let mut s = type_comment(c);
        if !extra.is_empty() {
            s.push_str(&format!(", {extra}"));
        }
        s
    };
    if ty.ends_with("[]") {
        return (format!("'{{}}'::{type_name}"), comment("empty array"));
    }
    let expr = match base.as_str() {
        "smallint" | "integer" | "bigint" => {
            if unique {
                next
            } else {
                format!("(random() * 1000)::{type_name}")
            }
        }
        "numeric" | "decimal" => {
            if unique {
                next
            } else if let [p, s] = nums[..] {
                // below the precision: trunc, as round could reach the limit
                let max = 10u64
                    .saturating_pow(p.saturating_sub(s))
                    .saturating_sub(1)
                    .clamp(1, 1000);
                format!("trunc((random() * {max})::numeric, {s})")
            } else if let [p] = nums[..] {
                let max = 10u64.saturating_pow(p).saturating_sub(1).clamp(1, 1000);
                format!("trunc((random() * {max})::numeric)")
            } else {
                "round((random() * 1000)::numeric, 2)".to_owned()
            }
        }
        "real" | "double precision" => format!("round((random() * 1000)::numeric, 2)::{type_name}"),
        "money" => "(random() * 1000)::numeric(10, 2)::money".to_owned(),
        "text" | "character varying" | "varchar" | "character" | "char" | "bpchar" | "citext"
        | "name" => {
            let mut e = format!(
                "{} || g",
                pgc_core::quote::quote_literal(&format!("{} ", c.name))
            );
            if unique {
                e.push_str(" || '-' || substr(md5(random()::text), 1, 8)");
            }
            match nums.first() {
                Some(n) => format!("left({e}, {n})"),
                None => e,
            }
        }
        "boolean" => "random() < 0.5".to_owned(),
        "date" => "current_date - (random() * 365)::integer".to_owned(),
        "timestamp without time zone" => {
            format!("(localtimestamp - random() * interval '365 days')::{type_name}")
        }
        "timestamp with time zone" => {
            format!("(now() - random() * interval '365 days')::{type_name}")
        }
        "time without time zone" | "time with time zone" => {
            format!("(time '00:00' + random() * interval '24 hours')::{type_name}")
        }
        "interval" => "random() * interval '30 days'".to_owned(),
        "uuid" => "md5(random()::text || g)::uuid".to_owned(),
        "json" => "json_build_object('n', g)".to_owned(),
        "jsonb" => "jsonb_build_object('n', g)".to_owned(),
        "bytea" => "decode(md5(g::text), 'hex')".to_owned(),
        "inet" | "cidr" => format!(
            "('10.' || (g / 65536) % 256 || '.' || (g / 256) % 256 || '.' || g % 256)::{type_name}"
        ),
        _ => {
            return if c.not_null {
                (format!("[{}]", c.name), comment("write a value"))
            } else {
                ("NULL".to_owned(), comment("write a value or keep NULL"))
            };
        }
    };
    let extra = if unique { "not yet in the table" } else { "" };
    (expr, comment(extra))
}

/// `gtg`: `INSERT … SELECT … FROM generate_series(1, rows) AS g` with a value for each
/// column the database does not fill; a foreign key's columns come from a random row
/// of its table (a `LEFT JOIN LATERAL` per key, evaluated for each row).
fn test_data(table: &str, t: &GenTable, rel: &Related, rows: u32) -> String {
    let outgoing: Vec<&RelFk> = rel.fks.iter().filter(|f| f.outgoing).collect();
    // the first foreign key (by name) that holds a column fills it
    let fk_of = |c: &GenColumn| {
        outgoing
            .iter()
            .enumerate()
            .find(|(_, f)| f.pairs.iter().any(|(this, _)| *this == c.name))
    };
    let cols: Vec<&GenColumn> = t
        .columns
        .iter()
        .filter(|c| !c.generated && c.identity.is_empty())
        .filter(|c| c.default.is_none() || fk_of(c).is_some())
        .collect();
    // the foreign keys used, numbered p1, p2, … in the order of their first column
    let mut used: Vec<usize> = Vec::new();
    for c in &cols {
        if let Some((k, _)) = fk_of(c)
            && !used.contains(&k)
        {
            used.push(k);
        }
    }
    let alias = |k: usize| format!("p{}", used.iter().position(|u| *u == k).unwrap_or(0) + 1);
    let mut lines: Vec<(String, String)> = Vec::new();
    if cols.is_empty() {
        lines.push((format!("INSERT INTO {table}"), String::new()));
        lines.push(("SELECT".to_owned(), String::new()));
    } else {
        lines.push((format!("INSERT INTO {table} ("), String::new()));
        listed(
            &mut lines,
            cols.iter()
                .map(|c| (format!("    {}", quote_ident(&c.name)), String::new())),
        );
        lines.push((")".to_owned(), String::new()));
        lines.push(("SELECT".to_owned(), String::new()));
        for (i, c) in cols.iter().enumerate() {
            let comma = if i + 1 < cols.len() { "," } else { "" };
            let (expr, comment) = match fk_of(c) {
                Some((k, f)) => {
                    let other = &f
                        .pairs
                        .iter()
                        .find(|(this, _)| *this == c.name)
                        .expect("pair")
                        .1;
                    (
                        format!("{}.{}", alias(k), quote_ident(other)),
                        format!("{}, a random row of {}.{}", f.name, f.schema, f.table),
                    )
                }
                None => test_value(table, c),
            };
            lines.push((format!("    {expr}{comma}"), comment));
        }
    }
    // the values' comments are aligned on their own, not with the long joins
    let mut out = vec![aligned(&lines)];
    out.push(format!("FROM generate_series(1, {rows}) AS g"));
    if !used.is_empty() {
        out.push(
            "-- \"WHERE g IS NOT NULL\": a new random row for each row, not one for all".to_owned(),
        );
    }
    for k in &used {
        let f = outgoing[*k];
        let other_cols = f
            .pairs
            .iter()
            .map(|(_, o)| quote_ident(o))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!(
            "LEFT JOIN LATERAL (SELECT {other_cols} FROM {}.{} WHERE g IS NOT NULL ORDER BY random() LIMIT 1) AS {} ON true",
            quote_ident(&f.schema),
            quote_ident(&f.table),
            alias(*k)
        ));
    }
    if let Some(last) = out.last_mut() {
        last.push(';');
    }
    out.join("\n")
}

/// `gtau`: `<table>_audit` (when, who, operation, the old and new row as `jsonb`), a
/// trigger function that writes into it and an `AFTER … FOR EACH ROW` trigger (D-054).
fn audit(table: &str, t: &GenTable) -> String {
    let schema = quote_ident(&t.schema);
    let audit = format!("{schema}.{}", quote_ident(&format!("{}_audit", t.name)));
    let function = format!("{schema}.[function_name]");
    let trigger = quote_ident(&format!("{}_audit_trg", t.name));
    [
        format!("CREATE TABLE {audit} ("),
        "    audit_id    bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,".to_owned(),
        "    changed_at  timestamp with time zone NOT NULL DEFAULT now(),".to_owned(),
        "    changed_by  text NOT NULL DEFAULT current_user,".to_owned(),
        "    operation   text NOT NULL,  -- INSERT, UPDATE or DELETE".to_owned(),
        "    old_row     jsonb,          -- UPDATE, DELETE".to_owned(),
        "    new_row     jsonb           -- INSERT, UPDATE".to_owned(),
        ");".to_owned(),
        String::new(),
        format!("CREATE OR REPLACE FUNCTION {function}()"),
        "RETURNS trigger".to_owned(),
        "LANGUAGE plpgsql".to_owned(),
        "AS $$".to_owned(),
        "BEGIN".to_owned(),
        format!("    INSERT INTO {audit} (operation, old_row, new_row)"),
        "    VALUES (".to_owned(),
        "        TG_OP,".to_owned(),
        "        CASE WHEN TG_OP IN ('UPDATE', 'DELETE') THEN to_jsonb(OLD) END,".to_owned(),
        "        CASE WHEN TG_OP IN ('INSERT', 'UPDATE') THEN to_jsonb(NEW) END".to_owned(),
        "    );".to_owned(),
        "    RETURN NULL;  -- ignored for AFTER triggers".to_owned(),
        "END;".to_owned(),
        "$$;".to_owned(),
        String::new(),
        format!("CREATE TRIGGER {trigger}"),
        format!("    AFTER INSERT OR UPDATE OR DELETE ON {table}"),
        format!("    FOR EACH ROW EXECUTE FUNCTION {function}();"),
    ]
    .join("\n")
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
        // a quoted `*` is part of the name
        assert!(matches!(parse("d \"a*b\""), Ok(Cmd::Describe { .. })));
        assert!(parse("d \"a\"*").is_err());
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
    fn double_quotes_are_read_one_way() {
        // where a text is cut: not inside quotes, a doubled quote does not end them
        assert_eq!(
            split_unquoted("a \"b c\"  d", char::is_whitespace),
            ["a", "\"b c\"", "", "d"]
        );
        assert_eq!(
            split_unquoted("s.\"a.\"\"b\".c", |c| c == '.'),
            ["s", "\"a.\"\"b\"", "c"]
        );
        // what it says: the marks gone, a doubled quote one quote
        let said: String = dequote("x\"a.\"\"b\"")
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        assert_eq!(said, "xa.\"b");
        assert_eq!(dequote("a\"b\""), [('a', false), ('b', true)]);
        // and what is built on the two
        assert_eq!(
            split_args("  orders \"Order Items\" s.x "),
            ["orders", "\"Order Items\"", "s.x"]
        );
        assert_eq!(last_arg("gtv a \"b c\""), "\"b c\"");
        assert_eq!(last_arg("gtv a "), "");
        assert_eq!(
            split_parts("Sales.\"Order \"\"A\"\"\""),
            [
                ("Sales".to_owned(), false),
                ("Order \"A\"".to_owned(), true)
            ]
        );
        assert_eq!(
            parse_name("Sales.\"My.Table\""),
            Ok((Some("sales".into()), "My.Table".into()))
        );
    }

    #[test]
    fn psql_patterns() {
        assert_eq!(pattern_regex("ord*"), "^(ord.*)$");
        assert_eq!(pattern_regex("Ord?r"), "^(ord.r)$");
        assert_eq!(pattern_regex("\"My*T\""), "^(My\\*T)$");
        assert_eq!(pattern_regex("a.b"), "^(a\\.b)$");
        // regular expression syntax passes through, as in psql; quoted it is text
        assert_eq!(pattern_regex("(orders|items)"), "^((orders|items))$");
        assert_eq!(pattern_regex("t[0-9]*"), "^(t[0-9].*)$");
        assert_eq!(pattern_regex("\"a(b\""), "^(a\\(b)$");
        assert_eq!(pattern_regex("a$b"), "^(a\\$b)$");
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
    fn schemas_are_offered_where_a_name_can_be_qualified() {
        let t = |s: &str| completion_of(s).map(|(a, k, schemas)| (a.to_owned(), k, schemas));
        assert_eq!(t("dt ord"), Some(("ord".into(), vec!["table"], true)));
        assert!(t("gts o").unwrap().2);
        assert_eq!(t("gtv a b").unwrap().0, "b");
        // no names of their own: schemas only
        for cmd in ["di", "df", "dT", "dD", "df+", "\\diS"] {
            assert_eq!(t(&format!("{cmd} x")), Some(("x".into(), vec![], true)));
        }
        // a schema itself, or no schema part at all
        assert_eq!(t("gp sa"), Some(("sa".into(), vec!["schema"], false)));
        assert_eq!(t("du x"), None);
        assert_eq!(t("dn x"), None);
        assert_eq!(t("l x"), None);
        assert_eq!(t("df"), None, "no argument yet");
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
        // one letter that has no row of its own
        assert_eq!(t("dE x"), Some(("x".into(), vec!["foreign table"])));
        assert_eq!(t("d+ x").unwrap().1, RELATIONS_SORTED);
        assert_eq!(t("gts o").unwrap().1, ["foreign table", "table", "view"]);
        assert_eq!(t("gtm o").unwrap().1, ["table"]);
        for input in ["gtrb o", "gtra o", "gtsb o", "gtsa o"] {
            assert_eq!(t(input).unwrap().1, ["table"]);
        }
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
                "gts", "gti", "gtu", "gtd", "gtc", "gto", "gtsi", "gtv", "gtm", "gtrb", "gtra",
                "gtsb", "gtsa", "gtg", "gtj", "gtr", "gtau"
            ]
        );
        assert_eq!(matching("\\gt").len(), 17);
        let m: Vec<&str> = matching("gp").iter().map(|c| c.0).collect();
        assert_eq!(m, ["gp", "gpi", "gpu", "gpd", "gpo"]);
        let m: Vec<&str> = matching("gf").iter().map(|c| c.0).collect();
        assert_eq!(m, ["gf", "gfr", "gfs", "gft", "gfe", "gfj"]);
        assert_eq!(matching("dt+ x").len(), 1);
    }

    #[test]
    /// The one list of the table commands agrees with the rows shown to the user:
    /// every command has a row, and which work on views too is said once.
    fn table_commands_and_their_rows_agree() {
        for (command, what) in TABLE_COMMANDS {
            assert_eq!(what.command(), *command);
            assert!(GENERATE.iter().any(|r| r.0 == *command), "{command}");
            assert!(needs_table(command), "{command}");
            assert!(matches!(
                parse(&format!("{command} t")),
                Ok(Cmd::Generate { what: w, .. }) if w == *what
            ));
        }
        let selectable: Vec<&str> = TABLE_COMMANDS
            .iter()
            .filter(|(_, what)| !what.tables_only())
            .map(|(command, _)| *command)
            .collect();
        assert_eq!(
            selectable,
            [
                "gts", "gti", "gtu", "gtd", "gtc", "gto", "gtsi", "gfr", "gfs", "gft", "gfe", "gfj"
            ]
        );
        assert!(Gen::TestData { rows: 1 }.tables_only());
        assert!(needs_table("gtv") && needs_table("\\gtg") && !needs_table("dt"));
    }

    #[test]
    fn update_of_a_table_with_nothing_to_set() {
        // only a GENERATED ALWAYS identity key and a generated column
        let mut t = orders();
        t.columns.retain(|c| c.name == "id" || c.name == "total");
        t.columns[0].default = None;
        t.columns[0].identity = "a".into();
        assert_eq!(
            generate(Gen::Update, &t),
            "-- public.orders has no column that can be set: nothing to update"
        );
    }

    #[test]
    fn insert_into_a_table_the_database_fills() {
        // only a serial key and a generated column: nothing to list
        let mut t = orders();
        t.columns.retain(|c| c.name == "id" || c.name == "total");
        assert_eq!(
            generate(Gen::Insert, &t),
            "INSERT INTO public.orders DEFAULT VALUES;"
        );
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
    fn generate_procedure_skeleton() {
        assert_eq!(
            parse("gp"),
            Ok(Cmd::Procedure {
                schema: "public".into()
            })
        );
        assert_eq!(
            parse("gp \"My Schema\""),
            Ok(Cmd::Procedure {
                schema: "My Schema".into()
            })
        );
        assert!(parse("gp a.b").is_err());
        assert!(!needs_table("gp"));
        assert_eq!(
            generate_procedure("My Schema"),
            "CREATE OR REPLACE PROCEDURE \"My Schema\".[procedure_name]()\nLANGUAGE plpgsql\nAS $$\nBEGIN\n    --PROCESS HERE\nEND;\n$$;"
        );
        assert_eq!(
            completion_target("gp sa").map(|(a, k)| (a.to_owned(), k)),
            Some(("sa".into(), vec!["schema"]))
        );
    }

    #[test]
    fn generate_table_procedures() {
        for (cmd, what) in [
            ("gpi", Gen::ProcInsert),
            ("gpu", Gen::ProcUpdate),
            ("gpd", Gen::ProcDelete),
            ("gpo", Gen::ProcUpsert),
        ] {
            assert!(needs_table(cmd));
            assert!(what.tables_only());
            assert_eq!(what.command(), cmd);
            assert_eq!(
                parse(&format!("{cmd} orders")),
                Ok(Cmd::Generate {
                    what,
                    table: (None, "orders".into())
                })
            );
            assert_eq!(completion_target(&format!("{cmd} o")).unwrap().1, ["table"]);
        }
        let t = orders();
        let insert = [
            "CREATE OR REPLACE PROCEDURE public.[procedure_name](",
            "    p_customer_id  integer,              -- NOT NULL",
            "    p_note         text,",
            "    INOUT p_id     integer DEFAULT NULL  -- primary key, filled by the database",
            ")",
            "LANGUAGE plpgsql",
            "AS $$",
            "BEGIN",
            "    INSERT INTO public.orders (",
            "        customer_id,",
            "        note",
            "    ) VALUES (",
            "        p_customer_id,",
            "        p_note",
            "    )",
            "    RETURNING id INTO p_id;",
            "END;",
            "$$;",
        ];
        // NOT NULL goes in the comment; the type is on the line
        assert_eq!(generate(Gen::ProcInsert, &t), insert.join("\n"));
        let update = [
            "CREATE OR REPLACE PROCEDURE public.[procedure_name](",
            "    p_id           integer,  -- primary key",
            "    p_customer_id  integer,  -- NOT NULL",
            "    p_note         text",
            ")",
            "LANGUAGE plpgsql",
            "AS $$",
            "BEGIN",
            "    UPDATE public.orders SET",
            "        customer_id = p_customer_id,",
            "        note = p_note",
            "    WHERE id = p_id;",
            "    IF NOT FOUND THEN",
            "        RAISE EXCEPTION 'public.orders: no row with id = %', p_id;",
            "    END IF;",
            "END;",
            "$$;",
        ];
        assert_eq!(generate(Gen::ProcUpdate, &t), update.join("\n"));
        let delete = [
            "CREATE OR REPLACE PROCEDURE public.[procedure_name](",
            "    p_id  integer  -- primary key",
            ")",
            "LANGUAGE plpgsql",
            "AS $$",
            "BEGIN",
            "    DELETE FROM public.orders",
            "    WHERE id = p_id;",
            "    IF NOT FOUND THEN",
            "        RAISE EXCEPTION 'public.orders: no row with id = %', p_id;",
            "    END IF;",
            "END;",
            "$$;",
        ];
        assert_eq!(generate(Gen::ProcDelete, &t), delete.join("\n"));
        let upsert = generate(Gen::ProcUpsert, &t);
        assert!(upsert.contains("    p_id           integer,  -- primary key\n"));
        assert!(upsert.contains(
            "    ) VALUES (\n        p_id,\n        p_customer_id,\n        p_note\n    )\n"
        ));
        assert!(upsert.contains("    ON CONFLICT (id) DO UPDATE SET"));
        assert!(upsert.contains("        note = EXCLUDED.note;\nEND;"));
        assert!(!nothing_to_update(&t));
    }

    #[test]
    fn table_procedures_without_key_and_odd_names() {
        // no key: a written condition, a plain message; names clash with a column
        let t = GenTable {
            schema: "public".into(),
            name: "50%".into(),
            columns: vec![col("p_a", "text", false), col("A", "integer", false)],
            unique: None,
            kind: "r".into(),
        };
        let d = generate(Gen::ProcDelete, &t);
        assert!(d.starts_with("CREATE OR REPLACE PROCEDURE public.[procedure_name]()\n"));
        assert!(d.contains("    WHERE [condition];  -- no primary key: write a condition"));
        assert!(d.contains("RAISE EXCEPTION 'public.50%%: no row found';"));
        let u = generate(Gen::ProcUpdate, &t);
        // "A" → p_a, which is a column name: p_a_2; "p_a" → p_p_a
        assert!(
            u.contains("        p_a = p_p_a,\n        \"A\" = p_a_2\n"),
            "{u}"
        );
        let i = generate(Gen::ProcInsert, &t);
        assert!(
            i.contains("    );\nEND;"),
            "no RETURNING without a key: {i}"
        );
        // only key columns: nothing to update
        let k = GenTable {
            columns: vec![GenColumn {
                pk_pos: Some(1),
                ..col("id", "integer", true)
            }],
            ..t.clone()
        };
        assert!(nothing_to_update(&k));
    }

    #[test]
    fn generate_function_skeleton() {
        assert_eq!(
            parse("gf"),
            Ok(Cmd::Function {
                schema: "public".into()
            })
        );
        assert_eq!(
            parse("gf Sales"),
            Ok(Cmd::Function {
                schema: "sales".into()
            })
        );
        assert!(
            parse("gf a.b")
                .unwrap_err()
                .contains("gf takes a schema name")
        );
        assert!(!needs_table("gf"));
        assert_eq!(
            generate_function("public"),
            "CREATE OR REPLACE FUNCTION public.[function_name]()\nRETURNS [return_type]\nLANGUAGE plpgsql\nAS $$\nBEGIN\n    --PROCESS HERE\n    RETURN [value];\nEND;\n$$;"
        );
        assert_eq!(completion_target("gf x").unwrap().1, ["schema"]);
    }

    #[test]
    fn generate_table_functions() {
        for (cmd, what) in [
            ("gfr", Gen::FuncRow),
            ("gfs", Gen::FuncSet),
            ("gft", Gen::FuncTable),
            ("gfe", Gen::FuncExists),
            ("gfj", Gen::FuncJson),
        ] {
            assert!(needs_table(cmd));
            assert!(!what.tables_only(), "views and foreign tables too");
            assert_eq!(what.command(), cmd);
            assert_eq!(
                parse(&format!("{cmd} orders")),
                Ok(Cmd::Generate {
                    what,
                    table: (None, "orders".into())
                })
            );
            assert_eq!(
                completion_target(&format!("{cmd} o")).unwrap().1,
                ["foreign table", "table", "view"]
            );
            let text = generate(what, &orders());
            assert!(
                text.contains("\nLANGUAGE plpgsql\nSTABLE\nAS $$\n"),
                "{text}"
            );
            assert!(text.ends_with("END;\n$$;"), "{text}");
        }
        let t = orders();
        let row = [
            "CREATE OR REPLACE FUNCTION public.[function_name](",
            "    p_id  integer  -- primary key",
            ")",
            "RETURNS public.orders",
            "LANGUAGE plpgsql",
            "STABLE",
            "AS $$",
            "DECLARE",
            "    v_row public.orders;",
            "BEGIN",
            "    SELECT",
            "        id,",
            "        customer_id,",
            "        note,",
            "        total",
            "    INTO v_row",
            "    FROM public.orders",
            "    WHERE id = p_id;",
            "    IF NOT FOUND THEN",
            "        RETURN NULL;",
            "    END IF;",
            "    RETURN v_row;",
            "END;",
            "$$;",
        ];
        assert_eq!(generate(Gen::FuncRow, &t), row.join("\n"));
        let set = generate(Gen::FuncSet, &t);
        assert!(set.starts_with(
            "CREATE OR REPLACE FUNCTION public.[function_name]()\nRETURNS SETOF public.orders\n"
        ));
        assert!(set.contains("    RETURN QUERY\n    SELECT\n        id,\n"));
        assert!(
            set.contains("    FROM public.orders\n    WHERE [condition];  -- write a condition\n")
        );
        let table = generate(Gen::FuncTable, &t);
        assert!(table.contains(
            "RETURNS TABLE (\n    id           integer,\n    customer_id  integer,\n    note         text,\n    total        numeric\n)\n"
        ), "{table}");
        assert!(table.contains("        T1.id,\n"), "{table}");
        assert!(table.contains("    FROM public.orders AS T1\n    WHERE [condition];"));
        assert!(generate(Gen::FuncExists, &t).contains(
            "    RETURN EXISTS (\n        SELECT 1\n        FROM public.orders\n        WHERE id = p_id\n    );\n"
        ));
        assert!(generate(Gen::FuncJson, &t).contains("RETURNS jsonb\n"));
        assert!(generate(Gen::FuncJson, &t).contains(
            "    RETURN (\n        SELECT to_jsonb(T1)\n        FROM public.orders AS T1\n        WHERE T1.id = p_id\n    );\n"
        ));
        // a view (no key): no parameters, a written condition; a column v_row
        let v = GenTable {
            kind: "v".into(),
            columns: vec![col("v_row", "text", false)],
            ..orders()
        };
        let r = generate(Gen::FuncRow, &v);
        assert!(r.starts_with("CREATE OR REPLACE FUNCTION public.[function_name]()\n"));
        assert!(r.contains("    v_row_2 public.orders;"), "{r}");
        assert!(r.contains("    WHERE [condition];  -- no primary key: write a condition"));
    }

    fn k(
        dir: &str,
        name: &str,
        ord: u32,
        table: &str,
        this: &str,
        other: &str,
        nn: bool,
    ) -> Vec<String> {
        [
            "k",
            dir,
            name,
            &ord.to_string(),
            "public",
            table,
            this,
            other,
            if nn { "true" } else { "false" },
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn c(table: &str, n: u32, name: &str) -> Vec<String> {
        ["c", "public", table, &n.to_string(), name, "", "", "", ""]
            .map(str::to_owned)
            .to_vec()
    }

    /// orders → customers (NOT NULL), orders → orders (parent_id, nullable);
    /// order_lines → orders (two columns), invoices → orders.
    fn orders_related() -> Related {
        related(vec![
            c("customers", 2, "name"),
            c("customers", 1, "id"),
            k(
                "o",
                "orders_parent_fkey",
                1,
                "orders",
                "parent_id",
                "id",
                false,
            ),
            k(
                "o",
                "orders_customer_fkey",
                1,
                "customers",
                "customer_id",
                "id",
                true,
            ),
            k(
                "i",
                "lines_order_fkey",
                2,
                "order_lines",
                "id",
                "order_id",
                true,
            ),
            k(
                "i",
                "lines_order_fkey",
                1,
                "order_lines",
                "id",
                "order_id",
                true,
            ),
            k(
                "i",
                "invoices_order_fkey",
                1,
                "invoices",
                "id",
                "order_id",
                false,
            ),
            c("orders", 1, "id"),
            c("orders", 2, "customer_id"),
            c("order_lines", 1, "order_id"),
            c("order_lines", 2, "line_no"),
            c("invoices", 1, "invoice_id"),
            c("invoices", 2, "order_id"),
        ])
    }

    fn orders_with_parent() -> GenTable {
        let mut t = orders();
        t.columns.insert(2, col("parent_id", "integer", false));
        t
    }

    #[test]
    fn foreign_keys_of_a_table() {
        let r = orders_related();
        let names: Vec<(bool, &str)> = r
            .fks
            .iter()
            .map(|f| (f.outgoing, f.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (true, "orders_customer_fkey"),
                (true, "orders_parent_fkey"),
                (false, "invoices_order_fkey"),
                (false, "lines_order_fkey"),
            ]
        );
        assert!(!r.fks[0].nullable && r.fks[1].nullable);
        // two pairs of one constraint, in key order
        assert_eq!(r.fks[3].pairs.len(), 2);
        assert_eq!(r.columns_of("public", "customers"), ["id", "name"]);
        assert!(r.columns_of("public", "nothing").is_empty());
    }

    #[test]
    fn generate_test_data_joins_and_referencing_rows() {
        assert_eq!(
            parse("gtg orders"),
            Ok(Cmd::Generate {
                what: Gen::TestData { rows: TEST_ROWS },
                table: (None, "orders".into())
            })
        );
        assert_eq!(
            parse("gtg sales.\"Order Items\" 1000"),
            Ok(Cmd::Generate {
                what: Gen::TestData { rows: 1000 },
                table: (Some("sales".into()), "Order Items".into())
            })
        );
        for bad in ["gtg", "gtg orders 0", "gtg orders x", "gtg a b c"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        for (cmd, what) in [
            ("gtj", Gen::JoinParents),
            ("gtr", Gen::Referencing),
            ("gtau", Gen::Audit),
        ] {
            assert!(needs_table(cmd));
            assert!(what.tables_only());
            assert_eq!(what.command(), cmd);
            assert_eq!(what.needs_keys(), what != Gen::Audit);
        }
        assert!(Gen::TestData { rows: 1 }.tables_only());
        let t = orders_with_parent();
        let rel = orders_related();
        // gtj: one join per outgoing key; the same table again is numbered
        let j = generate_related(Gen::JoinParents, &t, &rel).unwrap();
        let expected = [
            "SELECT",
            "    T1.id,",
            "    T1.customer_id,",
            "    T1.parent_id,",
            "    T1.note,",
            "    T1.total,",
            "    T2.id AS customers_id,",
            "    T2.name,",
            "    T3.id AS orders_2_id,",
            "    T3.customer_id AS orders_2_customer_id",
            "FROM public.orders AS T1",
            "JOIN public.customers AS T2 ON T2.id = T1.customer_id   -- orders_customer_fkey",
            "LEFT JOIN public.orders AS T3 ON T3.id = T1.parent_id;  -- orders_parent_fkey",
        ];
        assert_eq!(j, expected.join("\n"));
        // gtr: one SELECT per incoming key, placeholders for this table's key
        let r = generate_related(Gen::Referencing, &t, &rel).unwrap();
        let expected = [
            "-- invoices_order_fkey",
            "SELECT",
            "    invoice_id,",
            "    order_id",
            "FROM public.invoices",
            "WHERE order_id = [id];  -- public.orders.id: integer",
            "",
            "-- lines_order_fkey",
            "SELECT",
            "    order_id,",
            "    line_no",
            "FROM public.order_lines",
            "WHERE order_id = [id]     -- public.orders.id: integer",
            "    AND order_id = [id];  -- public.orders.id: integer",
        ];
        assert_eq!(r, expected.join("\n"));
        let none = generate_related(Gen::Referencing, &t, &Related::default());
        assert_eq!(none, Err("No foreign key references public.orders.".into()));
        // gtg: serial id and the generated total left out; keys from random rows
        let g = generate_related(Gen::TestData { rows: 50 }, &t, &rel).unwrap();
        let expected = [
            "INSERT INTO public.orders (",
            "    customer_id,",
            "    parent_id,",
            "    note",
            ")",
            "SELECT",
            "    p1.id,",
            "    p2.id,",
            "    'note ' || g",
            "FROM generate_series(1, 50) AS g",
            "-- \"WHERE g IS NOT NULL\": a new random row for each row, not one for all",
            "LEFT JOIN LATERAL (SELECT id FROM public.customers WHERE g IS NOT NULL ORDER BY random() LIMIT 1) AS p1 ON true",
            "LEFT JOIN LATERAL (SELECT id FROM public.orders WHERE g IS NOT NULL ORDER BY random() LIMIT 1) AS p2 ON true;",
        ];
        let code: Vec<&str> = g
            .lines()
            .map(|l| l.split("  --").next().unwrap().trim_end())
            .collect();
        assert_eq!(code, expected, "{g}");
        assert!(g.contains("-- orders_customer_fkey, a random row of public.customers"));
        assert!(g.contains("\n-- \"WHERE g IS NOT NULL\": a new random row for each row"));
        assert!(g.contains("    'note ' || g  -- text\n"), "{g}");
    }

    #[test]
    fn test_values_by_type() {
        let v = |ty: &str, not_null: bool| test_value("public.t", &col("c", ty, not_null)).0;
        // a type named to confuse: no slicing behind its end
        v("\"a)b(\"", true);
        v("numeric(10", true);
        assert_eq!(v("integer", true), "(random() * 1000)::integer");
        assert_eq!(
            v("numeric(5,2)", true),
            "trunc((random() * 999)::numeric, 2)"
        );
        assert_eq!(v("numeric(2,2)", true), "trunc((random() * 1)::numeric, 2)");
        assert_eq!(v("numeric", true), "round((random() * 1000)::numeric, 2)");
        assert_eq!(v("character varying(5)", true), "left('c ' || g, 5)");
        assert_eq!(v("text", true), "'c ' || g");
        assert_eq!(v("boolean", true), "random() < 0.5");
        assert_eq!(
            v("timestamp(0) without time zone", true),
            "(localtimestamp - random() * interval '365 days')::timestamp(0) without time zone"
        );
        assert_eq!(v("integer[]", true), "'{}'::integer[]");
        assert_eq!(v("mood", true), "[c]");
        assert_eq!(v("mood", false), "NULL");
        // keys: values not yet in the table
        let key = GenColumn {
            pk_pos: Some(1),
            ..col("Code", "text", true)
        };
        let (e, comment) = test_value("public.t", &key);
        assert_eq!(
            e,
            "'Code ' || g || '-' || substr(md5(random()::text), 1, 8)"
        );
        assert!(comment.ends_with("not yet in the table"));
        let id = GenColumn {
            pk_pos: Some(1),
            ..col("id", "bigint", true)
        };
        assert_eq!(
            test_value("public.t", &id).0,
            "(SELECT coalesce(max(id), 0) FROM public.t) + g"
        );
    }

    #[test]
    fn generate_audit() {
        let a = generate(Gen::Audit, &orders());
        assert!(a.starts_with("CREATE TABLE public.orders_audit (\n"), "{a}");
        assert!(a.contains("INSERT INTO public.orders_audit (operation, old_row, new_row)"));
        assert!(a.contains("CREATE OR REPLACE FUNCTION public.[function_name]()\nRETURNS trigger"));
        assert!(a.ends_with(
            "CREATE TRIGGER orders_audit_trg\n    AFTER INSERT OR UPDATE OR DELETE ON public.orders\n    FOR EACH ROW EXECUTE FUNCTION public.[function_name]();"
        ));
        let mut t = orders();
        t.name = "Order Items".into();
        let a = generate(Gen::Audit, &t);
        assert!(a.starts_with("CREATE TABLE public.\"Order Items_audit\" (\n"));
        assert!(a.contains("CREATE TRIGGER \"Order Items_audit_trg\"\n"));
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
        for c in ["gtrb", "gtra", "gtsb", "gtsa"] {
            assert!(needs_table(c));
        }
        let kind = |c: &str| match parse(&format!("{c} orders")).unwrap() {
            Cmd::Generate { what, .. } => what,
            other => panic!("{other:?}"),
        };
        let trig = |row, before| Gen::Trigger { row, before };
        assert_eq!(kind("gtrb"), trig(true, true));
        assert_eq!(kind("gtra"), trig(true, false));
        assert_eq!(kind("gtsb"), trig(false, true));
        assert_eq!(kind("gtsa"), trig(false, false));
        let head = [
            "CREATE OR REPLACE FUNCTION public.[function_name]()",
            "RETURNS trigger",
            "LANGUAGE plpgsql",
            "AS $$",
            "BEGIN",
        ];
        let fields = [
            "    -- NEW. / OLD.id           integer, NOT NULL",
            "    -- NEW. / OLD.customer_id  integer, NOT NULL",
            "    -- NEW. / OLD.note         text",
            "    -- NEW. / OLD.total        numeric",
        ];
        let make = |row, before| generate(trig(row, before), &orders());
        let join = |parts: &[&[&str]]| parts.concat().join("\n");
        assert_eq!(
            make(true, true),
            join(&[
                &head,
                &fields,
                &[
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
                    "    RETURN NULL;  -- NULL skips the row",
                    "END;",
                    "$$;",
                    "",
                    "CREATE TRIGGER orders_rb_trg",
                    "    BEFORE INSERT OR UPDATE OR DELETE ON public.orders",
                    "    FOR EACH ROW EXECUTE FUNCTION public.[function_name]();",
                ],
            ])
        );
        assert_eq!(
            make(true, false),
            join(&[
                &head,
                &fields,
                &[
                    "    IF TG_OP = 'INSERT' THEN",
                    "        --PROCESS NEW HERE",
                    "    ELSIF TG_OP = 'UPDATE' THEN",
                    "        --PROCESS OLD AND NEW HERE",
                    "    ELSIF TG_OP = 'DELETE' THEN",
                    "        --PROCESS OLD HERE",
                    "    END IF;",
                    "    RETURN NULL;  -- ignored for AFTER triggers",
                    "END;",
                    "$$;",
                    "",
                    "CREATE TRIGGER orders_ra_trg",
                    "    AFTER INSERT OR UPDATE OR DELETE ON public.orders",
                    "    FOR EACH ROW EXECUTE FUNCTION public.[function_name]();",
                ],
            ])
        );
        let body = [
            "    IF TG_OP = 'INSERT' THEN",
            "        --PROCESS INSERT HERE",
            "    ELSIF TG_OP = 'UPDATE' THEN",
            "        --PROCESS UPDATE HERE",
            "    ELSIF TG_OP = 'DELETE' THEN",
            "        --PROCESS DELETE HERE",
            "    ELSIF TG_OP = 'TRUNCATE' THEN",
            "        --PROCESS TRUNCATE HERE",
            "    END IF;",
            "    RETURN NULL;  -- ignored for statement triggers",
            "END;",
            "$$;",
            "",
        ];
        let each = "    FOR EACH STATEMENT EXECUTE FUNCTION public.[function_name]();";
        assert_eq!(
            make(false, true),
            join(&[
                &head,
                &body,
                &[
                    "CREATE TRIGGER orders_sb_trg",
                    "    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON public.orders",
                    each,
                ],
            ])
        );
        assert_eq!(
            make(false, false),
            join(&[
                &head,
                &body,
                &[
                    "CREATE TRIGGER orders_sa_trg",
                    "    AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON public.orders",
                    each,
                ],
            ])
        );
        // names quoted when needed
        let t = GenTable {
            schema: "Sales".into(),
            name: "Order Items".into(),
            ..orders()
        };
        let g = generate(trig(false, false), &t);
        assert!(
            g.starts_with("CREATE OR REPLACE FUNCTION \"Sales\".[function_name]()"),
            "{g}"
        );
        assert!(g.contains("CREATE TRIGGER \"Order Items_sa_trg\""), "{g}");
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
