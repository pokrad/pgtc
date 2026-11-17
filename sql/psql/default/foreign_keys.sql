-- gtg / gtj / gtr (F4, D-054): the foreign keys of one table in both directions and
-- the columns of the tables on their other side. $1 schema, $2 name (as generate.sql
-- found them). Constraints a partition inherits (conparentid) are left out.
-- All rows text, nine columns, the first says what the row is:
--   'k', direction ('o': this table references the other one, 'i': the other one
--        references this table), constraint, pair number, other schema, other table,
--        this table's column, other table's column, referencing column NOT NULL
--   'c', other schema, other table, attnum, column
WITH t AS (
    SELECT c.oid
    FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = $1::text AND c.relname = $2::text
), k AS (
    SELECT 'o'::text AS dir, con.conname, k.ord, con.confrelid AS other,
        ta.attname AS this_col, oa.attname AS other_col, ta.attnotnull AS ref_not_null
    FROM t
    JOIN pg_catalog.pg_constraint con
        ON con.conrelid = t.oid AND con.contype = 'f' AND con.conparentid = 0
    CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(tk, ok, ord)
    JOIN pg_catalog.pg_attribute ta ON ta.attrelid = t.oid AND ta.attnum = k.tk
    JOIN pg_catalog.pg_attribute oa ON oa.attrelid = con.confrelid AND oa.attnum = k.ok
    UNION ALL
    SELECT 'i'::text, con.conname, k.ord, con.conrelid,
        ta.attname, oa.attname, oa.attnotnull
    FROM t
    JOIN pg_catalog.pg_constraint con
        ON con.confrelid = t.oid AND con.contype = 'f' AND con.conparentid = 0
    CROSS JOIN LATERAL unnest(con.confkey, con.conkey) WITH ORDINALITY AS k(tk, ok, ord)
    JOIN pg_catalog.pg_attribute ta ON ta.attrelid = t.oid AND ta.attnum = k.tk
    JOIN pg_catalog.pg_attribute oa ON oa.attrelid = con.conrelid AND oa.attnum = k.ok
)
SELECT 'k'::text, k.dir, k.conname::text, k.ord::text, n.nspname::text, c.relname::text,
    k.this_col::text, k.other_col::text, k.ref_not_null::text
FROM k
JOIN pg_catalog.pg_class c ON c.oid = k.other
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
UNION ALL
SELECT 'c', n.nspname::text, c.relname::text, a.attnum::text, a.attname::text,
    '', '', '', ''
FROM (SELECT DISTINCT other FROM k) o
JOIN pg_catalog.pg_class c ON c.oid = o.other
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
