//! Where a server message points in the sent text (D-019, D-020): the error position,
//! or the line of the running `DO` body from the `CONTEXT`, with the column from
//! PostgreSQL's internal position; and the psql-style `LINE n:` text with its caret.

use pgc_db::ServerMessage;
use pgc_editor::lexer::{LexState, Token, TokenKind, lex_line};
use unicode_width::UnicodeWidthChar;

/// A place in the sent text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    /// Char index in the sent text.
    pub pos: usize,
    /// The column is known (else `pos` is the first non-blank character of the line).
    pub exact: bool,
}

/// The statements of a sent text, split once per run: a script that raises thousands
/// of notices is not read again for each of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Statements(Vec<Statement>);

/// [`locate_in`] for a text whose statements are not kept.
#[cfg(test)]
pub fn locate(sql: &str, m: &ServerMessage, completed: usize) -> Option<Located> {
    locate_in(sql, m, completed, &std::cell::OnceCell::new())
}

/// Where `m` points in `sql`; `completed`: statements of the run that had completed
/// when it arrived (the running one is the next). `split` keeps the statements of
/// `sql` (filled when they are first needed).
pub fn locate_in(
    sql: &str,
    m: &ServerMessage,
    completed: usize,
    split: &std::cell::OnceCell<Statements>,
) -> Option<Located> {
    if let Some(p) = m.position {
        let pos = (p as usize).saturating_sub(1).min(sql.chars().count());
        return Some(Located { pos, exact: true });
    }
    let context = m.context.as_deref()?;
    let (frame, n) = context
        .lines()
        .enumerate()
        .find_map(|(i, l)| Some((i, do_line(l)?)))?;
    let stmts = &split.get_or_init(|| Statements(statements(sql))).0;
    let body = match stmts.get(completed) {
        Some(s) if s.is_do => s.body,
        _ => {
            let mut dos = stmts.iter().filter(|s| s.is_do);
            match (dos.next(), dos.next()) {
                (Some(s), None) => s.body,
                _ => None,
            }
        }
    }?;
    // line N of the body: line 1 starts right after the opening quote
    let mut line = body;
    for _ in 1..n {
        line += sql[line..].find('\n')? + 1;
    }
    // the statement is in this DO (not in a function it called): the internal query
    // gives the column
    if frame == 0
        && let (Some(q), Some(ip)) = (m.internal_query.as_deref(), m.internal_position)
        && !q.is_empty()
        && let Some(at) = sql[line..].find(q)
    {
        let start = char_index(sql, line + at);
        return Some(Located {
            pos: start + (ip as usize).saturating_sub(1),
            exact: true,
        });
    }
    let end = sql[line..].find('\n').map_or(sql.len(), |e| line + e);
    let blank = sql[line..end]
        .find(|c: char| c != ' ' && c != '\t' && c != '\r')
        .unwrap_or(0);
    Some(Located {
        pos: char_index(sql, line + blank),
        exact: false,
    })
}

/// N of `PL/pgSQL function inline_code_block line N …` (also `compilation of PL/pgSQL
/// function "inline_code_block" near line N`). The words around it are in the server's
/// language (`lc_messages`); the function's name and the number are not, so the line
/// is the first number after the name.
fn do_line(context_line: &str) -> Option<usize> {
    const NAME: &str = "inline_code_block";
    let rest = &context_line[context_line.find(NAME)? + NAME.len()..];
    let digits = rest.trim_start_matches(|c: char| !c.is_ascii_digit());
    let n: usize = digits
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    (n > 0).then_some(n)
}

fn char_index(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

/// A statement of the sent text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Statement {
    is_do: bool,
    /// Byte offset where the body of a `DO` starts (after its opening quote).
    body: Option<usize>,
}

/// The statements of `sql` as PostgreSQL runs them (empty ones left out), split at
/// `;` outside strings, comments and bodies with the editor's lexer.
fn statements(sql: &str) -> Vec<Statement> {
    let mut out = Vec::new();
    let mut state = LexState::Normal;
    // open bodies (their tags), as the lexer has them
    let mut tags: Vec<String> = Vec::new();
    // the current statement: has text / is a DO / its body
    let mut cur: Option<Statement> = None;
    let mut tokens: Vec<Token> = Vec::new();
    let mut line_start = 0;
    for line in sql.split('\n') {
        tokens.clear();
        state = lex_line(&state, line, Some(&mut tokens));
        for t in &tokens {
            let text = &line[t.start..t.end];
            let at = line_start + t.start;
            if t.kind == TokenKind::BodyQuote {
                let tag = text.trim_matches('$');
                if let Some(k) = tags.iter().position(|x| x == tag) {
                    tags.truncate(k);
                } else {
                    if tags.is_empty()
                        && let Some(s) = cur.as_mut()
                        && s.is_do
                        && s.body.is_none()
                    {
                        s.body = Some(at + text.len());
                    }
                    tags.push(tag.to_owned());
                }
                continue;
            }
            if !tags.is_empty() || t.kind == TokenKind::Comment || text.trim().is_empty() {
                continue;
            }
            if t.kind == TokenKind::Operator && text == ";" {
                out.extend(cur.take());
                continue;
            }
            let s = cur.get_or_insert(Statement {
                is_do: text.eq_ignore_ascii_case("do"),
                body: None,
            });
            // `DO LANGUAGE plpgsql $$ … $$` / `DO '…'`: the first string is the body
            if s.is_do && s.body.is_none() && t.kind == TokenKind::String {
                let open = if let Some(rest) = text.strip_prefix('$') {
                    rest.find('$').map_or(1, |e| e + 2)
                } else if text.starts_with(['e', 'E']) {
                    2
                } else {
                    1
                };
                s.body = Some(at + open.min(text.len()));
            }
        }
        line_start += line.len() + 1;
    }
    out.extend(cur);
    out
}

/// Longest shown part of a line, and the room kept right of the caret (as psql).
const DISPLAY_SIZE: usize = 60;
const MIN_RIGHT_CUT: usize = 10;

/// psql's `LINE n: …` for char index `pos` of `sql` (`line_no` 1-based, the editor
/// line), and a `^` under the column when it is `exact`.
pub fn line_text(sql: &str, pos: usize, exact: bool, line_no: usize) -> String {
    let byte = sql.char_indices().nth(pos).map_or(sql.len(), |(b, _)| b);
    let start = sql[..byte].rfind('\n').map_or(0, |i| i + 1);
    let end = sql[byte..].find('\n').map_or(sql.len(), |e| byte + e);
    let line = sql[start..end].trim_end_matches('\r');
    // (char, display column), tabs shown as one space
    let mut cells: Vec<(char, usize)> = Vec::new();
    let mut width = 0;
    let mut caret = None;
    for (b, c) in line.char_indices() {
        if start + b == byte {
            caret = Some(width);
        }
        let c = if c == '\t' { ' ' } else { c };
        cells.push((c, width));
        width += c.width().unwrap_or(0);
    }
    let caret = caret.unwrap_or(width);
    let (from, to) = if width <= DISPLAY_SIZE {
        (0, width)
    } else if !exact || caret + MIN_RIGHT_CUT < DISPLAY_SIZE {
        (0, DISPLAY_SIZE)
    } else {
        let to = (caret + MIN_RIGHT_CUT).min(width);
        (to.saturating_sub(DISPLAY_SIZE), to)
    };
    let mut shown: String = cells
        .iter()
        .filter(|(_, col)| *col >= from && *col < to)
        .map(|(c, _)| *c)
        .collect();
    let lead = if from > 0 { "..." } else { "" };
    if to < width {
        shown.push_str("...");
    }
    let head = format!("LINE {line_no}: ");
    let mut s = format!("{head}{lead}{shown}");
    if exact {
        let indent = head.len() + lead.len() + caret - from;
        s.push('\n');
        s.push_str(&" ".repeat(indent));
        s.push('^');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(position: Option<u32>, context: Option<&str>) -> ServerMessage {
        ServerMessage {
            severity: "ERROR".into(),
            message: "boom".into(),
            context: context.map(str::to_owned),
            position,
            ..ServerMessage::default()
        }
    }

    fn at(sql: &str, l: Located) -> (&str, bool) {
        let b = sql.char_indices().nth(l.pos).map_or(sql.len(), |(b, _)| b);
        (&sql[b..], l.exact)
    }

    #[test]
    fn position_is_exact() {
        let sql = "select 1;\nselect čx from t;";
        let l = locate(sql, &msg(Some(18), None), 1).unwrap();
        assert_eq!(
            at(sql, l),
            ("čx from t;", true),
            "char positions, not bytes"
        );
    }

    #[test]
    fn statements_split_outside_strings_and_bodies() {
        let sql = "select ';';\n-- x;\nDO $b$ begin\n perform 1; end $b$;\n;\n\
                   do language plpgsql $$ x $$; create function f() returns int as $f$ \
                   select 1; $f$ language sql";
        let s = statements(sql);
        assert_eq!(s.len(), 4, "{s:?}");
        assert!(!s[0].is_do && s[1].is_do && s[2].is_do && !s[3].is_do);
        assert_eq!(&sql[s[1].body.unwrap()..][..6], " begin");
        assert_eq!(&sql[s[2].body.unwrap()..][..3], " x ");
    }

    #[test]
    fn do_lines_in_other_languages() {
        for (context, line) in [
            (
                "PL/pgSQL function inline_code_block line 3 at RAISE",
                Some(3),
            ),
            (
                "compilation of PL/pgSQL function \"inline_code_block\" near line 12",
                Some(12),
            ),
            (
                "PL/pgSQL-Funktion inline_code_block Zeile 4 bei RAISE",
                Some(4),
            ),
            (
                "fonction PL/pgSQL inline_code_block, ligne 7 à RAISE",
                Some(7),
            ),
            ("PL/pgSQL function f(integer) line 3 at RAISE", None),
            ("PL/pgSQL function inline_code_block", None),
            ("PL/pgSQL function inline_code_block line 0", None),
        ] {
            assert_eq!(do_line(context), line, "{context}");
        }
    }

    #[test]
    fn do_body_lines_from_the_context() {
        let sql = "select 1;\nDO $$\nBEGIN\n  RAISE NOTICE 'hi';\nEND $$;";
        let ctx = "PL/pgSQL function inline_code_block line 3 at RAISE";
        // one statement completed: the DO runs
        let l = locate(sql, &msg(None, Some(ctx)), 1).unwrap();
        assert_eq!(at(sql, l), ("RAISE NOTICE 'hi';\nEND $$;", false));
        // a count that does not fit: the only DO
        let l2 = locate(sql, &msg(None, Some(ctx)), 0).unwrap();
        assert_eq!(l, l2);
        let ctx = "compilation of PL/pgSQL function \"inline_code_block\" near line 2";
        let l = locate(sql, &msg(None, Some(ctx)), 1).unwrap();
        assert!(at(sql, l).0.starts_with("BEGIN"));
        // two DOs and a count that does not point at one: no position
        let two = "DO $$ begin end $$; DO $$ begin end $$; select 1";
        assert_eq!(locate(two, &msg(None, Some(ctx)), 2), None);
        // the second DO
        let ctx = "PL/pgSQL function inline_code_block line 1 at RAISE";
        let l = locate(two, &msg(None, Some(ctx)), 1).unwrap();
        assert_eq!(at(two, l), ("begin end $$; select 1", false));
        assert_eq!(locate(sql, &msg(None, Some("SQL function f")), 1), None);
    }

    #[test]
    fn internal_query_gives_the_column() {
        let sql = "DO $$\nBEGIN\n  PERFORM 1;\n  INSERT INTO t VALUES (nope);\nEND $$;";
        let mut m = msg(
            None,
            Some("PL/pgSQL function inline_code_block line 4 at SQL statement"),
        );
        m.internal_query = Some("INSERT INTO t VALUES (nope)".into());
        m.internal_position = Some(23);
        let l = locate(sql, &m, 0).unwrap();
        assert_eq!(at(sql, l), ("nope);\nEND $$;", true));
        // the statement is in a function the DO called: only the line of the DO
        m.context = Some(
            "PL/pgSQL function f() line 9 at SQL statement\n\
             PL/pgSQL function inline_code_block line 4 at PERFORM"
                .into(),
        );
        let l = locate(sql, &m, 0).unwrap();
        assert_eq!(at(sql, l), ("INSERT INTO t VALUES (nope);\nEND $$;", false));
    }

    #[test]
    fn line_text_as_psql() {
        let sql = "select 1;\nselect *\tfrom custmers;";
        let pos = sql.find("custmers").unwrap();
        assert_eq!(
            line_text(sql, pos, true, 2),
            format!("LINE 2: select * from custmers;\n{}^", " ".repeat(22))
        );
        assert_eq!(
            line_text(sql, pos, false, 2),
            "LINE 2: select * from custmers;"
        );
        // a long line is cut around the caret
        let long = format!("select {} from x", "a, ".repeat(40));
        let pos = long.find("from").unwrap();
        let t = line_text(&long, pos, true, 1);
        let (first, caret) = t.split_once('\n').unwrap();
        assert!(
            first.starts_with("LINE 1: ...") && first.ends_with("from x"),
            "{first}"
        );
        let col = caret.find('^').unwrap();
        assert_eq!(&first[col..col + 4], "from");
        // near the start only the end is cut
        let t = line_text(&long, 0, true, 1);
        assert!(t.starts_with("LINE 1: select a, ") && t.contains("...\n"));
    }
}
