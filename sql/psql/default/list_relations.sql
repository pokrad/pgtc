-- \d, \dt, \dv, \dm, \di, \ds (D-054): relations of the kinds in $3.
-- $1 schema regex or NULL, $2 name regex or NULL, $3 relkinds (e.g. 'rp' = tables).
-- Switches: plus (\d+), hide_system (no pattern, no S), indexes (\di: table column).
SELECT n.nspname::text AS "Schema",
    c.relname::text AS "Name",
    CASE c.relkind
        WHEN 'r' THEN 'table'
        WHEN 'p' THEN 'partitioned table'
        WHEN 'v' THEN 'view'
        WHEN 'm' THEN 'materialized view'
        WHEN 'i' THEN 'index'
        WHEN 'I' THEN 'partitioned index'
        WHEN 'S' THEN 'sequence'
        WHEN 'f' THEN 'foreign table'
    END::text AS "Type",
    pg_catalog.pg_get_userbyid(c.relowner)::text AS "Owner"
{% if indexes %}
    , t.relname::text AS "Table"
{% endif %}
{% if plus %}
    , pg_catalog.pg_size_pretty(pg_catalog.pg_table_size(c.oid))::text AS "Size",
    pg_catalog.obj_description(c.oid, 'pg_class')::text AS "Description"
{% endif %}
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_index x ON x.indexrelid = c.oid
LEFT JOIN pg_catalog.pg_class t ON t.oid = x.indrelid
WHERE pg_catalog.strpos($3::text, c.relkind::text) > 0
    AND ($1::text IS NULL OR n.nspname ~ $1::text)
    AND ($2::text IS NULL OR c.relname ~ $2::text)
    AND n.nspname !~ '^pg_toast'
{% if hide_system %}
    AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
{% endif %}
ORDER BY 1, 2
