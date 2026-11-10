-- Columns of one table for autocomplete (D-052).
-- $1 = schema (NULL: the table visible on the search path), $2 = table name.
SELECT a.attname::text AS name
FROM pg_catalog.pg_attribute a
JOIN pg_catalog.pg_class c ON c.oid = a.attrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relname = $2::text
    AND CASE
        WHEN $1::text IS NULL THEN pg_catalog.pg_table_is_visible(c.oid)
        ELSE n.nspname = $1::text
    END
    AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
    AND a.attnum > 0
    AND NOT a.attisdropped
ORDER BY a.attnum
