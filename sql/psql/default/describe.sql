-- \d name (D-054): the relation itself; no row = not found.
-- $1 schema or NULL (search path), $2 name (macros/psql.sql).
{% import "macros/psql.sql" as m %}
{{ m.target() }}
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
    pg_catalog.pg_get_userbyid(c.relowner)::text AS "Owner",
    pg_catalog.obj_description(c.oid, 'pg_class')::text AS "Description"
FROM t
JOIN pg_catalog.pg_class c ON c.oid = t.oid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
