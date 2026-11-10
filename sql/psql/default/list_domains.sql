-- \dD (D-054): domains. $1 schema regex or NULL, $2 name regex or NULL.
-- Switches: plus, hide_system.
SELECT n.nspname::text AS "Schema",
    t.typname::text AS "Name",
    pg_catalog.format_type(t.typbasetype, t.typtypmod)::text AS "Type",
    CASE WHEN t.typnotnull THEN 'not null' END::text AS "Nullable",
    t.typdefault::text AS "Default",
    (SELECT pg_catalog.string_agg(pg_catalog.pg_get_constraintdef(r.oid, true), ' ')
        FROM pg_catalog.pg_constraint r WHERE r.contypid = t.oid)::text AS "Check"
{% if plus %}
    , pg_catalog.obj_description(t.oid, 'pg_type')::text AS "Description"
{% endif %}
FROM pg_catalog.pg_type t
JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace
WHERE t.typtype = 'd'
    AND ($1::text IS NULL OR n.nspname ~ $1::text)
    AND ($2::text IS NULL OR t.typname ~ $2::text)
{% if hide_system %}
    AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
{% endif %}
ORDER BY 1, 2
