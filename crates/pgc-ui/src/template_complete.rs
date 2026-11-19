//! Autocomplete in the body of a template tab (D-074): what fits at the cursor of a
//! Jinja template — the template's parameters, the fields of a list's rows, filters,
//! statements, feature names — and, in the SQL around the marks, the parameters as
//! complete marks and the SQL keywords. Pure logic; the list itself is the one of the
//! query tabs (`completion.rs`).

use pgc_core::Feature;
use pgc_sql::devtemplate::{self, Param, ParamType, Shape, Source};

use crate::completion::Item;

/// Where the cursor is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// In the SQL, outside every mark.
    Sql,
    /// In an expression of `{{ … }}` or `{% … %}`.
    Expr,
    /// Right after `{%`: the statement.
    Statement,
    /// After `name.`.
    Attr(String),
    /// After `|`.
    Filter,
    /// In the text of `has("…")`.
    Feature,
}

/// What is being typed at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Char index where the typed part of the name starts.
    pub start: usize,
    pub prefix: String,
    pub place: Place,
    /// The loops the cursor is in: (row variable, what it loops over).
    pub loops: Vec<(String, String)>,
    /// Variables made with `{% set %}` before the cursor.
    pub sets: Vec<String>,
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The name that ends at the end of `text`.
fn last_name(text: &str) -> &str {
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_name_char(*c))
        .last()
        .map_or(text.len(), |(i, _)| i);
    &text[start..]
}

/// The first name of `text`.
fn first_name(text: &str) -> &str {
    let text = text.trim_start();
    let end = text
        .char_indices()
        .find(|(_, c)| !is_name_char(*c))
        .map_or(text.len(), |(i, _)| i);
    &text[..end]
}

/// The `{% for %}` loops open at the end of `before`, and the `{% set %}` variables.
fn scopes(before: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut loops = Vec::new();
    let mut sets = Vec::new();
    let mut rest = before;
    while let Some(open) = rest.find("{%") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("%}") else {
            break;
        };
        let tag = after[..close].trim_matches(|c: char| c == '-' || c.is_whitespace());
        let word = first_name(tag);
        match word {
            "for" => {
                let body = &tag[word.len()..];
                let (vars, over) = body.split_once(" in ").unwrap_or((body, ""));
                // `for key, value in …`: both are variables of the loop
                let over = first_name(over).to_owned();
                let names: Vec<&str> = vars.split(',').map(str::trim).collect();
                loops.push((names.join(","), over));
            }
            "endfor" => {
                loops.pop();
            }
            "set" => {
                let name = first_name(&tag[word.len()..]);
                if !name.is_empty() && !sets.iter().any(|s| s == name) {
                    sets.push(name.to_owned());
                }
            }
            _ => {}
        }
        rest = &after[close + 2..];
    }
    // a loop with two variables counts once for each
    let loops = loops
        .into_iter()
        .flat_map(|(vars, over)| {
            vars.split(',')
                .filter(|v| !v.is_empty())
                .map(|v| (v.to_owned(), over.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    (loops, sets)
}

/// What is typed at char index `cursor` of the body `text`; `None` where nothing is
/// offered (in a comment, in a text other than a feature name).
pub fn analyze(text: &str, cursor: usize) -> Option<Found> {
    let byte = text
        .char_indices()
        .nth(cursor)
        .map_or(text.len(), |(i, _)| i);
    let before = &text[..byte];
    let prefix = last_name(before);
    let start = cursor - prefix.chars().count();
    let head = &before[..before.len() - prefix.len()];
    let (loops, sets) = scopes(head);
    let found = |place| {
        Some(Found {
            start,
            prefix: prefix.to_owned(),
            place,
            loops: loops.clone(),
            sets: sets.clone(),
        })
    };
    // the mark the cursor is in: the last opener that is not closed
    let opener = ["{{", "{%", "{#"]
        .iter()
        .filter_map(|o| head.rfind(o).map(|at| (at, *o)))
        .max();
    let inner = opener.and_then(|(at, o)| {
        let inner = &head[at + 2..];
        let close = match o {
            "{{" => "}}",
            "{%" => "%}",
            _ => "#}",
        };
        (!inner.contains(close)).then_some((o, inner))
    });
    let Some((opener, inner)) = inner else {
        return found(Place::Sql);
    };
    if opener == "{#" {
        return None;
    }
    // inside a text: only the feature name of has("…")
    let quotes = inner.chars().filter(|c| *c == '"').count();
    let singles = inner.chars().filter(|c| *c == '\'').count();
    if quotes % 2 == 1 || singles % 2 == 1 {
        let quote = if quotes % 2 == 1 { '"' } else { '\'' };
        let open = inner.rfind(quote)?;
        // the whole text so far is the prefix (feature names are plain names)
        let typed = &inner[open + 1..];
        if !typed.is_empty() || !inner[..open].trim_end().ends_with("has(") {
            return None;
        }
        return found(Place::Feature);
    }
    if let Some(owner) = inner.strip_suffix('.') {
        let owner = last_name(owner);
        if owner.is_empty() {
            return None;
        }
        return found(Place::Attr(owner.to_owned()));
    }
    let trimmed = inner.trim_end();
    if trimmed.ends_with('|') {
        return found(Place::Filter);
    }
    if opener == "{%" && trimmed.trim_start_matches('-').is_empty() {
        return found(Place::Statement);
    }
    found(Place::Expr)
}

/// Short notes for the filters in the list.
const FILTER_NOTES: [(&str, &str); 11] = [
    ("ident", "quote as a name"),
    ("qtLiteral", "quote as a text"),
    ("qtTypeIdent", "quote as a type name"),
    ("indent", "indent every line"),
    ("default", "value when not defined"),
    ("upper", "upper case"),
    ("lower", "lower case"),
    ("trim", "without outer spaces"),
    ("replace", "replace a text"),
    ("join", "rows in one line"),
    ("length", "number of rows / characters"),
];

const STATEMENTS: [(&str, &str); 7] = [
    ("if", "if "),
    ("elif", "elif "),
    ("else", "else"),
    ("endif", "endif"),
    ("for", "for "),
    ("endfor", "endfor"),
    ("set", "set "),
];

const WORDS: [&str; 8] = ["not", "and", "or", "in", "is", "true", "false", "none"];

/// The parts of a value chosen from the database.
fn object_parts(source: Source) -> &'static [&'static str] {
    match source.shape() {
        Shape::Name => &[],
        Shape::Qualified => &["schema", "name"],
        Shape::WithArgs => &["schema", "name", "args"],
        Shape::OnTable => &["schema", "name", "table"],
    }
}

/// The entries of the list for `found`, with the template's parameters and the SQL
/// keywords (lower case).
pub fn items(found: &Found, params: &[Param], keywords: &[&str]) -> Vec<Item> {
    let prefix = found.prefix.to_lowercase();
    let fits = |name: &str| name.to_lowercase().starts_with(&prefix);
    let mut out: Vec<Item> = Vec::new();
    let mut push = |label: &str, kind: &'static str, insert: String| {
        if fits(label) {
            out.push(Item {
                label: label.to_owned(),
                kind,
                insert,
            });
        }
    };
    let list_of = |var: &str| {
        found
            .loops
            .iter()
            .rev()
            .find(|(v, _)| v == var)
            .and_then(|(_, over)| params.iter().find(|p| p.name == *over))
            .filter(|p| p.kind == ParamType::List)
    };
    match &found.place {
        Place::Sql => {
            // a parameter is written as its whole mark
            for p in params {
                push(&p.name, p.kind.name(), devtemplate::param_snippet(p));
            }
            if !prefix.is_empty() {
                for k in keywords {
                    push(&k.to_uppercase(), "keyword", k.to_uppercase());
                }
            }
        }
        Place::Expr => {
            for (var, _) in found.loops.iter().rev() {
                push(var, "row", var.clone());
            }
            if !found.loops.is_empty() {
                push("loop", "loop", "loop".to_owned());
            }
            for p in params {
                push(&p.name, p.kind.name(), p.name.clone());
            }
            for s in &found.sets {
                push(s, "variable", s.clone());
            }
            for (name, _) in devtemplate::CONTEXT_VARIABLES {
                push(name, "always there", name.to_owned());
            }
            push("has", "server feature", "has(\"".to_owned());
            if !prefix.is_empty() {
                for w in WORDS {
                    push(w, "keyword", w.to_owned());
                }
            }
        }
        Place::Statement => {
            for (name, insert) in STATEMENTS {
                push(name, "statement", insert.to_owned());
            }
        }
        Place::Attr(owner) => {
            if let Some(list) = list_of(owner) {
                for f in &list.fields {
                    push(&f.name, f.kind.name(), f.name.clone());
                }
            } else if owner == "loop" {
                for name in ["last", "first", "index", "index0", "length"] {
                    push(name, "loop", name.to_owned());
                }
            } else if let Some(source) = params
                .iter()
                .find(|p| p.name == *owner)
                .and_then(|p| p.source)
            {
                for part in object_parts(source) {
                    push(part, "part", (*part).to_owned());
                }
            }
        }
        Place::Filter => {
            for (name, text, _) in devtemplate::FILTERS {
                let note = FILTER_NOTES
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or("filter", |(_, note)| note);
                push(name, note, text.trim_start_matches('|').to_owned());
            }
        }
        Place::Feature => {
            for f in Feature::ALL {
                let since = crate::dialog::intern(&format!("PostgreSQL {}+", f.since() / 10000));
                push(f.name(), since, f.name().to_owned());
            }
        }
    }
    // names first as they are declared; the keywords of the SQL sorted behind them
    let (mut names, mut words): (Vec<Item>, Vec<Item>) =
        out.into_iter().partition(|i| i.kind != "keyword");
    words.sort_by(|a, b| a.label.cmp(&b.label));
    names.append(&mut words);
    let mut seen: Vec<String> = Vec::new();
    names.retain(|i| {
        let new = !seen.contains(&i.label);
        if new {
            seen.push(i.label.clone());
        }
        new
    });
    // a name typed out completely is not offered again (only itself)
    if names.len() == 1 && names[0].insert == found.prefix {
        names.clear();
    }
    names
}

/// The list opens by itself here after a typed character: in a mark from the first
/// letter (and after `.`, `|`, `has("`), in the SQL after the third letter.
pub fn opens_by_typing(found: &Found) -> bool {
    let n = found.prefix.chars().count();
    match found.place {
        Place::Sql => n == crate::completion::AUTO_LETTERS,
        Place::Attr(_) | Place::Filter | Place::Feature => n <= 1,
        Place::Expr | Place::Statement => n == 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Vec<Param> {
        let mut schema = Param::new("schema", ParamType::Choice);
        schema.source = Some(Source::Schemas);
        let mut table = Param::new("table", ParamType::Choice);
        table.source = Some(Source::Tables);
        let mut func = Param::new("func", ParamType::Choice);
        func.source = Some(Source::Functions);
        let mut cols = Param::new("columns", ParamType::List);
        cols.fields = vec![
            Param::new("col_name", ParamType::Identifier),
            Param::new("col_type", ParamType::Text),
        ];
        vec![
            schema,
            table,
            func,
            Param::new("name", ParamType::Identifier),
            Param::new("unique", ParamType::Bool),
            cols,
        ]
    }

    /// Analysis at the `|` mark... the cursor is where `^` stands in `text`.
    fn at(text: &str) -> Option<Found> {
        let cursor = text.chars().position(|c| c == '^').expect("a ^");
        analyze(&text.replace('^', ""), cursor)
    }

    fn labels(text: &str) -> Vec<String> {
        let found = at(text).expect("something to complete");
        items(&found, &params(), &["select", "schema", "set"])
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    fn inserts(text: &str) -> Vec<String> {
        let found = at(text).expect("something to complete");
        items(&found, &params(), &["select"])
            .into_iter()
            .map(|i| i.insert)
            .collect()
    }

    #[test]
    fn places() {
        let f = at("SELECT na^").unwrap();
        assert_eq!((f.place, f.prefix.as_str(), f.start), (Place::Sql, "na", 7));
        assert_eq!(at("{{ na^").unwrap().place, Place::Expr);
        assert_eq!(at("{{ a }} x^").unwrap().place, Place::Sql);
        assert_eq!(at("{% ^").unwrap().place, Place::Statement);
        assert_eq!(at("{%- en^").unwrap().place, Place::Statement);
        assert_eq!(at("{% if un^").unwrap().place, Place::Expr);
        assert_eq!(at("{{ name|^").unwrap().place, Place::Filter);
        assert_eq!(at("{{ name | up^").unwrap().place, Place::Filter);
        assert_eq!(
            at("{{ table.^").unwrap().place,
            Place::Attr("table".to_owned())
        );
        assert_eq!(
            at("{{ row.co^").unwrap().place,
            Place::Attr("row".to_owned())
        );
        assert_eq!(at("{% if has(\"^").unwrap().place, Place::Feature);
        // nothing in comments and in other texts
        assert_eq!(at("{# na^"), None);
        assert_eq!(at("{% if kind == \"bt^"), None);
        assert_eq!(at("{{ .^"), None);
        // a multi-byte character before the cursor
        let f = at("-- š\n{{ na^").unwrap();
        assert_eq!((f.place, f.start), (Place::Expr, 8));
    }

    #[test]
    fn loops_and_sets_in_scope() {
        let f = at("{% set pk = 1 %}{% for c in columns %}{{ ^").unwrap();
        assert_eq!(f.loops, vec![("c".to_owned(), "columns".to_owned())]);
        assert_eq!(f.sets, vec!["pk"]);
        // a closed loop is over
        let f = at("{% for c in columns %}x{% endfor %}{{ ^").unwrap();
        assert!(f.loops.is_empty());
        let f = at("{%- for k, v in pairs -%}{{ ^").unwrap();
        assert_eq!(f.loops.len(), 2);
        assert_eq!(f.loops[1], ("v".to_owned(), "pairs".to_owned()));
    }

    #[test]
    fn entries_in_marks() {
        // the row variable and `loop` first, then the parameters in their order
        let all = labels("{% for c in columns %}{{ ^");
        assert_eq!(
            all,
            [
                "c",
                "loop",
                "schema",
                "table",
                "func",
                "name",
                "unique",
                "columns",
                "server_version",
                "database",
                "has"
            ]
        );
        assert_eq!(labels("{{ s^"), ["schema", "server_version"]);
        assert_eq!(labels("{% if n^"), ["name", "none", "not"]);
        // the fields of the list the loop goes over
        assert_eq!(
            labels("{% for c in columns %}{{ c.^"),
            ["col_name", "col_type"]
        );
        assert_eq!(labels("{% for c in columns %}{{ c.col_t^"), ["col_type"]);
        assert_eq!(
            labels("{% for c in columns %}{% if not loop.^"),
            ["last", "first", "index", "index0", "length"]
        );
        // the parts of an object from the database
        assert_eq!(labels("{{ table.^"), ["schema", "name"]);
        assert_eq!(labels("{{ func.^"), ["schema", "name", "args"]);
        assert!(labels("{{ schema.^").is_empty());
        assert!(labels("{{ unknown.^").is_empty());
        // filters, with their arguments
        assert_eq!(labels("{{ name|i^"), ["ident", "indent"]);
        assert_eq!(inserts("{{ name|ind^"), ["indent(4, true)"]);
        // statements
        assert_eq!(labels("{% e^"), ["elif", "else", "endif", "endfor"]);
        assert_eq!(inserts("{% f^"), ["for "]);
        // features
        assert!(labels("{% if has(\"^").contains(&"NullsNotDistinct".to_owned()));
        // a name typed out is not offered again
        assert!(labels("{{ unique^").is_empty());
    }

    #[test]
    fn entries_in_the_sql() {
        // parameters as their whole mark, then the keywords
        assert_eq!(
            labels("CREATE TABLE s^"),
            ["schema", "SCHEMA", "SELECT", "SET"]
        );
        assert_eq!(inserts("CREATE TABLE sch^"), ["{{ schema|ident }}"]);
        assert_eq!(inserts("x na^"), ["{{ name|ident }}"]);
        assert_eq!(inserts("uni^"), ["{% if unique %}\n\n{% endif %}\n"]);
        // without a typed letter only the parameters
        assert_eq!(labels("CREATE ^").len(), params().len());
    }

    #[test]
    fn opening_by_typing() {
        assert!(opens_by_typing(&at("{{ n^").unwrap()));
        assert!(!opens_by_typing(&at("{{ na^").unwrap()));
        assert!(opens_by_typing(&at("{{ name|^").unwrap()));
        assert!(opens_by_typing(&at("{{ table.^").unwrap()));
        assert!(opens_by_typing(&at("{% if has(\"^").unwrap()));
        assert!(!opens_by_typing(&at("se^").unwrap()));
        assert!(opens_by_typing(&at("sel^").unwrap()));
        assert!(!opens_by_typing(&at("sele^").unwrap()));
    }
}
