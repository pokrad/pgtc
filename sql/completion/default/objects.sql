-- Names for autocomplete (D-052): schemas, relations, functions and types of the
-- current database. visible = reachable without a schema (search path).
-- Columns: schema, name, kind, visible ('t' / 'f').
SELECT NULL::text AS schema, n.nspname::text AS name, 'schema' AS kind, true AS visible
FROM pg_catalog.pg_namespace n
WHERE n.nspname !~ '^pg_(toast|temp_|toast_temp_)'
UNION ALL
SELECT n.nspname::text, c.relname::text,
    CASE c.relkind
        WHEN 'v' THEN 'view'
        WHEN 'm' THEN 'materialized view'
        WHEN 'f' THEN 'foreign table'
        WHEN 'S' THEN 'sequence'
        ELSE 'table'
    END,
    pg_catalog.pg_table_is_visible(c.oid)
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f', 'S')
    AND n.nspname !~ '^pg_(toast|temp_|toast_temp_)'
UNION ALL
SELECT DISTINCT n.nspname::text, p.proname::text,
    CASE p.prokind WHEN 'p' THEN 'procedure' WHEN 'a' THEN 'aggregate' ELSE 'function' END,
    pg_catalog.pg_function_is_visible(p.oid)
FROM pg_catalog.pg_proc p
JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
WHERE p.prokind <> 'w'
    AND n.nspname !~ '^pg_(toast|temp_|toast_temp_)'
UNION ALL
SELECT n.nspname::text, t.typname::text, 'type', pg_catalog.pg_type_is_visible(t.oid)
FROM pg_catalog.pg_type t
JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace
WHERE t.typtype IN ('b', 'c', 'd', 'e', 'm', 'r')
    AND t.typcategory <> 'A'
    AND (t.typtype <> 'c'
        OR (SELECT c.relkind FROM pg_catalog.pg_class c WHERE c.oid = t.typrelid) = 'c')
    AND n.nspname !~ '^pg_(toast|temp_|toast_temp_)'
