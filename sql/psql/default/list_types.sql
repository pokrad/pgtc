-- \dT (D-054): data types (no array types, no table row types). $1 schema regex or
-- NULL, $2 name regex or NULL. Switches: plus, hide_system.
SELECT n.nspname::text AS "Schema",
    pg_catalog.format_type(t.oid, NULL)::text AS "Name",
    pg_catalog.obj_description(t.oid, 'pg_type')::text AS "Description"
{% if plus %}
    , t.typname::text AS "Internal name",
    CASE t.typtype WHEN 'b' THEN 'base' WHEN 'c' THEN 'composite' WHEN 'd' THEN 'domain'
        WHEN 'e' THEN 'enum' WHEN 'p' THEN 'pseudo' WHEN 'r' THEN 'range'
        WHEN 'm' THEN 'multirange' END::text AS "Kind",
    pg_catalog.pg_get_userbyid(t.typowner)::text AS "Owner"
{% endif %}
FROM pg_catalog.pg_type t
JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace
WHERE (t.typrelid = 0
        OR (SELECT c.relkind FROM pg_catalog.pg_class c WHERE c.oid = t.typrelid) = 'c')
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_type e
        WHERE e.oid = t.typelem AND e.typarray = t.oid)
    AND ($1::text IS NULL OR n.nspname ~ $1::text)
    AND ($2::text IS NULL OR t.typname ~ $2::text
        OR pg_catalog.format_type(t.oid, NULL) ~ $2::text)
{% if hide_system %}
    AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
{% endif %}
ORDER BY 1, 2
