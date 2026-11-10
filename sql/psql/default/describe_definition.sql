-- \d name (D-054): the query of a view or materialized view. $1 schema or NULL,
-- $2 name.
{% import "macros/psql.sql" as m %}
{{ m.target() }}
SELECT pg_catalog.pg_get_viewdef(t.oid, true)::text AS "Definition"
FROM t
WHERE t.relkind IN ('v', 'm')
