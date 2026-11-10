-- \dp (D-054): access privileges of tables, views, sequences. $1 schema regex or NULL,
-- $2 name regex or NULL. Switch: hide_system.
SELECT n.nspname::text AS "Schema",
    c.relname::text AS "Name",
    CASE c.relkind
        WHEN 'r' THEN 'table'
        WHEN 'p' THEN 'partitioned table'
        WHEN 'v' THEN 'view'
        WHEN 'm' THEN 'materialized view'
        WHEN 'S' THEN 'sequence'
        WHEN 'f' THEN 'foreign table'
    END::text AS "Type",
    pg_catalog.array_to_string(c.relacl, E'\n')::text AS "Access privileges",
    pg_catalog.array_to_string(ARRAY(
        SELECT a.attname || ': ' || pg_catalog.array_to_string(a.attacl, ' ')
        FROM pg_catalog.pg_attribute a
        WHERE a.attrelid = c.oid AND a.attacl IS NOT NULL AND NOT a.attisdropped
        ORDER BY a.attnum), E'\n')::text AS "Column privileges"
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f')
    AND ($1::text IS NULL OR n.nspname ~ $1::text)
    AND ($2::text IS NULL OR c.relname ~ $2::text)
{% if hide_system %}
    AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
{% endif %}
ORDER BY 1, 2
