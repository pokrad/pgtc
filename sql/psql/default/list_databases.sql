-- \l (D-054): databases. $1 name regex or NULL. Switch: plus.
SELECT d.datname::text AS "Name",
    pg_catalog.pg_get_userbyid(d.datdba)::text AS "Owner",
    pg_catalog.pg_encoding_to_char(d.encoding)::text AS "Encoding",
    d.datcollate::text AS "Collate",
    d.datctype::text AS "Ctype",
    pg_catalog.array_to_string(d.datacl, E'\n')::text AS "Access privileges"
{% if plus %}
    , CASE WHEN pg_catalog.has_database_privilege(d.oid, 'CONNECT')
        THEN pg_catalog.pg_size_pretty(pg_catalog.pg_database_size(d.oid))
        ELSE 'No Access'
    END::text AS "Size",
    t.spcname::text AS "Tablespace",
    pg_catalog.shobj_description(d.oid, 'pg_database')::text AS "Description"
{% endif %}
FROM pg_catalog.pg_database d
JOIN pg_catalog.pg_tablespace t ON t.oid = d.dattablespace
WHERE $1::text IS NULL OR d.datname ~ $1::text
ORDER BY 1
