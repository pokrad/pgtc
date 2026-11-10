-- \d name (D-054): constraints. $1 schema or NULL, $2 name.
{% import "macros/psql.sql" as m %}
{{ m.target() }}
SELECT r.conname::text AS "Name",
    CASE r.contype WHEN 'p' THEN 'primary key' WHEN 'u' THEN 'unique'
        WHEN 'f' THEN 'foreign key' WHEN 'c' THEN 'check' WHEN 'x' THEN 'exclusion'
        WHEN 't' THEN 'trigger' END::text AS "Type",
    pg_catalog.pg_get_constraintdef(r.oid, true)::text AS "Definition"
FROM t
JOIN pg_catalog.pg_constraint r ON r.conrelid = t.oid
ORDER BY 2, 1
