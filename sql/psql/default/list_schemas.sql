-- \dn (D-054): schemas. $1 name regex or NULL. Switches: plus, hide_system.
SELECT n.nspname::text AS "Name",
    pg_catalog.pg_get_userbyid(n.nspowner)::text AS "Owner"
{% if plus %}
    , pg_catalog.array_to_string(n.nspacl, E'\n')::text AS "Access privileges",
    pg_catalog.obj_description(n.oid, 'pg_namespace')::text AS "Description"
{% endif %}
FROM pg_catalog.pg_namespace n
WHERE ($1::text IS NULL OR n.nspname ~ $1::text)
{% if hide_system %}
    AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
{% endif %}
ORDER BY 1
