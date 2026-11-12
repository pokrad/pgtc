//! PostgreSQL SQL lexer (D-015) for highlighting. Works line by line; the state at the
//! end of a line (inside a block comment, string, quoted identifier, dollar quote or a
//! function / `DO` body) is carried to the next line.
//!
//! A dollar quote right after `AS` or `DO` (`AS $$`, `DO $body$`) is a **body** and
//! its text is lexed as SQL (keywords, strings, comments); its closing tag always ends
//! it, also inside an unclosed string or comment of the body, as in PostgreSQL. Every
//! other dollar quote is a string.

use std::sync::OnceLock;

/// Lexer state at the start of a line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LexState {
    #[default]
    Normal,
    /// Inside `/* */`; PostgreSQL block comments nest.
    BlockComment(u32),
    /// Inside `'...'`; `escape` for `E'...'` strings (backslash escapes).
    String { escape: bool },
    /// Inside `"..."`.
    QuotedIdent,
    /// Inside a dollar-quoted string `$tag$ ... $tag$` (tag may be empty).
    Dollar(String),
    /// Normal text whose last word was `AS` or `DO`: a dollar quote next is a body.
    AfterAs,
    /// Inside function / `DO` bodies: their tags (outermost first) and the state in
    /// the innermost one.
    Body {
        tags: Vec<String>,
        inner: Box<LexState>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Text,
    Keyword,
    String,
    Number,
    Comment,
    QuotedIdent,
    Operator,
    /// The `$tag$` that opens or closes a body (drawn like a string; ends a statement
    /// for autocomplete).
    BodyQuote,
}

/// A token as a byte range of the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// Lexes one line (without its line break) starting in `state`. Tokens are pushed to
/// `out` if given (the background scan only needs the end state).
pub fn lex_line(state: &LexState, line: &str, mut out: Option<&mut Vec<Token>>) -> LexState {
    let len = line.len();
    let (mut tags, mut inner) = match state {
        LexState::Body { tags, inner } => (tags.clone(), (**inner).clone()),
        s => (Vec::new(), s.clone()),
    };
    let mut i = 0;
    loop {
        // The first closing tag of any open body ends it and the bodies inside it (an
        // outer body is a string to PostgreSQL: it ends at the first `$tag$` of its own).
        let close = tags
            .iter()
            .enumerate()
            .filter_map(|(k, t)| {
                let c = format!("${t}$");
                line[i..].find(&c).map(|p| (i + p, k, c.len()))
            })
            .min();
        let limit = close.map_or(len, |c| c.0);
        let (state, at, opened) = lex_range(&inner, line, i, limit, &mut out);
        inner = state;
        i = at;
        if let Some(tag) = opened {
            // `AS $tag$`: a body starts (`i` is after its tag)
            tags.push(tag);
            inner = LexState::Normal;
            continue;
        }
        if let Some((at, k, n)) = close {
            push(&mut out, at, at + n, TokenKind::BodyQuote);
            i = at + n;
            tags.truncate(k);
            // a body is opened from normal text
            inner = LexState::Normal;
            continue;
        }
        break;
    }
    if tags.is_empty() {
        inner
    } else {
        LexState::Body {
            tags,
            inner: Box::new(inner),
        }
    }
}

fn push(out: &mut Option<&mut Vec<Token>>, start: usize, end: usize, kind: TokenKind) {
    if let Some(out) = out.as_deref_mut()
        && end > start
    {
        out.push(Token { start, end, kind });
    }
}

/// Lexes `line[from..end]` starting in `state` (never a [`LexState::Body`]). Stops at
/// `end`, or right after the `$tag$` of a body that opens (then returns its tag).
/// Returns the state, the position reached and the opened body's tag.
fn lex_range(
    state: &LexState,
    line: &str,
    from: usize,
    end: usize,
    out: &mut Option<&mut Vec<Token>>,
) -> (LexState, usize, Option<String>) {
    let bytes = &line.as_bytes()[..end];
    let len = end;
    // The background scan only needs the end state: no keyword lookups.
    let want_tokens = out.is_some();
    let mut i = from;
    // Start of an opening quote / `/*` / `$tag$`, so that it becomes part of the token
    // produced by the branch for the construct it opens.
    let mut opened: Option<usize> = None;
    let mut after_as = matches!(state, LexState::AfterAs);
    let mut state = match state {
        LexState::AfterAs => LexState::Normal,
        s => s.clone(),
    };

    while i < len {
        match &mut state {
            LexState::BlockComment(depth) => {
                let start = opened.take().unwrap_or(i);
                while i < len {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        *depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        *depth -= 1;
                        i += 2;
                        if *depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                let i2 = i.min(len);
                push(out, start, i2, TokenKind::Comment);
                i = i2;
                if matches!(state, LexState::BlockComment(0)) {
                    state = LexState::Normal;
                }
            }
            LexState::String { escape } => {
                let escape = *escape;
                let start = opened.take().unwrap_or(i);
                let mut closed = false;
                while i < len {
                    if escape && bytes[i] == b'\\' {
                        i += 2;
                    } else if bytes[i] == b'\'' {
                        if bytes.get(i + 1) == Some(&b'\'') {
                            i += 2;
                        } else {
                            i += 1;
                            closed = true;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                let i2 = i.min(len);
                push(out, start, i2, TokenKind::String);
                i = i2;
                if closed {
                    state = LexState::Normal;
                }
            }
            LexState::QuotedIdent => {
                let start = opened.take().unwrap_or(i);
                let mut closed = false;
                while i < len {
                    if bytes[i] == b'"' {
                        if bytes.get(i + 1) == Some(&b'"') {
                            i += 2;
                        } else {
                            i += 1;
                            closed = true;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                let i2 = i.min(len);
                push(out, start, i2, TokenKind::QuotedIdent);
                i = i2;
                if closed {
                    state = LexState::Normal;
                }
            }
            LexState::Dollar(tag) => {
                let close = format!("${tag}$");
                let start = opened.take().unwrap_or(i);
                match line[i..len].find(&close) {
                    Some(pos) => {
                        i += pos + close.len();
                        push(out, start, i, TokenKind::String);
                        state = LexState::Normal;
                    }
                    None => {
                        i = len;
                        push(out, start, i, TokenKind::String);
                    }
                }
            }
            LexState::Normal | LexState::AfterAs | LexState::Body { .. } => {
                let start = i;
                let b = bytes[i];
                // whitespace and comments keep `after_as`; everything else sets it
                let mut word_as = false;
                if b.is_ascii_whitespace() {
                    while i < len && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    push(out, start, i, TokenKind::Text);
                    continue;
                } else if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
                    push(out, start, len, TokenKind::Comment);
                    i = len;
                    continue;
                } else if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    state = LexState::BlockComment(1);
                    i += 2;
                    opened = Some(start);
                    continue;
                } else if b == b'\'' {
                    state = LexState::String { escape: false };
                    i += 1;
                    opened = Some(start);
                } else if b == b'"' {
                    state = LexState::QuotedIdent;
                    i += 1;
                    opened = Some(start);
                } else if b == b'$' {
                    if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
                        // parameter $1
                        i += 1;
                        while i < len && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                        push(out, start, i, TokenKind::Operator);
                    } else if let Some(tag_len) = dollar_tag(&bytes[i + 1..]) {
                        let tag = line[i + 1..i + 1 + tag_len].to_owned();
                        i += tag_len + 2;
                        if after_as {
                            push(out, start, i, TokenKind::BodyQuote);
                            return (LexState::Normal, i, Some(tag));
                        }
                        opened = Some(start);
                        state = LexState::Dollar(tag);
                    } else {
                        i += 1;
                        push(out, start, i, TokenKind::Operator);
                    }
                } else if b.is_ascii_digit()
                    || (b == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
                {
                    i += 1;
                    while i < len
                        && (bytes[i].is_ascii_alphanumeric()
                            || bytes[i] == b'.'
                            || bytes[i] == b'_')
                    {
                        // exponent sign: 1e-5
                        if (bytes[i] == b'e' || bytes[i] == b'E')
                            && matches!(bytes.get(i + 1), Some(b'+' | b'-'))
                        {
                            i += 1;
                        }
                        i += 1;
                    }
                    push(out, start, i.min(len), TokenKind::Number);
                    i = i.min(len);
                } else if b == b'_' || b.is_ascii_alphabetic() || b >= 0x80 {
                    while i < len
                        && (bytes[i] == b'_'
                            || bytes[i] == b'$'
                            || bytes[i].is_ascii_alphanumeric()
                            || bytes[i] >= 0x80)
                    {
                        i += 1;
                    }
                    let word = &line[start..i];
                    // E'...' / e'...' strings use backslash escapes; B'', X'', U&'' are
                    // ordinary strings for highlighting.
                    if bytes.get(i) == Some(&b'\'') && word.eq_ignore_ascii_case("e") {
                        i += 1;
                        opened = Some(start);
                        state = LexState::String { escape: true };
                    } else {
                        word_as =
                            word.eq_ignore_ascii_case("as") || word.eq_ignore_ascii_case("do");
                        let kind = if want_tokens && is_keyword(word) {
                            TokenKind::Keyword
                        } else {
                            TokenKind::Text
                        };
                        push(out, start, i, kind);
                    }
                } else {
                    // operators and punctuation; keep UTF-8 boundaries
                    i += 1;
                    while i < len && !line.is_char_boundary(i) {
                        i += 1;
                    }
                    push(out, start, i, TokenKind::Operator);
                }
                after_as = word_as;
            }
        }
    }
    if let Some(start) = opened {
        let kind = match state {
            LexState::BlockComment(_) => TokenKind::Comment,
            LexState::QuotedIdent => TokenKind::QuotedIdent,
            _ => TokenKind::String,
        };
        push(out, start, len, kind);
    }
    if end < line.len() {
        // a body's closing tag follows: what was open in the body ends here
        return (LexState::Normal, end, None);
    }
    if after_as && state == LexState::Normal {
        state = LexState::AfterAs;
    }
    (state, end, None)
}

/// Length of the tag if `rest` (after the first `$`) starts with `tag$`.
fn dollar_tag(rest: &[u8]) -> Option<usize> {
    let mut n = 0;
    while n < rest.len() {
        let c = rest[n];
        if c == b'$' {
            return Some(n);
        }
        let ok = c == b'_' || c.is_ascii_alphabetic() || c >= 0x80 || (n > 0 && c.is_ascii_digit());
        if !ok {
            return None;
        }
        n += 1;
    }
    None
}

/// SQL and PL/pgSQL keywords highlighted in the editor.
const KEYWORDS: &[&str] = &[
    "abort",
    "absolute",
    "access",
    "action",
    "add",
    "admin",
    "after",
    "aggregate",
    "alias",
    "all",
    "also",
    "alter",
    "always",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "assert",
    "assertion",
    "assignment",
    "asymmetric",
    "at",
    "attach",
    "attribute",
    "authorization",
    "backward",
    "before",
    "begin",
    "between",
    "bigint",
    "bigserial",
    "binary",
    "bit",
    "boolean",
    "both",
    "breadth",
    "by",
    "bytea",
    "cache",
    "call",
    "called",
    "cascade",
    "cascaded",
    "case",
    "cast",
    "catalog",
    "chain",
    "char",
    "character",
    "characteristics",
    "check",
    "checkpoint",
    "class",
    "close",
    "cluster",
    "coalesce",
    "collate",
    "collation",
    "column",
    "columns",
    "comment",
    "comments",
    "commit",
    "committed",
    "concurrently",
    "configuration",
    "conflict",
    "connection",
    "constant",
    "constraint",
    "constraints",
    "content",
    "continue",
    "conversion",
    "copy",
    "cost",
    "create",
    "cross",
    "csv",
    "cube",
    "current",
    "current_catalog",
    "current_date",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "cursor",
    "cycle",
    "data",
    "database",
    "date",
    "day",
    "deallocate",
    "debug",
    "dec",
    "decimal",
    "declare",
    "default",
    "defaults",
    "deferrable",
    "deferred",
    "definer",
    "delete",
    "delimiter",
    "delimiters",
    "depends",
    "depth",
    "desc",
    "detach",
    "diagnostics",
    "dictionary",
    "disable",
    "discard",
    "distinct",
    "do",
    "document",
    "domain",
    "double",
    "drop",
    "each",
    "else",
    "elseif",
    "elsif",
    "enable",
    "encoding",
    "encrypted",
    "end",
    "enum",
    "escape",
    "event",
    "except",
    "exception",
    "exclude",
    "excluding",
    "exclusive",
    "execute",
    "exists",
    "exit",
    "explain",
    "expression",
    "extension",
    "external",
    "extract",
    "false",
    "family",
    "fetch",
    "filter",
    "first",
    "float",
    "following",
    "for",
    "force",
    "foreach",
    "foreign",
    "format",
    "forward",
    "found",
    "freeze",
    "from",
    "full",
    "function",
    "functions",
    "generated",
    "get",
    "global",
    "grant",
    "granted",
    "greatest",
    "group",
    "grouping",
    "groups",
    "handler",
    "having",
    "header",
    "hold",
    "hour",
    "identity",
    "if",
    "ilike",
    "immediate",
    "immutable",
    "implicit",
    "import",
    "in",
    "include",
    "including",
    "increment",
    "index",
    "indexes",
    "info",
    "inherit",
    "inherits",
    "initially",
    "inline",
    "inner",
    "inout",
    "input",
    "insensitive",
    "insert",
    "instead",
    "int",
    "integer",
    "intersect",
    "interval",
    "into",
    "invoker",
    "is",
    "isnull",
    "isolation",
    "join",
    "json",
    "jsonb",
    "key",
    "label",
    "language",
    "large",
    "last",
    "lateral",
    "leading",
    "leakproof",
    "least",
    "left",
    "level",
    "like",
    "limit",
    "listen",
    "load",
    "local",
    "localtime",
    "localtimestamp",
    "location",
    "lock",
    "locked",
    "log",
    "logged",
    "loop",
    "mapping",
    "match",
    "matched",
    "materialized",
    "maxvalue",
    "merge",
    "method",
    "minute",
    "minvalue",
    "mode",
    "month",
    "move",
    "name",
    "names",
    "national",
    "natural",
    "nchar",
    "new",
    "next",
    "nfc",
    "nfd",
    "nfkc",
    "nfkd",
    "no",
    "none",
    "normalize",
    "normalized",
    "not",
    "nothing",
    "notice",
    "notify",
    "notnull",
    "nowait",
    "null",
    "nullif",
    "nulls",
    "numeric",
    "object",
    "of",
    "off",
    "offset",
    "oids",
    "old",
    "on",
    "only",
    "open",
    "operator",
    "option",
    "options",
    "or",
    "order",
    "ordinality",
    "others",
    "out",
    "outer",
    "over",
    "overlaps",
    "overlay",
    "overriding",
    "owned",
    "owner",
    "parallel",
    "parameter",
    "parser",
    "partial",
    "partition",
    "passing",
    "password",
    "perform",
    "placing",
    "plans",
    "policy",
    "position",
    "preceding",
    "precision",
    "prepare",
    "prepared",
    "preserve",
    "primary",
    "prior",
    "privileges",
    "procedural",
    "procedure",
    "procedures",
    "program",
    "publication",
    "query",
    "quote",
    "raise",
    "range",
    "read",
    "real",
    "reassign",
    "record",
    "recursive",
    "ref",
    "references",
    "referencing",
    "refresh",
    "reindex",
    "relative",
    "release",
    "rename",
    "repeatable",
    "replace",
    "replica",
    "reset",
    "restart",
    "restrict",
    "return",
    "returning",
    "returns",
    "reverse",
    "revoke",
    "right",
    "role",
    "rollback",
    "rollup",
    "routine",
    "routines",
    "row",
    "rows",
    "rowtype",
    "rule",
    "savepoint",
    "schema",
    "schemas",
    "scroll",
    "search",
    "second",
    "security",
    "select",
    "sequence",
    "sequences",
    "serial",
    "serializable",
    "server",
    "session",
    "session_user",
    "set",
    "setof",
    "sets",
    "share",
    "show",
    "similar",
    "simple",
    "skip",
    "slice",
    "smallint",
    "smallserial",
    "snapshot",
    "some",
    "sql",
    "sqlerrm",
    "sqlstate",
    "stable",
    "stacked",
    "standalone",
    "start",
    "statement",
    "statistics",
    "stdin",
    "stdout",
    "storage",
    "stored",
    "strict",
    "strip",
    "subscription",
    "substring",
    "support",
    "symmetric",
    "sysid",
    "system",
    "system_user",
    "table",
    "tables",
    "tablesample",
    "tablespace",
    "temp",
    "template",
    "temporary",
    "text",
    "then",
    "ties",
    "time",
    "timestamp",
    "timestamptz",
    "to",
    "trailing",
    "transaction",
    "transform",
    "treat",
    "trigger",
    "trim",
    "true",
    "truncate",
    "trusted",
    "type",
    "types",
    "unbounded",
    "uncommitted",
    "unencrypted",
    "union",
    "unique",
    "unknown",
    "unlisten",
    "unlogged",
    "until",
    "update",
    "user",
    "using",
    "uuid",
    "vacuum",
    "valid",
    "validate",
    "validator",
    "value",
    "values",
    "varchar",
    "variadic",
    "varying",
    "verbose",
    "version",
    "view",
    "views",
    "volatile",
    "warning",
    "when",
    "where",
    "while",
    "whitespace",
    "window",
    "with",
    "within",
    "without",
    "work",
    "wrapper",
    "write",
    "xml",
    "year",
    "yes",
    "zone",
];

/// All keywords, sorted, in lower case (autocomplete, D-052).
pub fn keywords() -> &'static [&'static str] {
    static SORTED: OnceLock<Vec<&'static str>> = OnceLock::new();
    SORTED.get_or_init(|| {
        let mut v = KEYWORDS.to_vec();
        v.sort_unstable();
        v.dedup();
        v
    })
}

pub fn is_keyword(word: &str) -> bool {
    let sorted = keywords();
    if word.len() > 24 || !word.is_ascii() {
        return false;
    }
    // lower-case on the stack (no allocation per word)
    let mut buf = [0u8; 24];
    for (d, s) in buf.iter_mut().zip(word.bytes()) {
        *d = s.to_ascii_lowercase();
    }
    let lower = std::str::from_utf8(&buf[..word.len()]).unwrap_or("");
    sorted.binary_search(&lower).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(state: &LexState, line: &str) -> (Vec<(String, TokenKind)>, LexState) {
        let mut out = Vec::new();
        let end = lex_line(state, line, Some(&mut out));
        let toks = out
            .into_iter()
            .filter(|t| t.kind != TokenKind::Text || !line[t.start..t.end].trim().is_empty())
            .map(|t| (line[t.start..t.end].to_owned(), t.kind))
            .collect();
        (toks, end)
    }

    use TokenKind as K;

    #[test]
    fn basic_tokens() {
        let (t, end) = kinds(
            &LexState::Normal,
            "SELECT a, 'x''y', 3.5e-2, \"Q\"\"x\" FROM t -- c",
        );
        assert_eq!(end, LexState::Normal);
        assert_eq!(t[0], ("SELECT".into(), K::Keyword));
        assert_eq!(t[1], ("a".into(), K::Text));
        assert!(t.contains(&("'x''y'".into(), K::String)));
        assert!(t.contains(&("3.5e-2".into(), K::Number)));
        assert!(t.contains(&("\"Q\"\"x\"".into(), K::QuotedIdent)));
        assert!(t.contains(&("FROM".into(), K::Keyword)));
        assert_eq!(t.last().unwrap(), &("-- c".into(), K::Comment));
    }

    #[test]
    fn multi_line_states() {
        let (_, s) = kinds(&LexState::Normal, "x /* a /* nested */");
        assert_eq!(s, LexState::BlockComment(1));
        let (t, s) = kinds(&s, "still */ select");
        assert_eq!(s, LexState::Normal);
        assert_eq!(t[0], ("still */".into(), K::Comment));
        assert_eq!(t[1].1, K::Keyword);

        // a dollar quote that is not a body stays a string
        let (_, s) = kinds(&LexState::Normal, "select $fn$ text");
        assert_eq!(s, LexState::Dollar("fn".into()));
        let (t, s) = kinds(&s, "  more; $fn$; select");
        assert_eq!(s, LexState::Normal);
        assert_eq!(t[0].1, K::String);
        assert_eq!(t.last().unwrap().1, K::Keyword);

        let (_, s) = kinds(&LexState::Normal, "select $$ body");
        assert_eq!(s, LexState::Dollar(String::new()));

        let (_, s) = kinds(&LexState::Normal, "select E'a\\'b");
        assert_eq!(s, LexState::String { escape: true });
        // standard strings: a backslash is no escape, so 'a\' is closed
        let (t, s) = kinds(&LexState::Normal, "select 'a\\' x");
        assert_eq!(s, LexState::Normal);
        assert!(t.contains(&("'a\\'".into(), K::String)));
        // an opener at the end of a line is a token too
        let (t, s) = kinds(&LexState::Normal, "x /*");
        assert_eq!(s, LexState::BlockComment(1));
        assert_eq!(t.last().unwrap(), &("/*".into(), K::Comment));
    }

    fn body(tags: &[&str], inner: LexState) -> LexState {
        LexState::Body {
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            inner: Box::new(inner),
        }
    }

    /// `AS $tag$` / `DO $$` open a body lexed as SQL (D-015).
    #[test]
    fn function_and_do_bodies_are_code() {
        let (t, s) = kinds(&LexState::Normal, "CREATE FUNCTION f() RETURNS int AS $fn$");
        assert_eq!(s, body(&["fn"], LexState::Normal));
        assert_eq!(t.last().unwrap(), &("$fn$".into(), K::BodyQuote));
        let (t, s) = kinds(
            &s,
            "BEGIN RETURN (SELECT count(*) FROM t WHERE x = 'a;b'); END",
        );
        assert_eq!(s, body(&["fn"], LexState::Normal));
        assert_eq!(t[0], ("BEGIN".into(), K::Keyword));
        assert!(t.contains(&("SELECT".into(), K::Keyword)));
        assert!(t.contains(&("'a;b'".into(), K::String)));
        let (t, s) = kinds(&s, "$fn$ LANGUAGE plpgsql;");
        assert_eq!(s, LexState::Normal);
        assert_eq!(t[0], ("$fn$".into(), K::BodyQuote));
        assert!(t.contains(&("LANGUAGE".into(), K::Keyword)));

        // DO, and AS at the end of the previous line
        let (_, s) = kinds(&LexState::Normal, "do $$ begin raise notice 'x'; end $$;");
        assert_eq!(s, LexState::Normal);
        let (_, s) = kinds(&LexState::Normal, "CREATE FUNCTION f() RETURNS int AS");
        assert_eq!(s, LexState::AfterAs);
        let (_, s) = kinds(&s, "  -- the body:");
        assert_eq!(s, LexState::AfterAs, "a comment keeps it");
        let (_, s) = kinds(&s, "$$ select 1 $$");
        assert_eq!(s, LexState::Normal);
        // ON CONFLICT DO NOTHING: the next word ends it
        let (_, s) = kinds(&LexState::Normal, "on conflict do nothing");
        assert_eq!(s, LexState::Normal);
    }

    /// The closing tag ends a body also inside an unclosed string or comment; a nested
    /// dollar quote with another tag is inside it.
    #[test]
    fn body_closing_and_nesting() {
        let (t, s) = kinds(&LexState::Normal, "DO $$ x := 'unclosed $$; select");
        assert_eq!(s, LexState::Normal);
        assert!(t.contains(&("'unclosed ".into(), K::String)));
        assert!(t.contains(&("$$".into(), K::BodyQuote)));
        assert_eq!(t.last().unwrap(), &("select".into(), K::Keyword));
        let (_, s) = kinds(&LexState::Normal, "DO $$ /* comment");
        assert_eq!(s, body(&[""], LexState::BlockComment(1)));
        let (t, s) = kinds(&s, "still */ select $$");
        assert_eq!(s, LexState::Normal);
        assert_eq!(t[0], ("still */".into(), K::Comment));
        // a string dollar quote inside a body
        let (t, s) = kinds(
            &LexState::Normal,
            "DO $$ BEGIN EXECUTE $q$ select 1 $q$; END $$",
        );
        assert_eq!(s, LexState::Normal);
        assert!(t.contains(&("$q$ select 1 $q$".into(), K::String)));
        // a body inside a body (CREATE FUNCTION in a DO block)
        let (_, s) = kinds(
            &LexState::Normal,
            "DO $$ BEGIN CREATE FUNCTION g() RETURNS int AS $g$",
        );
        assert_eq!(s, body(&["", "g"], LexState::Normal));
        let (t, s) = kinds(&s, "SELECT 1 $g$ LANGUAGE sql; END");
        assert_eq!(s, body(&[""], LexState::Normal));
        assert!(t.contains(&("SELECT".into(), K::Keyword)));
        // the outer body's tag ends both (PostgreSQL reads the outer body as a string)
        let (_, s) = kinds(&s, "AS $g$ SELECT $$ select");
        assert_eq!(s, LexState::Normal);
    }

    #[test]
    fn parameters_and_odd_input() {
        let (t, _) = kinds(&LexState::Normal, "where id = $1 and x::int");
        assert!(t.contains(&("$1".into(), K::Operator)));
        // unicode identifiers and operators do not panic
        let (_, s) = kinds(&LexState::Normal, "select čevapi → ž, 'ž");
        assert_eq!(s, LexState::String { escape: false });
        assert!(is_keyword("Select") && !is_keyword("čevapi"));
    }
}
