-- gtv (F4, D-054): tables, columns and foreign keys between the tables of a view.
-- $1 schemas, $2 names: text in array syntax ({"a",NULL}; NULL: search path), same
-- length; bind parameters are text, so they are cast to text[] here.
-- All rows text, nine columns, the first says what the row is:
--   't', position, schema, name, relkind
--   'c', position, attnum, column
--   'f', referencing position, referenced position, constraint, pair number,
--        referencing column, referenced column, referencing column NOT NULL
-- Positions are 1-based in the order the names were typed.
WITH input AS (
    SELECT x.s, x.n, x.i
    FROM unnest($1::text::text[], $2::text::text[]) WITH ORDINALITY AS x(s, n, i)
), t AS (
    SELECT input.i, c.oid, n.nspname, c.relname, c.relkind
    FROM input
    JOIN pg_catalog.pg_class c ON c.relname = input.n
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE CASE WHEN input.s IS NULL THEN pg_catalog.pg_table_is_visible(c.oid)
            ELSE n.nspname = input.s END
        AND c.relkind IN ('r', 'p', 'v', 'f')
)
SELECT 't'::text, t.i::text, t.nspname::text, t.relname::text, t.relkind::text,
    ''::text, ''::text, ''::text, ''::text
FROM t
UNION ALL
SELECT 'c', t.i::text, a.attnum::text, a.attname::text, '', '', '', '', ''
FROM t
JOIN pg_catalog.pg_attribute a ON a.attrelid = t.oid AND a.attnum > 0 AND NOT a.attisdropped
UNION ALL
SELECT 'f', f.i::text, r.i::text, con.conname::text, k.ord::text,
    fa.attname::text, ra.attname::text, fa.attnotnull::text, ''
FROM t f
JOIN pg_catalog.pg_constraint con ON con.conrelid = f.oid AND con.contype = 'f'
JOIN t r ON r.oid = con.confrelid
CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(fk, rk, ord)
JOIN pg_catalog.pg_attribute fa ON fa.attrelid = f.oid AND fa.attnum = k.fk
JOIN pg_catalog.pg_attribute ra ON ra.attrelid = r.oid AND ra.attnum = k.rk
