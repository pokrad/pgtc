-- \d name (D-054): indexes. $1 schema or NULL, $2 name.
{% import "macros/psql.sql" as m %}
{{ m.target() }}
SELECT i.relname::text AS "Name",
    CASE WHEN x.indisprimary THEN 'primary key' WHEN x.indisunique THEN 'unique'
    END::text AS "Kind",
    pg_catalog.pg_get_indexdef(x.indexrelid, 0, true)::text AS "Definition"
FROM t
JOIN pg_catalog.pg_index x ON x.indrelid = t.oid
JOIN pg_catalog.pg_class i ON i.oid = x.indexrelid
ORDER BY x.indisprimary DESC, 1
