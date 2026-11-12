-- gts / gti / gtu / gtd (F4, D-054): the columns of one table, view or foreign table.
-- $1 schema or NULL (search path), $2 name. One row per column, all text:
-- schema, table, column, type, not_null, default (its expression or ''), identity
-- ('a' / 'd' / ''), generated ('s' / ''), pk_pos (position in the primary key or ''),
-- uk_pos (position in the first unique constraint whose columns are all NOT NULL),
-- uk_name.
WITH t AS (
    SELECT c.oid, n.nspname, c.relname
    FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE c.relname = $2::text
        AND CASE WHEN $1::text IS NULL THEN pg_catalog.pg_table_is_visible(c.oid)
            ELSE n.nspname = $1::text END
        AND c.relkind IN ('r', 'p', 'v', 'f')
), pk AS (
    SELECT r.conkey FROM pg_catalog.pg_constraint r, t
    WHERE r.conrelid = t.oid AND r.contype = 'p'
), uk AS (
    SELECT r.conname, r.conkey FROM pg_catalog.pg_constraint r, t
    WHERE r.conrelid = t.oid AND r.contype = 'u'
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_attribute x
            WHERE x.attrelid = t.oid AND x.attnum = ANY (r.conkey) AND NOT x.attnotnull)
    ORDER BY r.conname
    LIMIT 1
)
SELECT t.nspname::text, t.relname::text, a.attname::text,
    pg_catalog.format_type(a.atttypid, a.atttypmod)::text,
    a.attnotnull::text,
    COALESCE((SELECT pg_catalog.pg_get_expr(d.adbin, d.adrelid)
        FROM pg_catalog.pg_attrdef d
        WHERE d.adrelid = a.attrelid AND d.adnum = a.attnum AND a.attgenerated = ''), '')::text,
    a.attidentity::text,
    a.attgenerated::text,
    COALESCE(pg_catalog.array_position((SELECT conkey FROM pk), a.attnum)::text, ''),
    COALESCE(pg_catalog.array_position((SELECT conkey FROM uk), a.attnum)::text, ''),
    COALESCE((SELECT conname FROM uk)::text, '')
FROM t
JOIN pg_catalog.pg_attribute a ON a.attrelid = t.oid AND a.attnum > 0 AND NOT a.attisdropped
ORDER BY a.attnum
