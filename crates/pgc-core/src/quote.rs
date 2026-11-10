//! Identifier and literal quoting — the single implementation used everywhere
//! (CLAUDE.md). Same rules as PostgreSQL's `quote_ident` / `quote_literal`.
//!
//! Note: user-supplied *values* go into SQL as bind parameters; `quote_literal` is for
//! generated DDL text only (D-010, D-012).

/// Keywords that force quoting: PostgreSQL's reserved, type/function-name and
/// column-name keywords (all categories except "unreserved"), across versions 11–18.
const QUOTE_KEYWORDS: &[&str] = &[
    // reserved
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "constraint",
    "create",
    "current_catalog",
    "current_date",
    "current_role",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "from",
    "grant",
    "group",
    "having",
    "in",
    "initially",
    "intersect",
    "into",
    "lateral",
    "leading",
    "limit",
    "localtime",
    "localtimestamp",
    "not",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "placing",
    "primary",
    "references",
    "returning",
    "select",
    "session_user",
    "some",
    "symmetric",
    "system_user",
    "table",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "variadic",
    "when",
    "where",
    "window",
    "with",
    // type / function name
    "authorization",
    "binary",
    "collation",
    "concurrently",
    "cross",
    "current_schema",
    "freeze",
    "full",
    "ilike",
    "inner",
    "is",
    "isnull",
    "join",
    "left",
    "like",
    "natural",
    "notnull",
    "outer",
    "overlaps",
    "right",
    "similar",
    "tablesample",
    "verbose",
    // column name
    "between",
    "bigint",
    "bit",
    "boolean",
    "char",
    "character",
    "coalesce",
    "dec",
    "decimal",
    "exists",
    "extract",
    "float",
    "greatest",
    "grouping",
    "inout",
    "int",
    "integer",
    "interval",
    "json",
    "json_array",
    "json_arrayagg",
    "json_exists",
    "json_object",
    "json_objectagg",
    "json_query",
    "json_scalar",
    "json_serialize",
    "json_table",
    "json_value",
    "least",
    "merge_action",
    "national",
    "nchar",
    "none",
    "normalize",
    "nullif",
    "numeric",
    "out",
    "overlay",
    "position",
    "precision",
    "real",
    "row",
    "setof",
    "smallint",
    "substring",
    "time",
    "timestamp",
    "treat",
    "trim",
    "values",
    "varchar",
    "xmlattributes",
    "xmlconcat",
    "xmlelement",
    "xmlexists",
    "xmlforest",
    "xmlnamespaces",
    "xmlparse",
    "xmlpi",
    "xmlroot",
    "xmlserialize",
    "xmltable",
];

/// True if `name` must be written in double quotes.
pub fn needs_quoting(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return true;
    };
    if !(first.is_ascii_lowercase() || first == '_') {
        return true;
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return true;
    }
    QUOTE_KEYWORDS.contains(&name)
}

/// `quote_ident`: `customers` → `customers`, `Customers` → `"Customers"`,
/// `a"b` → `"a""b"`, `select` → `"select"`.
pub fn quote_ident(name: &str) -> String {
    if needs_quoting(name) {
        format!("\"{}\"", name.replace('"', "\"\""))
    } else {
        name.to_owned()
    }
}

/// Quotes each part and joins with `.`: `["public", "My T"]` → `public."My T"`.
pub fn quote_qualified(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|p| quote_ident(p))
        .collect::<Vec<_>>()
        .join(".")
}

/// `quote_literal`: `it's` → `'it''s'`; text with a backslash uses `E'...'` with the
/// backslashes doubled (same as PostgreSQL).
pub fn quote_literal(value: &str) -> String {
    let body = value.replace('\'', "''");
    if value.contains('\\') {
        format!("E'{}'", body.replace('\\', "\\\\"))
    } else {
        format!("'{body}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idents() {
        assert_eq!(quote_ident("customers"), "customers");
        assert_eq!(quote_ident("_x1"), "_x1");
        assert_eq!(quote_ident("Customers"), "\"Customers\"");
        assert_eq!(quote_ident("my table"), "\"my table\"");
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(quote_ident("1abc"), "\"1abc\"");
        assert_eq!(quote_ident("select"), "\"select\"");
        assert_eq!(quote_ident("user"), "\"user\"");
        assert_eq!(quote_ident("name"), "name"); // unreserved keyword
        assert_eq!(quote_ident(""), "\"\"");
        assert_eq!(quote_ident("čevapi"), "\"čevapi\"");
        assert_eq!(quote_qualified(&["public", "My T"]), "public.\"My T\"");
    }

    #[test]
    fn literals() {
        assert_eq!(quote_literal("abc"), "'abc'");
        assert_eq!(quote_literal("it's"), "'it''s'");
        assert_eq!(quote_literal("a\\b'"), "E'a\\\\b'''");
    }

    #[test]
    fn keyword_list_is_lower_case_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in QUOTE_KEYWORDS {
            assert_eq!(*k, k.to_lowercase());
            assert!(seen.insert(k), "duplicate {k}");
        }
    }
}
