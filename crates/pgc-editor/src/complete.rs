//! Autocomplete context (D-052): what to offer at the cursor, from the lexer tokens of
//! the statement around it. A heuristic, not a parser.

use crate::editor::Editor;
use crate::lexer::{Token, TokenKind};

/// Lines before / after the cursor looked at for the statement.
const LINES_BEFORE: usize = 300;
const LINES_AFTER: usize = 100;

/// What kind of names fit at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// After `FROM` / `JOIN` / `UPDATE` / `INTO` / `TABLE`: schemas and relations.
    Relations,
    /// After `name.`: columns of the table or alias `name`, or the objects of the
    /// schema `name` (folded like PostgreSQL: unquoted names in lower case).
    Qualified(String),
    /// Anywhere else: columns of the statement's tables, relations, functions, keywords.
    Any,
}

/// A table named in the statement (`FROM s.t AS a`), names folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    /// The typed part of the name before the cursor (as typed).
    pub prefix: String,
    /// Char index where the prefix starts (the inserted text replaces from here).
    pub start: usize,
    pub want: Want,
    pub tables: Vec<TableRef>,
}

/// Keywords after which a list of tables follows.
const TABLE_KEYWORDS: &[&str] = &["from", "join", "update", "into", "table"];
/// Keywords that start another clause (end a `FROM` list).
const CLAUSE_KEYWORDS: &[&str] = &[
    "select",
    "from",
    "join",
    "where",
    "group",
    "order",
    "having",
    "on",
    "set",
    "update",
    "into",
    "values",
    "table",
    "using",
    "limit",
    "offset",
    "returning",
    "window",
    "union",
    "except",
    "intersect",
    "fetch",
    "for",
];
/// Words that cannot be a table alias.
const NOT_ALIAS: &[&str] = &[
    "where",
    "on",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "full",
    "cross",
    "natural",
    "group",
    "order",
    "having",
    "limit",
    "offset",
    "set",
    "using",
    "values",
    "union",
    "except",
    "intersect",
    "window",
    "returning",
    "select",
    "fetch",
    "for",
    "lateral",
    "default",
    "only",
    "tablesample",
    "with",
];

#[derive(Debug, Clone)]
struct Tok {
    line: usize,
    start: usize,
    end: usize,
    kind: TokenKind,
    text: String,
}

impl Tok {
    fn is(&self, op: &str) -> bool {
        self.kind == TokenKind::Operator && self.text == op
    }

    fn word(&self) -> Option<String> {
        match self.kind {
            TokenKind::Text | TokenKind::Keyword => Some(self.text.to_lowercase()),
            _ => None,
        }
    }

    fn keyword_in(&self, list: &[&str]) -> bool {
        self.word().is_some_and(|w| list.contains(&w.as_str()))
    }

    /// The name this token stands for, folded like PostgreSQL.
    fn name(&self) -> Option<String> {
        match self.kind {
            TokenKind::Text | TokenKind::Keyword => Some(self.text.to_lowercase()),
            TokenKind::QuotedIdent if self.text.len() >= 2 && self.text.ends_with('"') => {
                Some(self.text[1..self.text.len() - 1].replace("\"\"", "\""))
            }
            _ => None,
        }
    }
}

fn is_word_char(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

/// Analyses the editor text at its cursor; `None` where nothing should be offered
/// (inside a string, comment or quoted identifier).
pub fn analyze(editor: &Editor) -> Option<Analysis> {
    let cursor = editor.cursor();
    let line = editor.line_of_char(cursor);
    let first = line.saturating_sub(LINES_BEFORE);
    let lines = editor.highlighted_lines(first, line - first + 1 + LINES_AFTER);
    let line_start = editor.line_to_char(line);
    let text = &lines[line - first].0;
    let byte = text
        .char_indices()
        .nth(cursor - line_start)
        .map_or(text.len(), |(b, _)| b);
    let mut a = analyze_lines(&lines, line - first, byte)?;
    // the prefix start back to a char index of the editor
    a.start = cursor - a.prefix.chars().count();
    Some(a)
}

/// [`analyze`] on lexed lines: the cursor is at byte `byte` of line `line`. `start` of
/// the result is left as 0 (the caller knows the char index of the cursor).
pub fn analyze_lines(lines: &[(String, Vec<Token>)], line: usize, byte: usize) -> Option<Analysis> {
    let mut toks = Vec::new();
    for (n, (text, tokens)) in lines.iter().enumerate() {
        for t in tokens {
            let s = &text[t.start..t.end];
            if t.kind == TokenKind::Text && s.trim().is_empty() {
                continue;
            }
            toks.push(Tok {
                line: n,
                start: t.start,
                end: t.end,
                kind: t.kind,
                text: s.to_owned(),
            });
        }
    }
    let before = |t: &Tok| (t.line, t.end) <= (line, byte);
    // the token the cursor is in (or right after)
    let at = toks
        .iter()
        .position(|t| t.line == line && t.start < byte && byte <= t.end);
    let mut prefix = String::new();
    if let Some(i) = at {
        let t = &toks[i];
        match t.kind {
            TokenKind::String | TokenKind::Comment | TokenKind::QuotedIdent => {
                // inside, or right after an unterminated one
                if byte < t.end || !closed(t) {
                    return None;
                }
            }
            TokenKind::Text | TokenKind::Keyword => {
                let typed = &t.text[..byte - t.start];
                if typed.chars().all(is_word_char) {
                    prefix = typed.to_owned();
                }
            }
            _ => {}
        }
    }
    // tokens before the prefix, and the statement (between `;`) around the cursor
    let n_before = toks.iter().take_while(|t| before(t)).count();
    // the prefix's own token is among them only when the cursor is at its end (inside
    // a word the token ends after the cursor)
    let prefix_tokens = usize::from(!prefix.is_empty() && at.is_some_and(|i| i < n_before));
    let stmt_start = toks[..n_before]
        .iter()
        .rposition(|t| t.is(";"))
        .map_or(0, |i| i + 1);
    let stmt_end = toks[n_before..]
        .iter()
        .position(|t| t.is(";"))
        .map_or(toks.len(), |i| n_before + i);
    let stmt = &toks[stmt_start..stmt_end];
    let prev = &toks[stmt_start..n_before - prefix_tokens];

    let want = if prev.last().is_some_and(|t| t.is(".")) {
        let q = prev.len().checked_sub(2).and_then(|i| prev[i].name());
        match q {
            Some(q) => Want::Qualified(q),
            None => Want::Any,
        }
    } else {
        // the clause the cursor is in
        let clause = prev.iter().rev().find(|t| t.keyword_in(CLAUSE_KEYWORDS));
        let in_table_list = clause.is_some_and(|c| c.keyword_in(TABLE_KEYWORDS));
        let last = prev.last();
        let right_after = last.is_some_and(|t| t.keyword_in(TABLE_KEYWORDS) || t.is(","));
        if in_table_list && right_after {
            Want::Relations
        } else {
            Want::Any
        }
    };
    Some(Analysis {
        prefix,
        start: 0,
        want,
        tables: tables(stmt),
    })
}

fn closed(t: &Tok) -> bool {
    match t.kind {
        TokenKind::QuotedIdent => t.text.len() >= 2 && t.text.ends_with('"'),
        TokenKind::String => t.text.len() >= 2 && t.text.ends_with('\''),
        _ => false,
    }
}

/// Tables of a statement: after `FROM` / `JOIN` / `UPDATE` / `INTO` / `TABLE` and after
/// `,` in such a list; `name`, `schema.name`, optional `AS` and alias.
fn tables(stmt: &[Tok]) -> Vec<TableRef> {
    let mut out = Vec::new();
    let mut in_list = false;
    let mut i = 0;
    while i < stmt.len() {
        let t = &stmt[i];
        let starts = if t.keyword_in(TABLE_KEYWORDS) {
            in_list = true;
            true
        } else if t.keyword_in(CLAUSE_KEYWORDS) {
            in_list = false;
            false
        } else {
            in_list && t.is(",")
        };
        i += 1;
        if !starts {
            continue;
        }
        // name or schema.name (not a subquery / function call)
        let Some(first) = stmt.get(i).and_then(Tok::name) else {
            continue;
        };
        if stmt
            .get(i)
            .is_some_and(|t| t.keyword_in(&["select", "lateral", "only"]))
        {
            continue;
        }
        let dot = stmt.get(i + 1).is_some_and(|t| t.is("."));
        let (schema, name, next) = match stmt.get(i + 2).and_then(Tok::name) {
            Some(second) if dot => (Some(first), second, i + 3),
            // `schema.` still being typed: not a table (yet)
            _ if dot => continue,
            _ => (None, first, i + 1),
        };
        if stmt.get(next).is_some_and(|t| t.is("(")) {
            continue;
        }
        let mut j = next;
        if stmt.get(j).and_then(Tok::word).as_deref() == Some("as") {
            j += 1;
        }
        let alias = stmt.get(j).and_then(|t| {
            let a = t.name()?;
            (t.kind == TokenKind::QuotedIdent || !NOT_ALIAS.contains(&a.as_str())).then_some(a)
        });
        if alias.is_some() {
            j += 1;
        }
        out.push(TableRef {
            schema,
            name,
            alias,
        });
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexState, lex_line};

    /// Analysis of `sql` with the cursor at `|`.
    fn at(sql: &str) -> Option<Analysis> {
        let cursor = sql.find('|').expect("cursor mark");
        let text = sql.replacen('|', "", 1);
        let mut state = LexState::Normal;
        let mut lines = Vec::new();
        let (mut line, mut byte, mut seen) = (0, 0, 0);
        for (n, l) in text.split('\n').enumerate() {
            if cursor >= seen && cursor <= seen + l.len() {
                line = n;
                byte = cursor - seen;
            }
            seen += l.len() + 1;
            let mut tokens = Vec::new();
            state = lex_line(&state, l, Some(&mut tokens));
            lines.push((l.to_owned(), tokens));
        }
        analyze_lines(&lines, line, byte)
    }

    fn t(schema: Option<&str>, name: &str, alias: Option<&str>) -> TableRef {
        TableRef {
            schema: schema.map(str::to_owned),
            name: name.to_owned(),
            alias: alias.map(str::to_owned),
        }
    }

    #[test]
    fn tables_after_from_and_join() {
        let a = at("SELECT * FROM ord|").unwrap();
        assert_eq!(a.prefix, "ord");
        assert_eq!(a.want, Want::Relations);
        let a = at("select * from orders o join |").unwrap();
        assert_eq!(a.want, Want::Relations);
        assert_eq!(a.prefix, "");
        let a = at("select * from orders o, cu|").unwrap();
        assert_eq!(a.want, Want::Relations);
        let a = at("UPDATE |").unwrap();
        assert_eq!(a.want, Want::Relations);
        // not right after the keyword: an alias or a condition follows
        assert_eq!(
            at("select * from orders o join c on |").unwrap().want,
            Want::Any
        );
        assert_eq!(at("select * from orders wh|").unwrap().want, Want::Any);
    }

    #[test]
    fn qualified_names_and_aliases() {
        let a = at("SELECT o.| FROM sales.orders AS o, \"Customers\" c").unwrap();
        assert_eq!(a.want, Want::Qualified("o".into()));
        assert_eq!(
            a.tables,
            vec![
                t(Some("sales"), "orders", Some("o")),
                t(None, "Customers", Some("c"))
            ]
        );
        let a = at("select * from Sales.|").unwrap();
        assert_eq!(a.want, Want::Qualified("sales".into()));
        // the unfinished `public.` is no table named "public"
        let a = at("SELECT * FROM public.|").unwrap();
        assert_eq!(a.want, Want::Qualified("public".into()));
        assert!(a.tables.is_empty(), "{:?}", a.tables);
        let a = at("SELECT * FROM public.|, orders o").unwrap();
        assert_eq!(a.tables, vec![t(None, "orders", Some("o"))]);
        let a = at("select \"My T\".na| from x").unwrap();
        assert_eq!(a.want, Want::Qualified("My T".into()));
        assert_eq!(a.prefix, "na");
    }

    #[test]
    fn statement_is_between_semicolons() {
        let a = at("select * from a x;\nselect na| from b where 1 = 1;\nselect * from c").unwrap();
        assert_eq!(a.want, Want::Any);
        assert_eq!(a.prefix, "na");
        assert_eq!(a.tables, vec![t(None, "b", None)]);
        // a join list with aliases, keywords that are no aliases
        let a = at("select | from a left join b bb on a.id = bb.id where true").unwrap();
        assert_eq!(a.tables, vec![t(None, "a", None), t(None, "b", Some("bb"))]);
        // subqueries and functions are not tables
        let a = at("select | from (select 1) s, generate_series(1, 3) g").unwrap();
        assert!(a.tables.is_empty(), "{:?}", a.tables);
    }

    #[test]
    fn cursor_inside_a_word() {
        // the first word of the text and of a later statement
        let a = at("sel|ect 1").unwrap();
        assert_eq!((a.prefix.as_str(), &a.want), ("sel", &Want::Any));
        let a = at("select 1; sel|ect 2").unwrap();
        assert_eq!((a.prefix.as_str(), &a.want), ("sel", &Want::Any));
        // the context is the one before the word, as at its end
        let a = at("select * from us|ers").unwrap();
        assert_eq!((a.prefix.as_str(), &a.want), ("us", &Want::Relations));
        let a = at("select o.na|me from orders o").unwrap();
        assert_eq!(a.prefix, "na");
        assert_eq!(a.want, Want::Qualified("o".into()));
        assert_eq!(a.tables, vec![t(None, "orders", Some("o"))]);
    }

    #[test]
    fn nothing_inside_strings_comments_and_quotes() {
        assert!(at("select 'abc|' from t").is_none());
        assert!(at("select 'abc|").is_none());
        assert!(at("select 1 -- comm|").is_none());
        assert!(at("select \"Ab|").is_none());
        assert!(at("select /* x| */ 1").is_none());
        // right after a closed string: fine
        assert!(at("select 'a' |").is_some());
    }
}
