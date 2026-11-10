-- \d name (D-054): triggers (not internal ones). $1 schema or NULL, $2 name.
{% import "macros/psql.sql" as m %}
{{ m.target() }}
SELECT tg.tgname::text AS "Name",
    pg_catalog.pg_get_triggerdef(tg.oid, true)::text AS "Definition"
FROM t
JOIN pg_catalog.pg_trigger tg ON tg.tgrelid = t.oid AND NOT tg.tgisinternal
ORDER BY 1
