//! AI assistant (D-073): providers, the structure and editor text that are sent, the
//! request of each protocol and its answer. The SQL that comes back goes into the
//! editor and is never run.
//!
//! Everything here is pure logic, except [`send`] (the HTTP request, run in a tokio
//! task) and [`api_key`] (reads the key file).

use std::collections::BTreeMap;

use pgc_core::quote::{quote_ident, quote_qualified};
use pgc_core::settings::{AiEffort, AiModel, AiProtocol, AiSettings};
use serde_json::{Value, json};

/// Editor text above this many characters is cut to the part around the cursor.
pub const EDITOR_LIMIT: usize = 50_000;

/// Marks around the selected part of the editor text.
pub const SELECTION_START: &str = "<<<SELECTION>>>";
pub const SELECTION_END: &str = "<<<END SELECTION>>>";

/// Version of the Anthropic API the request is written for.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Longest answer asked for (Anthropic needs a limit; thinking counts in it).
const MAX_TOKENS: u32 = 16_000;

// ---- presets ------------------------------------------------------------------------

/// A provider preset: fills in the fields of the AI settings; each stays editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub protocol: AiProtocol,
    pub base_url: &'static str,
    pub model: &'static str,
}

pub const PRESETS: [Preset; 5] = [
    Preset {
        name: "Claude",
        protocol: AiProtocol::Anthropic,
        base_url: "https://api.anthropic.com",
        model: "claude-sonnet-5-5",
    },
    Preset {
        name: "OpenAI",
        protocol: AiProtocol::OpenaiCompatible,
        base_url: "https://api.openai.com/v1",
        model: "gpt-5",
    },
    Preset {
        name: "Grok",
        protocol: AiProtocol::OpenaiCompatible,
        base_url: "https://api.x.ai/v1",
        model: "grok-4",
    },
    Preset {
        name: "Gemini",
        protocol: AiProtocol::OpenaiCompatible,
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        model: "gemini-2.5-pro",
    },
    Preset {
        name: "Ollama (local)",
        protocol: AiProtocol::OpenaiCompatible,
        base_url: "http://localhost:11434/v1",
        model: "qwen2.5-coder",
    },
];

/// The preset whose protocol and base URL `ai` has (the model may differ), as an
/// index into [`PRESETS`].
pub fn preset_of(ai: &AiSettings) -> Option<usize> {
    let url = ai.base_url.trim().trim_end_matches('/');
    PRESETS
        .iter()
        .position(|p| p.protocol == ai.protocol && p.base_url == url)
}

/// Enabled, with a base URL and a model (the key is checked when it is needed).
pub fn configured(ai: &AiSettings) -> bool {
    ai.enabled && !ai.base_url.trim().is_empty() && !ai.model.trim().is_empty()
}

/// The API key from the key file; `None` when there is none (local servers need no
/// key; a service that needs one answers with its own error).
pub fn api_key(key_file: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(key_file)
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
}

/// Host the request goes to, for the dialog (`api.anthropic.com`).
pub fn host(base_url: &str) -> String {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    rest.split(['/', '?']).next().unwrap_or(rest).to_owned()
}

// ---- the structure --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelKind {
    Table,
    PartitionedTable,
    View,
    MaterializedView,
    ForeignTable,
}

impl RelKind {
    fn from_code(c: &str) -> Option<Self> {
        Some(match c {
            "r" => RelKind::Table,
            "p" => RelKind::PartitionedTable,
            "v" => RelKind::View,
            "m" => RelKind::MaterializedView,
            "f" => RelKind::ForeignTable,
            _ => return None,
        })
    }

    fn keyword(self) -> &'static str {
        match self {
            RelKind::Table => "TABLE",
            RelKind::PartitionedTable => "TABLE",
            RelKind::View => "VIEW",
            RelKind::MaterializedView => "MATERIALIZED VIEW",
            RelKind::ForeignTable => "FOREIGN TABLE",
        }
    }

    fn is_view(self) -> bool {
        matches!(self, RelKind::View | RelKind::MaterializedView)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub ty: String,
    pub not_null: bool,
    /// `DEFAULT …`, `GENERATED … AS IDENTITY` or `GENERATED ALWAYS AS (…) STORED`, as
    /// the query wrote it; empty without one.
    pub extra: String,
    /// `COMMENT ON COLUMN` (one line); empty without one.
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub kind: RelKind,
    /// `COMMENT ON` (one line); empty without one.
    pub comment: String,
    /// By attnum.
    pub columns: BTreeMap<i32, Column>,
    /// (contype, definition), primary key first, then unique, foreign keys, checks;
    /// by name inside each kind.
    pub constraints: Vec<(String, String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routine {
    pub schema: String,
    pub name: String,
    pub procedure: bool,
    pub args: String,
    pub result: String,
    pub comment: String,
}

/// The database structure read by `sql/ai/default/structure.sql`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Structure {
    /// By (schema, name).
    pub relations: BTreeMap<(String, String), Relation>,
    pub routines: Vec<Routine>,
    /// (schema, name) → labels as SQL literals.
    pub enums: BTreeMap<(String, String), String>,
}

impl Structure {
    pub fn from_rows(rows: Vec<Vec<String>>) -> Self {
        let mut s = Structure::default();
        let mut columns = Vec::new();
        let mut constraints = Vec::new();
        for row in rows {
            let r = |i: usize| row.get(i).cloned().unwrap_or_default();
            match row.first().map(String::as_str) {
                Some("r") => {
                    if let Some(kind) = RelKind::from_code(&r(3)) {
                        s.relations.insert(
                            (r(1), r(2)),
                            Relation {
                                kind,
                                comment: r(8),
                                columns: BTreeMap::new(),
                                constraints: Vec::new(),
                            },
                        );
                    }
                }
                Some("c") => columns.push(row),
                Some("k") => constraints.push(row),
                Some("f") => s.routines.push(Routine {
                    schema: r(1),
                    name: r(2),
                    procedure: r(3) == "p",
                    args: r(4),
                    result: r(5),
                    comment: r(8),
                }),
                Some("e") => {
                    s.enums.insert((r(1), r(2)), r(3));
                }
                _ => {}
            }
        }
        for row in columns {
            let r = |i: usize| row.get(i).cloned().unwrap_or_default();
            if let Some(rel) = s.relations.get_mut(&(r(1), r(2))) {
                rel.columns.insert(
                    r(3).parse().unwrap_or(0),
                    Column {
                        name: r(4),
                        ty: r(5),
                        not_null: r(6) == "true",
                        extra: r(7),
                        comment: r(8),
                    },
                );
            }
        }
        for row in constraints {
            let r = |i: usize| row.get(i).cloned().unwrap_or_default();
            if let Some(rel) = s.relations.get_mut(&(r(1), r(2))) {
                rel.constraints.push((r(4), r(3), r(5)));
            }
        }
        let order = |t: &str| "pufc".find(t).unwrap_or(4);
        for rel in s.relations.values_mut() {
            rel.constraints
                .sort_by(|a, b| order(&a.0).cmp(&order(&b.0)).then(a.1.cmp(&b.1)));
        }
        s.routines
            .sort_by(|a, b| (&a.schema, &a.name, &a.args).cmp(&(&b.schema, &b.name, &b.args)));
        s
    }

    /// Every schema with something in it, sorted.
    pub fn schemas(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .relations
            .keys()
            .map(|k| k.0.clone())
            .chain(self.routines.iter().map(|r| r.schema.clone()))
            .chain(self.enums.keys().map(|k| k.0.clone()))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The structure as compact DDL-like text; only `schema` when given.
    pub fn text(&self, schema: Option<&str>) -> String {
        let wanted = |s: &str| schema.is_none_or(|w| w == s);
        let mut out = String::new();
        for ((s, name), labels) in &self.enums {
            if wanted(s) {
                out.push_str(&format!(
                    "TYPE {} AS ENUM ({labels});\n",
                    quote_qualified(&[s, name])
                ));
            }
        }
        for ((s, name), rel) in &self.relations {
            if !wanted(s) {
                continue;
            }
            let qualified = quote_qualified(&[s, name]);
            if !rel.comment.is_empty() {
                out.push_str(&format!("-- {}\n", rel.comment));
            }
            if rel.kind.is_view() {
                let cols: Vec<String> = rel
                    .columns
                    .values()
                    .map(|c| format!("{} {}", quote_ident(&c.name), c.ty))
                    .collect();
                out.push_str(&format!(
                    "{} {qualified} ({});\n",
                    rel.kind.keyword(),
                    cols.join(", ")
                ));
                continue;
            }
            // (line, comment)
            let mut lines: Vec<(String, &str)> = rel
                .columns
                .values()
                .map(|c| {
                    let mut l = format!("{} {}", quote_ident(&c.name), c.ty);
                    if c.not_null {
                        l.push_str(" NOT NULL");
                    }
                    if !c.extra.is_empty() {
                        l.push(' ');
                        l.push_str(&c.extra);
                    }
                    (l, c.comment.as_str())
                })
                .collect();
            lines.extend(rel.constraints.iter().map(|(_, _, def)| (def.clone(), "")));
            let partitioned = if rel.kind == RelKind::PartitionedTable {
                " -- partitioned"
            } else {
                ""
            };
            out.push_str(&format!(
                "{} {qualified} ({partitioned}\n",
                rel.kind.keyword()
            ));
            let last = lines.len().saturating_sub(1);
            for (i, (l, comment)) in lines.iter().enumerate() {
                out.push_str("  ");
                out.push_str(l);
                if i < last {
                    out.push(',');
                }
                if !comment.is_empty() {
                    out.push_str(" -- ");
                    out.push_str(comment);
                }
                out.push('\n');
            }
            out.push_str(");\n");
        }
        for r in self.routines.iter().filter(|r| wanted(&r.schema)) {
            let qualified = quote_qualified(&[&r.schema, &r.name]);
            if r.procedure {
                out.push_str(&format!("PROCEDURE {qualified}({});", r.args));
            } else {
                out.push_str(&format!(
                    "FUNCTION {qualified}({}) RETURNS {};",
                    r.args, r.result
                ));
            }
            if !r.comment.is_empty() {
                out.push_str(" -- ");
                out.push_str(&r.comment);
            }
            out.push('\n');
        }
        out
    }
}

// ---- autocomplete in the prompt ----------------------------------------------------------

/// The name being typed before char `cursor` of a prompt: where its last part starts
/// (the inserted text replaces from there), the parts before it (`sales.` →
/// `["sales"]`) and the last part, without quotes.
pub fn prompt_word(chars: &[char], cursor: usize) -> (usize, Vec<String>, String) {
    let cursor = cursor.min(chars.len());
    let in_name = |c: char| c.is_alphanumeric() || matches!(c, '_' | '$' | '"' | '.');
    let mut from = cursor;
    while from > 0 && in_name(chars[from - 1]) {
        from -= 1;
    }
    let word: String = chars[from..cursor].iter().collect();
    let mut parts: Vec<String> = word.split('.').map(|p| p.replace('"', "")).collect();
    let last = word.rsplit('.').next().unwrap_or("");
    let start = cursor - last.chars().count();
    let prefix = parts.pop().unwrap_or_default();
    (start, parts, prefix)
}

fn relation_kind(kind: RelKind) -> &'static str {
    match kind {
        RelKind::Table | RelKind::PartitionedTable => "table",
        RelKind::View => "view",
        RelKind::MaterializedView => "mat. view",
        RelKind::ForeignTable => "foreign table",
    }
}

/// Order of the groups in the list.
fn prompt_rank(kind: &str) -> u8 {
    match kind {
        "column" => 0,
        "table" | "view" | "mat. view" | "foreign table" => 1,
        "function" | "procedure" => 2,
        "schema" => 3,
        _ => 4,
    }
}

/// The names for the prompt's autocomplete (D-073), from the structure read for the
/// dialog. Without a qualifier: schemas, and every object schema-qualified (found by
/// its own name too); after `schema.`: the objects of that schema; after `table.` (a
/// name only one schema has) or `schema.table.`: the columns.
pub fn prompt_items(
    s: &Structure,
    qualifier: &[String],
    prefix: &str,
) -> Vec<crate::completion::Item> {
    use crate::completion::Item;
    let prefix = prefix.to_lowercase();
    let fits = |name: &str| name.to_lowercase().starts_with(&prefix);
    let mut out: Vec<Item> = Vec::new();
    let mut add = |label: String, kind: &'static str, insert: String| {
        out.push(Item {
            label,
            kind,
            insert,
        })
    };
    // every object: (schema, name, kind)
    let mut objects: Vec<(&str, &str, &'static str)> = s
        .relations
        .iter()
        .map(|((sc, n), r)| (sc.as_str(), n.as_str(), relation_kind(r.kind)))
        .collect();
    objects.extend(s.routines.iter().map(|r| {
        let kind = if r.procedure { "procedure" } else { "function" };
        (r.schema.as_str(), r.name.as_str(), kind)
    }));
    objects.extend(
        s.enums
            .keys()
            .map(|(sc, n)| (sc.as_str(), n.as_str(), "type")),
    );
    let columns = |rel: &Relation, add: &mut dyn FnMut(String, &'static str, String)| {
        for c in rel.columns.values().filter(|c| fits(&c.name)) {
            add(c.name.clone(), "column", quote_ident(&c.name));
        }
    };
    match qualifier {
        [] => {
            for schema in s.schemas().iter().filter(|n| fits(n)) {
                add(schema.clone(), "schema", quote_ident(schema));
            }
            for (schema, name, kind) in &objects {
                let label = format!("{schema}.{name}");
                if fits(name) || fits(&label) {
                    add(label, kind, quote_qualified(&[schema, name]));
                }
            }
        }
        [q] => {
            for (schema, name, kind) in &objects {
                if schema.eq_ignore_ascii_case(q) && fits(name) {
                    add((*name).to_owned(), kind, quote_ident(name));
                }
            }
            let mut tables = s
                .relations
                .iter()
                .filter(|((_, n), _)| n.eq_ignore_ascii_case(q));
            if let (Some((_, rel)), None) = (tables.next(), tables.next()) {
                columns(rel, &mut add);
            }
        }
        [schema, table] => {
            if let Some((_, rel)) = s.relations.iter().find(|((sc, n), _)| {
                sc.eq_ignore_ascii_case(schema) && n.eq_ignore_ascii_case(table)
            }) {
                columns(rel, &mut add);
            }
        }
        _ => {}
    }
    out.sort_by(|a, b| {
        (prompt_rank(a.kind), a.label.to_lowercase())
            .cmp(&(prompt_rank(b.kind), b.label.to_lowercase()))
    });
    // overloaded functions once
    out.dedup_by(|a, b| a.label == b.label && a.kind == b.kind);
    out
}

// ---- the editor text ------------------------------------------------------------------

/// The editor text as it is sent: the selection marked, cut to [`EDITOR_LIMIT`]
/// characters around the cursor; `cut` says the text was cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorText {
    pub text: String,
    pub cut: bool,
}

/// `text` with its selection (char indices) marked; a text longer than `limit` chars
/// is cut to whole lines around `cursor` (and the selection, when it fits).
pub fn editor_text(
    text: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
    limit: usize,
) -> Option<EditorText> {
    if text.trim().is_empty() {
        return None;
    }
    // byte offset of char index `ci`
    let byte = |ci: usize| text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b);
    let len = text.chars().count();
    let cut = len > limit;
    let (mut from, mut to) = (0, text.len());
    if cut {
        // the window: centred on the selection (or the cursor), whole lines only
        let (a, b) = selection.unwrap_or((cursor, cursor));
        let middle = if b - a <= limit { (a + b) / 2 } else { cursor };
        let start = middle.saturating_sub(limit / 2).min(len - limit);
        from = byte(start);
        to = byte(start + limit);
        if from > 0
            && let Some(nl) = text[from..to].find('\n')
        {
            from += nl + 1;
        }
        if to < text.len()
            && let Some(nl) = text[from..to].rfind('\n')
        {
            to = from + nl + 1;
        }
    }
    let marks = selection
        .filter(|&(a, b)| a < b)
        .map(|(a, b)| (byte(a), byte(b)))
        .filter(|&(a, b)| a < to && b > from)
        .map(|(a, b)| (a.max(from), b.min(to)));
    let out = match marks {
        Some((a, b)) => format!(
            "{}{SELECTION_START}{}{SELECTION_END}{}",
            &text[from..a],
            &text[a..b],
            &text[b..to]
        ),
        None => text[from..to].to_owned(),
    };
    Some(EditorText { text: out, cut })
}

// ---- the request ------------------------------------------------------------------------

/// What goes into one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// `PostgreSQL 16.2` and the database name.
    pub server: String,
    pub database: String,
    pub structure: String,
    pub editor: Option<EditorText>,
    pub prompt: String,
}

/// The fixed instructions and the structure.
pub fn system_prompt(ask: &Ask) -> String {
    format!(
        "You write SQL for {server}. The user works in pgtc, a terminal PostgreSQL client. \
Your answer goes into the user's SQL editor unchanged; the user reviews it and runs it.\n\
Rules:\n\
- Answer with PostgreSQL SQL only, valid for {server}. No Markdown, no code fences.\n\
- Put any explanation into SQL comments (-- ...).\n\
- Use the objects of the structure below; create new ones only when asked.\n\
- In the editor text, the part between {SELECTION_START} and {SELECTION_END} is \
selected: your answer replaces it.\n\n\
Structure of database \"{database}\" (no data):\n{structure}",
        server = ask.server,
        database = ask.database,
        structure = ask.structure,
    )
}

/// The editor text and the user's request.
pub fn user_prompt(ask: &Ask) -> String {
    let mut out = String::new();
    if let Some(e) = &ask.editor {
        if e.cut {
            out.push_str("SQL editor text (only the part around the cursor):\n");
        } else {
            out.push_str("SQL editor text:\n");
        }
        out.push_str(&e.text);
        if !e.text.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str("Request:\n");
    out.push_str(&ask.prompt);
    out
}

/// About how many tokens `chars` characters are (four characters a token).
pub fn tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

/// Characters of everything a request for `ask` sends.
pub fn request_chars(ask: &Ask) -> usize {
    system_prompt(ask).chars().count() + user_prompt(ask).chars().count()
}

/// Added to an answer that stopped at the model's length limit.
pub const CUT_OFF: &str = "-- (the answer was cut off here: it reached the model's length limit)";

/// An HTTP request, ready for [`send`] / [`fetch_models`].
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub protocol: AiProtocol,
    pub url: String,
    /// Headers besides `content-type`; the key is in here.
    pub headers: Vec<(String, String)>,
    /// `Null`: a GET request.
    pub body: Value,
}

/// The headers with the key (and the API version) of `protocol`.
fn auth_headers(protocol: AiProtocol, key: Option<&str>) -> Vec<(String, String)> {
    match protocol {
        AiProtocol::OpenaiCompatible => key
            .map(|k| vec![("authorization".to_owned(), format!("Bearer {k}"))])
            .unwrap_or_default(),
        AiProtocol::Anthropic => {
            let mut headers = vec![("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned())];
            if let Some(k) = key {
                headers.push(("x-api-key".to_owned(), k.to_owned()));
            }
            headers
        }
    }
}

/// The request for the provider's list of models.
pub fn models_request(ai: &AiSettings, key: Option<&str>) -> Request {
    let base = ai.base_url.trim().trim_end_matches('/');
    Request {
        protocol: ai.protocol,
        url: match ai.protocol {
            AiProtocol::OpenaiCompatible => format!("{base}/models"),
            AiProtocol::Anthropic => format!("{base}/v1/models?limit=1000"),
        },
        headers: auth_headers(ai.protocol, key),
        body: Value::Null,
    }
}

/// The models of a model list response (`{"data": [{"id": …}]}` in both protocols;
/// Anthropic adds `display_name` and `created_at`, OpenAI-compatible services
/// `created`), newest first — by name where no date is given; or the provider's
/// error.
pub fn models(status: u16, body: &str) -> Result<Vec<AiModel>, String> {
    let json: Option<Value> = serde_json::from_str(body).ok();
    if !(200..300).contains(&status) {
        let detail = json
            .as_ref()
            .and_then(|j| j.pointer("/error/message").and_then(Value::as_str))
            .map_or_else(|| body.chars().take(300).collect(), str::to_owned);
        return Err(format!("The AI service answered HTTP {status}: {detail}"));
    }
    // (when it was made — a text that sorts by time —, the model)
    let mut list: Vec<(String, AiModel)> = json
        .as_ref()
        .and_then(|j| j.get("data"))
        .and_then(Value::as_array)
        .map(|d| {
            d.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(Value::as_str)?;
                    let made = match (m.get("created_at"), m.get("created")) {
                        (Some(Value::String(t)), _) => t.clone(),
                        (_, Some(Value::Number(n))) => format!("{:020}", n.as_u64().unwrap_or(0)),
                        _ => String::new(),
                    };
                    let name = m
                        .get("display_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    Some((
                        made,
                        AiModel {
                            id: id.to_owned(),
                            name,
                        },
                    ))
                })
                .collect()
        })
        .ok_or("The AI service sent no list of models.")?;
    list.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    let mut out: Vec<AiModel> = list.into_iter().map(|(_, m)| m).collect();
    out.dedup_by(|a, b| a.id == b.id);
    if out.is_empty() {
        return Err("The AI service lists no models.".to_owned());
    }
    Ok(out)
}

/// The request of `ai`'s protocol for `ask`, with `ai`'s model and effort.
pub fn request(ai: &AiSettings, key: Option<&str>, ask: &Ask) -> Request {
    let base = ai.base_url.trim().trim_end_matches('/');
    let system = system_prompt(ask);
    let user = user_prompt(ask);
    let model = ai.model.trim();
    let headers = auth_headers(ai.protocol, key);
    match ai.protocol {
        AiProtocol::OpenaiCompatible => {
            let mut body = json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
                // the answer comes as a stream: its progress is shown (D-073)
                "stream": true,
                "stream_options": {"include_usage": true},
            });
            // (low / medium / high here: the two top levels are sent as high)
            let effort = match ai.effort {
                AiEffort::Default => None,
                AiEffort::Low => Some("low"),
                AiEffort::Medium => Some("medium"),
                AiEffort::High | AiEffort::Xhigh | AiEffort::Max => Some("high"),
            };
            if let Some(e) = effort {
                body["reasoning_effort"] = json!(e);
            }
            Request {
                protocol: ai.protocol,
                url: format!("{base}/chat/completions"),
                headers,
                body,
            }
        }
        AiProtocol::Anthropic => {
            // the instructions and the structure are the same for every request of
            // a session with the same scope: marked for the prompt cache, so
            // further questions cost less and start sooner
            let mut body = json!({
                "model": model,
                "max_tokens": MAX_TOKENS,
                "system": [{
                    "type": "text",
                    "text": system,
                    "cache_control": {"type": "ephemeral"},
                }],
                "messages": [{"role": "user", "content": user}],
                // the answer comes as a stream, with summaries of the thinking: the
                // progress is shown (D-073)
                "stream": true,
                "thinking": {"type": "adaptive", "display": "summarized"},
            });
            let effort = match ai.effort {
                AiEffort::Default => None,
                AiEffort::Low => Some("low"),
                AiEffort::Medium => Some("medium"),
                AiEffort::High => Some("high"),
                AiEffort::Xhigh => Some("xhigh"),
                AiEffort::Max => Some("max"),
            };
            if let Some(e) = effort {
                body["output_config"] = json!({"effort": e});
            }
            Request {
                protocol: ai.protocol,
                url: format!("{base}/v1/messages"),
                headers,
                body,
            }
        }
    }
}

/// The answer text of a successful response, or the provider's error.
pub fn answer(protocol: AiProtocol, status: u16, body: &str) -> Result<String, String> {
    let json: Option<Value> = serde_json::from_str(body).ok();
    let error = |j: &Value| {
        j.pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| j.get("error").and_then(Value::as_str))
            .or_else(|| j.get("message").and_then(Value::as_str))
            .map(str::to_owned)
    };
    if !(200..300).contains(&status) {
        let detail = json
            .as_ref()
            .and_then(error)
            .unwrap_or_else(|| body.chars().take(300).collect());
        return Err(format!("The AI service answered HTTP {status}: {detail}"));
    }
    let Some(json) = json else {
        return Err("The AI service sent an answer that is not JSON.".to_owned());
    };
    let text = match protocol {
        AiProtocol::OpenaiCompatible => json
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_owned),
        AiProtocol::Anthropic => json.get("content").and_then(Value::as_array).map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        }),
    };
    // stopped at the length limit: said in the text, so it is not run as it is
    let cut = match protocol {
        AiProtocol::OpenaiCompatible => {
            json.pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                == Some("length")
        }
        AiProtocol::Anthropic => {
            json.get("stop_reason").and_then(Value::as_str) == Some("max_tokens")
        }
    };
    match text {
        Some(t) if !t.trim().is_empty() && cut => Ok(format!("{}\n{CUT_OFF}\n", t.trim_end())),
        Some(t) if !t.trim().is_empty() => Ok(t),
        _ => Err(error(&json).unwrap_or_else(|| "The AI service sent no answer.".to_owned())),
    }
}

/// The answer as editor text: code fence lines removed, text outside the fences
/// turned into `--` comments (when there are fences), the prompt above it as
/// comments, a line end at the end.
pub fn editor_insert(prompt: &str, answer: &str) -> String {
    let fenced = answer.lines().any(|l| l.trim_start().starts_with("```"));
    let mut body = Vec::new();
    let mut inside = false;
    for line in answer.lines() {
        if line.trim_start().starts_with("```") {
            inside = !inside;
            continue;
        }
        if fenced && !inside {
            if line.trim().is_empty() {
                body.push(String::new());
            } else if line.trim_start().starts_with("--") {
                body.push(line.to_owned());
            } else {
                body.push(format!("-- {line}"));
            }
        } else {
            body.push(line.to_owned());
        }
    }
    // no empty lines at the start and the end
    while body.first().is_some_and(|l| l.trim().is_empty()) {
        body.remove(0);
    }
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    let mut out = String::new();
    for line in prompt.trim().lines() {
        out.push_str("-- ");
        out.push_str(line.trim_end());
        out.push('\n');
    }
    for line in body {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

// ---- the streamed answer ----------------------------------------------------------------

/// A step of a request, for the tab's Messages (D-073).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// The model started to think.
    Thinking,
    /// A part of the model's summary of its thinking.
    ThinkingText(String),
    /// The model started to write the answer.
    Writing,
    /// So many characters of the answer are there (said every [`WRITTEN_STEP`]).
    Written(usize),
    /// Tokens sent (of them from the prompt cache) and received.
    Usage {
        sent: u64,
        cached: u64,
        received: u64,
    },
}

/// The answer's progress is reported each time it has grown by this many characters.
const WRITTEN_STEP: usize = 1500;

/// Reads the server-sent events of a streamed answer as they arrive.
#[derive(Debug)]
pub struct Stream {
    protocol: AiProtocol,
    /// Bytes of an event that is not complete yet.
    pending: Vec<u8>,
    /// The start of the body, for an answer that is no event stream after all.
    raw: Vec<u8>,
    events: usize,
    text: String,
    /// Thinking text not yet reported (a paragraph in the making).
    thinking: String,
    said_thinking: bool,
    said_writing: bool,
    /// Characters of the answer at the last [`Progress::Written`].
    written: usize,
    cut: bool,
    error: Option<String>,
    sent: u64,
    cached: u64,
    received: u64,
}

impl Stream {
    pub fn new(protocol: AiProtocol) -> Self {
        Self {
            protocol,
            pending: Vec::new(),
            raw: Vec::new(),
            events: 0,
            text: String::new(),
            thinking: String::new(),
            said_thinking: false,
            said_writing: false,
            written: 0,
            cut: false,
            error: None,
            sent: 0,
            cached: 0,
            received: 0,
        }
    }

    /// The next bytes of the body; the steps they complete.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Progress> {
        if self.events == 0 && self.raw.len() < 1 << 20 {
            self.raw.extend_from_slice(bytes);
        }
        // (a carriage return is never part of the data: JSON escapes it)
        self.pending
            .extend(bytes.iter().copied().filter(|b| *b != b'\r'));
        let mut out = Vec::new();
        while let Some(end) = self.pending.windows(2).position(|w| w == b"\n\n") {
            let event: Vec<u8> = self.pending.drain(..end + 2).collect();
            let event = String::from_utf8_lossy(&event);
            let data: Vec<&str> = event
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if data.is_empty() {
                continue;
            }
            self.events += 1;
            let data = data.join("\n");
            if let Ok(json) = serde_json::from_str::<Value>(&data) {
                match self.protocol {
                    AiProtocol::Anthropic => self.anthropic(&json, &mut out),
                    AiProtocol::OpenaiCompatible => self.openai(&json, &mut out),
                }
            }
        }
        out
    }

    fn start_thinking(&mut self, out: &mut Vec<Progress>) {
        if !self.said_thinking {
            self.said_thinking = true;
            out.push(Progress::Thinking);
        }
    }

    /// More thinking text: its complete paragraphs are reported.
    fn think(&mut self, text: &str, out: &mut Vec<Progress>) {
        self.start_thinking(out);
        self.thinking.push_str(text);
        while let Some(end) = self.thinking.find("\n\n") {
            let paragraph: String = self.thinking.drain(..end + 2).collect();
            if !paragraph.trim().is_empty() {
                out.push(Progress::ThinkingText(paragraph.trim().to_owned()));
            }
        }
    }

    /// The thinking text left over is reported.
    fn end_thinking(&mut self, out: &mut Vec<Progress>) {
        let rest = std::mem::take(&mut self.thinking);
        if !rest.trim().is_empty() {
            out.push(Progress::ThinkingText(rest.trim().to_owned()));
        }
    }

    fn write(&mut self, text: &str, out: &mut Vec<Progress>) {
        if text.is_empty() {
            return;
        }
        if !self.said_writing {
            self.said_writing = true;
            self.end_thinking(out);
            out.push(Progress::Writing);
        }
        self.text.push_str(text);
        if self.text.len() - self.written >= WRITTEN_STEP {
            self.written = self.text.len();
            out.push(Progress::Written(self.text.chars().count()));
        }
    }

    fn anthropic(&mut self, j: &Value, out: &mut Vec<Progress>) {
        let str_at = |p: &str| j.pointer(p).and_then(Value::as_str);
        let num_at = |p: &str| j.pointer(p).and_then(Value::as_u64).unwrap_or(0);
        match str_at("/type") {
            Some("message_start") => {
                self.cached = num_at("/message/usage/cache_read_input_tokens");
                self.sent = num_at("/message/usage/input_tokens")
                    + self.cached
                    + num_at("/message/usage/cache_creation_input_tokens");
            }
            Some("content_block_start") => match str_at("/content_block/type") {
                Some("thinking") => self.start_thinking(out),
                Some("text") => {
                    let first = str_at("/content_block/text").unwrap_or("").to_owned();
                    self.write(&first, out);
                }
                _ => {}
            },
            Some("content_block_delta") => match str_at("/delta/type") {
                Some("thinking_delta") => {
                    let t = str_at("/delta/thinking").unwrap_or("").to_owned();
                    self.think(&t, out);
                }
                Some("text_delta") => {
                    let t = str_at("/delta/text").unwrap_or("").to_owned();
                    self.write(&t, out);
                }
                _ => {}
            },
            Some("content_block_stop") => self.end_thinking(out),
            Some("message_delta") => {
                self.received = num_at("/usage/output_tokens").max(self.received);
                match str_at("/delta/stop_reason") {
                    Some("max_tokens") => self.cut = true,
                    Some("refusal") if self.text.trim().is_empty() => {
                        self.error = Some("The model declined this request.".to_owned());
                    }
                    _ => {}
                }
            }
            Some("error") => {
                self.error = Some(format!(
                    "The AI service reported an error: {}",
                    str_at("/error/message").unwrap_or("(no text)")
                ));
            }
            _ => {}
        }
    }

    fn openai(&mut self, j: &Value, out: &mut Vec<Progress>) {
        let str_at = |p: &str| j.pointer(p).and_then(Value::as_str);
        if let Some(e) = str_at("/error/message") {
            self.error = Some(format!("The AI service reported an error: {e}"));
            return;
        }
        // (some services send the model's reasoning beside the answer)
        if let Some(t) = str_at("/choices/0/delta/reasoning_content")
            .or_else(|| str_at("/choices/0/delta/reasoning"))
            .filter(|t| !t.is_empty())
        {
            let t = t.to_owned();
            self.think(&t, out);
        }
        if let Some(t) = str_at("/choices/0/delta/content") {
            let t = t.to_owned();
            self.write(&t, out);
        }
        if str_at("/choices/0/finish_reason") == Some("length") {
            self.cut = true;
        }
        if let Some(u) = j.get("usage").filter(|u| u.is_object()) {
            let n = |p: &str| u.pointer(p).and_then(Value::as_u64).unwrap_or(0);
            self.sent = n("/prompt_tokens");
            self.cached = n("/prompt_tokens_details/cached_tokens");
            self.received = n("/completion_tokens");
        }
    }

    /// The body has ended: the last steps, and the answer text or an error.
    pub fn finish(mut self) -> (Vec<Progress>, Result<String, String>) {
        // no event stream after all (a service that ignores `stream`): one JSON answer
        if self.events == 0 {
            let body = String::from_utf8_lossy(&self.raw).into_owned();
            return (Vec::new(), answer(self.protocol, 200, &body));
        }
        let mut out = Vec::new();
        self.end_thinking(&mut out);
        if self.sent > 0 || self.received > 0 {
            out.push(Progress::Usage {
                sent: self.sent,
                cached: self.cached,
                received: self.received,
            });
        }
        let result = match self.error {
            Some(e) => Err(e),
            None if self.text.trim().is_empty() => Err("The AI service sent no answer.".to_owned()),
            None if self.cut => Ok(format!("{}\n{CUT_OFF}\n", self.text.trim_end())),
            None => Ok(self.text),
        };
        (out, result)
    }
}

// ---- HTTP -------------------------------------------------------------------------------

/// Longest time for a whole answer (models may need minutes), and for the wait
/// between two parts of it.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn client() -> Result<reqwest::Client, String> {
    use std::sync::Arc;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .timeout(TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// Sends `req` (a GET when it has no body); the response, its body not read yet, or
/// an error text for the user.
async fn open(req: &Request) -> Result<reqwest::Response, String> {
    let client = client()?;
    let mut builder = if req.body.is_null() {
        client.get(&req.url)
    } else {
        client.post(&req.url).json(&req.body)
    };
    for (name, value) in &req.headers {
        let mut value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| "The API key has characters that cannot be sent.".to_owned())?;
        // the key: never shown by any logging of the request
        value.set_sensitive(true);
        builder = builder.header(name.as_str(), value);
    }
    builder.send().await.map_err(|e| {
        if e.is_timeout() {
            "The AI service did not answer in time.".to_owned()
        } else if e.is_connect() {
            format!("Could not connect to {}: {e}", host(&req.url))
        } else {
            format!("The AI request failed: {e}")
        }
    })
}

async fn body(response: reqwest::Response) -> Result<String, String> {
    response
        .text()
        .await
        .map_err(|e| format!("Reading the AI answer failed: {e}"))
}

/// Sends `req` and reads the streamed answer, reporting each step to `progress`; the
/// answer text or an error text for the user. A model that does not take the
/// `thinking` setting is asked again without it.
pub async fn send(mut req: Request, progress: impl Fn(Progress)) -> Result<String, String> {
    let mut response = open(&req).await?;
    let mut status = response.status().as_u16();
    if status == 400 && req.body.get("thinking").is_some() {
        let text = body(response).await?;
        if !text.contains("thinking") {
            return answer(req.protocol, status, &text);
        }
        if let Some(o) = req.body.as_object_mut() {
            o.remove("thinking");
        }
        response = open(&req).await?;
        status = response.status().as_u16();
    }
    if !(200..300).contains(&status) {
        let text = body(response).await?;
        return answer(req.protocol, status, &text);
    }
    let mut stream = Stream::new(req.protocol);
    loop {
        match response.chunk().await {
            Ok(Some(bytes)) => stream.feed(&bytes).into_iter().for_each(&progress),
            Ok(None) => break,
            Err(e) if e.is_timeout() => {
                return Err("The AI service stopped answering.".to_owned());
            }
            Err(e) => return Err(format!("Reading the AI answer failed: {e}")),
        }
    }
    let (last, result) = stream.finish();
    last.into_iter().for_each(&progress);
    result
}

/// Sends a [`models_request`]; the models or an error text for the user.
pub async fn fetch_models(req: Request) -> Result<Vec<AiModel>, String> {
    let response = open(&req).await?;
    let status = response.status().as_u16();
    models(status, &body(response).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cols: &[&str]) -> Vec<String> {
        cols.iter().map(|c| (*c).to_owned()).collect()
    }

    fn sample() -> Structure {
        Structure::from_rows(vec![
            row(&["r", "sales", "customer", "r", "", "", "", "", "Who buys"]),
            row(&["r", "sales", "v_customer", "v", "", "", "", ""]),
            row(&["r", "hr", "Employee", "p", "", "", "", ""]),
            row(&[
                "c",
                "sales",
                "customer",
                "2",
                "name",
                "text",
                "true",
                "",
                "Full name",
            ]),
            row(&[
                "c",
                "sales",
                "customer",
                "1",
                "customer_id",
                "integer",
                "true",
                "DEFAULT nextval('sales.customer_customer_id_seq'::regclass)",
            ]),
            row(&[
                "c",
                "sales",
                "customer",
                "3",
                "territory_id",
                "integer",
                "false",
                "",
            ]),
            row(&[
                "k",
                "sales",
                "customer",
                "fk_territory",
                "f",
                "FOREIGN KEY (territory_id) REFERENCES sales.territory(territory_id)",
                "",
                "",
            ]),
            row(&[
                "k",
                "sales",
                "customer",
                "pk",
                "p",
                "PRIMARY KEY (customer_id)",
                "",
                "",
            ]),
            row(&["c", "sales", "v_customer", "1", "name", "text", "false", ""]),
            row(&[
                "c",
                "hr",
                "Employee",
                "1",
                "id",
                "bigint",
                "true",
                "GENERATED ALWAYS AS IDENTITY",
            ]),
            row(&["f", "sales", "add", "p", "IN p_name text", "", "", ""]),
            row(&[
                "f",
                "sales",
                "total",
                "f",
                "p_id integer",
                "numeric",
                "",
                "",
                "Sum of orders",
            ]),
            row(&["e", "hr", "mood", "'happy', 'sad'", "", "", "", ""]),
        ])
    }

    #[test]
    fn structure_text() {
        let s = sample();
        assert_eq!(s.schemas(), ["hr", "sales"]);
        let want = [
            "TYPE hr.mood AS ENUM ('happy', 'sad');",
            "TABLE hr.\"Employee\" ( -- partitioned",
            "  id bigint NOT NULL GENERATED ALWAYS AS IDENTITY",
            ");",
            "-- Who buys",
            "TABLE sales.customer (",
            "  customer_id integer NOT NULL DEFAULT nextval('sales.customer_customer_id_seq'::regclass),",
            "  name text NOT NULL, -- Full name",
            "  territory_id integer,",
            "  PRIMARY KEY (customer_id),",
            "  FOREIGN KEY (territory_id) REFERENCES sales.territory(territory_id)",
            ");",
            "VIEW sales.v_customer (name text);",
            "PROCEDURE sales.add(IN p_name text);",
            "FUNCTION sales.total(p_id integer) RETURNS numeric; -- Sum of orders",
        ];
        assert_eq!(s.text(None), want.join("\n") + "\n");
        assert_eq!(s.text(Some("hr")).lines().count(), 4);
        assert!(s.text(Some("nothing")).is_empty());
    }

    #[test]
    fn editor_text_marks_and_cuts() {
        assert_eq!(editor_text("  \n", 0, None, 100), None);
        let e = editor_text("select 1;\nselect 2;\n", 0, Some((10, 18)), 100).unwrap();
        assert_eq!(
            e.text,
            "select 1;\n<<<SELECTION>>>select 2<<<END SELECTION>>>;\n"
        );
        assert!(!e.cut);
        // selection to the end of the text
        let e = editor_text("abc", 3, Some((1, 3)), 100).unwrap();
        assert_eq!(e.text, "a<<<SELECTION>>>bc<<<END SELECTION>>>");

        // a long text: whole lines around the cursor
        let text: String = (0..100).map(|i| format!("line {i:02}\n")).collect();
        let cursor = text.find("line 50").unwrap();
        let e = editor_text(&text, cursor, None, 40).unwrap();
        assert!(e.cut);
        assert!(e.text.len() <= 40);
        assert!(e.text.starts_with("line "));
        assert!(e.text.ends_with('\n'));
        assert!(e.text.contains("line 50") || e.text.contains("line 49"));
    }

    fn ask() -> Ask {
        Ask {
            server: "PostgreSQL 16.2".into(),
            database: "shop".into(),
            structure: "TABLE public.t (\n  id integer\n);\n".into(),
            editor: Some(EditorText {
                text: "select 1".into(),
                cut: false,
            }),
            prompt: "count the rows of t".into(),
        }
    }

    #[test]
    fn requests_of_both_protocols() {
        let mut ai = AiSettings {
            enabled: true,
            protocol: AiProtocol::Anthropic,
            base_url: "https://api.anthropic.com/".into(),
            model: "claude-sonnet-5-5".into(),
            effort: Default::default(),
            models: Vec::new(),
        };
        let r = request(&ai, Some("k1"), &ask());
        assert_eq!(r.url, "https://api.anthropic.com/v1/messages");
        assert!(r.headers.contains(&("x-api-key".into(), "k1".into())));
        assert!(r.headers.iter().any(|h| h.0 == "anthropic-version"));
        assert_eq!(r.body["model"], "claude-sonnet-5-5");
        assert_eq!(r.body["max_tokens"], 16000);
        assert!(
            r.body["system"][0]["text"]
                .as_str()
                .unwrap()
                .contains("TABLE public.t")
        );
        assert_eq!(r.body["system"][0]["cache_control"]["type"], "ephemeral");
        let user = r.body["messages"][0]["content"].as_str().unwrap();
        assert_eq!(
            user,
            "SQL editor text:\nselect 1\n\nRequest:\ncount the rows of t"
        );

        ai.protocol = AiProtocol::OpenaiCompatible;
        ai.base_url = "http://localhost:11434/v1".into();
        let r = request(&ai, None, &ask());
        assert_eq!(r.url, "http://localhost:11434/v1/chat/completions");
        assert!(r.headers.is_empty());
        assert_eq!(r.body["messages"][0]["role"], "system");
        assert_eq!(r.body["messages"][1]["role"], "user");
        let r = request(&ai, Some("k2"), &ask());
        assert_eq!(r.headers, [("authorization".into(), "Bearer k2".into())]);
    }

    #[test]
    fn effort_and_model_lists() {
        let mut ai = AiSettings {
            enabled: true,
            protocol: AiProtocol::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-opus-5-5".into(),
            effort: AiEffort::Default,
            models: Vec::new(),
        };
        // the default: nothing is sent
        let r = request(&ai, None, &ask());
        assert!(r.body.get("output_config").is_none());
        assert_eq!(r.body["max_tokens"], 16000);
        ai.effort = AiEffort::Xhigh;
        let r = request(&ai, None, &ask());
        assert_eq!(r.body["output_config"]["effort"], "xhigh");
        assert_eq!(r.body["model"], "claude-opus-5-5");
        let m = models_request(&ai, Some("k"));
        assert_eq!(m.url, "https://api.anthropic.com/v1/models?limit=1000");
        assert!(m.body.is_null());
        assert!(m.headers.contains(&("x-api-key".into(), "k".into())));

        ai.protocol = AiProtocol::OpenaiCompatible;
        ai.base_url = "http://localhost:11434/v1/".into();
        let r = request(&ai, None, &ask());
        assert_eq!(
            r.body["reasoning_effort"], "high",
            "the top levels are high"
        );
        ai.effort = AiEffort::Low;
        assert_eq!(request(&ai, None, &ask()).body["reasoning_effort"], "low");
        ai.effort = AiEffort::Default;
        assert!(
            request(&ai, None, &ask())
                .body
                .get("reasoning_effort")
                .is_none()
        );
        assert_eq!(
            models_request(&ai, None).url,
            "http://localhost:11434/v1/models"
        );

        // Anthropic: names for people, newest first
        let list = r#"{"data":[
            {"id":"claude-opus-4-8","display_name":"Claude Opus 4.8","created_at":"2026-03-01T00:00:00Z"},
            {"id":"claude-sonnet-5-5","display_name":"Claude Sonnet 5.5","created_at":"2026-09-01T00:00:00Z"},
            {"id":"claude-haiku-5-5","display_name":"Claude Haiku 5.5","created_at":"2026-09-01T00:00:00Z"}
        ],"has_more":false}"#;
        let got = models(200, list).unwrap();
        let labels: Vec<&str> = got.iter().map(AiModel::label).collect();
        assert_eq!(
            labels,
            ["Claude Haiku 5.5", "Claude Sonnet 5.5", "Claude Opus 4.8"]
        );
        assert_eq!(got[0].id, "claude-haiku-5-5");
        // OpenAI-compatible: ids only, `created` as a number; or no date at all
        let list = r#"{"data":[{"id":"old","created":100},{"id":"new","created":2000}]}"#;
        let got = models(200, list).unwrap();
        assert_eq!((got[0].label(), got[1].label()), ("new", "old"));
        let list = r#"{"data":[{"id":"b"},{"id":"a"}]}"#;
        assert_eq!(models(200, list).unwrap()[0].id, "a");
        assert!(models(200, r#"{"data":[]}"#).is_err());
        assert!(models(200, "nope").is_err());
        assert_eq!(
            models(401, r#"{"error":{"message":"invalid x-api-key"}}"#),
            Err("The AI service answered HTTP 401: invalid x-api-key".into())
        );
    }

    #[test]
    fn streamed_answers() {
        // Anthropic: thinking summaries by paragraph, then the text; usage at the end
        let events = [
            r#"event: message_start
data: {"type":"message_start","message":{"model":"claude-sonnet-5-5","usage":{"input_tokens":20,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0}}}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"The user wants totals.\n\nJoin customer"}}"#,
            r#"event: ping
data: {"type":"ping"}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" to the orders."}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"SELECT 1"}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":";"}}"#,
            r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
            r#"event: message_stop
data: {"type":"message_stop"}"#,
        ];
        let body = events.join("\r\n\r\n") + "\n\n";
        // fed in small pieces: events and characters may be split anywhere
        let mut st = Stream::new(AiProtocol::Anthropic);
        let mut steps = Vec::new();
        for piece in body.as_bytes().chunks(7) {
            steps.extend(st.feed(piece));
        }
        let (last, result) = st.finish();
        steps.extend(last);
        assert_eq!(result, Ok("SELECT 1;".into()));
        assert_eq!(
            steps,
            [
                Progress::Thinking,
                Progress::ThinkingText("The user wants totals.".into()),
                Progress::ThinkingText("Join customer to the orders.".into()),
                Progress::Writing,
                Progress::Usage {
                    sent: 1020,
                    cached: 1000,
                    received: 7
                },
            ]
        );

        // cut off at the length limit; an error in the stream
        let mut st = Stream::new(AiProtocol::Anthropic);
        st.feed(
            b"data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"SELECT\"}}\n\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":3}}\n\n",
        );
        assert_eq!(st.finish().1, Ok(format!("SELECT\n{CUT_OFF}\n")));
        let mut st = Stream::new(AiProtocol::Anthropic);
        st.feed(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n");
        assert_eq!(
            st.finish().1,
            Err("The AI service reported an error: Overloaded".into())
        );

        // OpenAI-compatible: content pieces, reasoning where a service sends it, usage
        let mut st = Stream::new(AiProtocol::OpenaiCompatible);
        let mut steps = st.feed(
            b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"Count rows.\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"SELECT count(*)\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\" FROM t;\"},\"finish_reason\":\"stop\"}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":50,\"completion_tokens\":9}}\n\n\
data: [DONE]\n\n",
        );
        let (last, result) = st.finish();
        steps.extend(last);
        assert_eq!(result, Ok("SELECT count(*) FROM t;".into()));
        assert_eq!(
            steps,
            [
                Progress::Thinking,
                Progress::ThinkingText("Count rows.".into()),
                Progress::Writing,
                Progress::Usage {
                    sent: 50,
                    cached: 0,
                    received: 9
                },
            ]
        );

        // a service that ignores `stream`: one JSON answer
        let mut st = Stream::new(AiProtocol::OpenaiCompatible);
        st.feed(br#"{"choices":[{"message":{"content":"SELECT 2;"}}]}"#);
        assert_eq!(st.finish().1, Ok("SELECT 2;".into()));
        // nothing at all
        let st = Stream::new(AiProtocol::Anthropic);
        assert!(st.finish().1.is_err());
    }

    #[test]
    fn answers_and_errors() {
        let open = r#"{"choices":[{"message":{"role":"assistant","content":"SELECT 1;"}}]}"#;
        assert_eq!(
            answer(AiProtocol::OpenaiCompatible, 200, open),
            Ok("SELECT 1;".into())
        );
        let claude =
            r#"{"content":[{"type":"text","text":"SELECT "},{"type":"text","text":"2;"}]}"#;
        assert_eq!(
            answer(AiProtocol::Anthropic, 200, claude),
            Ok("SELECT 2;".into())
        );
        let err = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(
            answer(AiProtocol::Anthropic, 401, err),
            Err("The AI service answered HTTP 401: invalid x-api-key".into())
        );
        assert_eq!(
            answer(AiProtocol::OpenaiCompatible, 502, "Bad gateway"),
            Err("The AI service answered HTTP 502: Bad gateway".into())
        );
        assert!(answer(AiProtocol::Anthropic, 200, r#"{"content":[]}"#).is_err());
        // stopped at the length limit: a comment says so
        let cut =
            r#"{"content":[{"type":"text","text":"SELECT 1,\n"}],"stop_reason":"max_tokens"}"#;
        assert_eq!(
            answer(AiProtocol::Anthropic, 200, cut),
            Ok(format!("SELECT 1,\n{CUT_OFF}\n"))
        );
        let cut = r#"{"choices":[{"message":{"content":"SELECT"},"finish_reason":"length"}]}"#;
        assert!(
            answer(AiProtocol::OpenaiCompatible, 200, cut)
                .unwrap()
                .ends_with(&format!("{CUT_OFF}\n"))
        );
        let whole = r#"{"choices":[{"message":{"content":"SELECT 1;"},"finish_reason":"stop"}]}"#;
        assert_eq!(
            answer(AiProtocol::OpenaiCompatible, 200, whole),
            Ok("SELECT 1;".into())
        );
        assert!(answer(AiProtocol::Anthropic, 200, "nope").is_err());
    }

    #[test]
    fn answer_into_the_editor() {
        assert_eq!(
            editor_insert("count the rows\nof t", "SELECT count(*) FROM t;\n"),
            "-- count the rows\n-- of t\nSELECT count(*) FROM t;\n"
        );
        assert_eq!(
            editor_insert(
                "q",
                "Here it is:\n\n```sql\nSELECT 1;\n```\n-- done\nHope it helps."
            ),
            "-- q\n-- Here it is:\n\nSELECT 1;\n-- done\n-- Hope it helps.\n"
        );
    }

    #[test]
    fn prompt_words() {
        let chars: Vec<char> = "rows of sales.cust".chars().collect();
        assert_eq!(
            prompt_word(&chars, chars.len()),
            (14, vec!["sales".to_owned()], "cust".to_owned())
        );
        let chars: Vec<char> = "the \"Order Items\".".chars().collect();
        // a space inside quotes ends the word (quotes are rare in prompts)
        assert_eq!(prompt_word(&chars, chars.len()).2, "");
        let chars: Vec<char> = "sum of customer.".chars().collect();
        assert_eq!(
            prompt_word(&chars, chars.len()),
            (16, vec!["customer".to_owned()], String::new())
        );
        assert_eq!(prompt_word(&[], 0), (0, vec![], String::new()));
        let chars: Vec<char> = "a b".chars().collect();
        assert_eq!(prompt_word(&chars, 3), (2, vec![], "b".to_owned()));
    }

    #[test]
    fn prompt_names() {
        let s = sample();
        let labels = |q: &[&str], p: &str| -> Vec<(String, &'static str, String)> {
            let q: Vec<String> = q.iter().map(|x| (*x).to_owned()).collect();
            prompt_items(&s, &q, p)
                .into_iter()
                .map(|i| (i.label, i.kind, i.insert))
                .collect()
        };
        // by its own name, inserted schema-qualified
        assert_eq!(
            labels(&[], "cu"),
            [(
                "sales.customer".to_owned(),
                "table",
                "sales.customer".to_owned()
            )]
        );
        // a schema and what starts with its name
        let all = labels(&[], "s");
        assert!(all.contains(&("sales".into(), "schema", "sales".into())));
        assert!(all.contains(&("sales.v_customer".into(), "view", "sales.v_customer".into())));
        // quoted when needed
        assert_eq!(
            labels(&[], "emp"),
            [(
                "hr.Employee".to_owned(),
                "table",
                "hr.\"Employee\"".to_owned()
            )]
        );
        // after a schema: its objects, the name only
        let sales: Vec<String> = labels(&["sales"], "").into_iter().map(|x| x.0).collect();
        assert_eq!(sales, ["customer", "v_customer", "add", "total"]);
        // after a table: its columns; also with the schema
        let cols: Vec<String> = labels(&["customer"], "").into_iter().map(|x| x.0).collect();
        assert_eq!(cols, ["customer_id", "name", "territory_id"]);
        assert_eq!(labels(&["SALES", "customer"], "ter")[0].1, "column");
        assert!(labels(&["nothing"], "").is_empty());
    }

    #[test]
    fn hosts_and_presets() {
        assert_eq!(host("https://api.anthropic.com"), "api.anthropic.com");
        assert_eq!(host("http://localhost:11434/v1"), "localhost:11434");
        assert!(PRESETS.iter().all(|p| p.base_url.starts_with("http")));
        assert_eq!(tokens(10), 3);
        let mut ai = AiSettings::default();
        assert!(!configured(&ai));
        assert_eq!(preset_of(&ai), None);
        ai.protocol = AiProtocol::Anthropic;
        ai.base_url = "https://api.anthropic.com/".into();
        assert_eq!(PRESETS[preset_of(&ai).unwrap()].name, "Claude");
        assert_eq!(
            request_chars(&ask()),
            system_prompt(&ask()).len() + user_prompt(&ask()).len()
        );
        ai.enabled = true;
        ai.base_url = "x".into();
        ai.model = "m".into();
        assert!(configured(&ai));
        assert_eq!(api_key(std::path::Path::new("no/such/file")), None);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ai_key");
        std::fs::write(&file, "  \n").unwrap();
        assert_eq!(api_key(&file), None);
        std::fs::write(&file, "sk-1\n").unwrap();
        assert_eq!(api_key(&file), Some("sk-1".into()));
    }
}
