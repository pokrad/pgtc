-- \dx (D-054): installed extensions. $1 name regex or NULL.
SELECT e.extname::text AS "Name",
    e.extversion::text AS "Version",
    n.nspname::text AS "Schema",
    pg_catalog.obj_description(e.oid, 'pg_extension')::text AS "Description"
FROM pg_catalog.pg_extension e
JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
WHERE $1::text IS NULL OR e.extname ~ $1::text
ORDER BY 1
