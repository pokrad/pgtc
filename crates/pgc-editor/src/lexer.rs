//! PostgreSQL SQL lexer (D-015) for highlighting. Works line by line; the state at the
//! end of a line (inside a block comment, string, quoted identifier or dollar quote)
//! is carried to the next line.

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
    /// Inside `$tag$ ... $tag$` (tag may be empty).
    Dollar(String),
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
    let bytes = line.as_bytes();
    let len = bytes.len();
    // The background scan only needs the end state: no keyword lookups.
    let want_tokens = out.is_some();
    let mut i = 0;
    // Start of an opening quote / `/*` / `$tag$`, so that it becomes part of the token
    // produced by the branch for the construct it opens.
    let mut opened: Option<usize> = None;
    let mut state = state.clone();
    let mut push = |start: usize, end: usize, kind: TokenKind| {
        if let Some(out) = out.as_deref_mut()
            && end > start
        {
            out.push(Token { start, end, kind });
        }
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
                push(start, i, TokenKind::Comment);
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
                push(start, i2, TokenKind::String);
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
                push(start, i2, TokenKind::QuotedIdent);
                i = i2;
                if closed {
                    state = LexState::Normal;
                }
            }
            LexState::Dollar(tag) => {
                let close = format!("${tag}$");
                let start = opened.take().unwrap_or(i);
                match line[i..].find(&close) {
                    Some(pos) => {
                        i += pos + close.len();
                        push(start, i, TokenKind::String);
                        state = LexState::Normal;
                    }
                    None => {
                        i = len;
                        push(start, i, TokenKind::String);
                    }
                }
            }
            LexState::Normal => {
                let start = i;
                let b = bytes[i];
                if b.is_ascii_whitespace() {
                    while i < len && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    push(start, i, TokenKind::Text);
                } else if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
                    push(start, len, TokenKind::Comment);
                    i = len;
                } else if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    state = LexState::BlockComment(1);
                    i += 2;
                    opened = Some(start);
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
                        push(start, i, TokenKind::Operator);
                    } else if let Some(tag_len) = dollar_tag(&bytes[i + 1..]) {
                        let tag = line[i + 1..i + 1 + tag_len].to_owned();
                        i += tag_len + 2;
                        opened = Some(start);
                        state = LexState::Dollar(tag);
                    } else {
                        i += 1;
                        push(start, i, TokenKind::Operator);
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
                    push(start, i, TokenKind::Number);
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
                        let kind = if want_tokens && is_keyword(word) {
                            TokenKind::Keyword
                        } else {
                            TokenKind::Text
                        };
                        push(start, i, kind);
                    }
                } else {
                    // operators and punctuation; keep UTF-8 boundaries
                    i += 1;
                    while i < len && !line.is_char_boundary(i) {
                        i += 1;
                    }
                    push(start, i, TokenKind::Operator);
                }
            }
        }
    }
    if let Some(start) = opened {
        let kind = match state {
            LexState::BlockComment(_) => TokenKind::Comment,
            LexState::QuotedIdent => TokenKind::QuotedIdent,
            _ => TokenKind::String,
        };
        push(start, len, kind);
    }
    state
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
    "smallint",
    "smallserial",
    "snapshot",
    "some",
    "sql",
    "stable",
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

        let (_, s) = kinds(&LexState::Normal, "AS $fn$ BEGIN");
        assert_eq!(s, LexState::Dollar("fn".into()));
        let (t, s) = kinds(&s, "  RETURN 1; END $fn$; select");
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
