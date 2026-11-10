-- \db (D-054): tablespaces. $1 name regex or NULL. Switch: plus.
SELECT t.spcname::text AS "Name",
    pg_catalog.pg_get_userbyid(t.spcowner)::text AS "Owner",
    pg_catalog.pg_tablespace_location(t.oid)::text AS "Location"
{% if plus %}
    , pg_catalog.array_to_string(t.spcacl, E'\n')::text AS "Access privileges",
    pg_catalog.array_to_string(t.spcoptions, ', ')::text AS "Options",
    pg_catalog.pg_size_pretty(pg_catalog.pg_tablespace_size(t.oid))::text AS "Size",
    pg_catalog.shobj_description(t.oid, 'pg_tablespace')::text AS "Description"
{% endif %}
FROM pg_catalog.pg_tablespace t
WHERE $1::text IS NULL OR t.spcname ~ $1::text
ORDER BY 1
