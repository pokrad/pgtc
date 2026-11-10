-- Structure model (D-048): extensions and schemas.
-- Rows: schema (text), kind ('extension' / 'schema'), data (JSON of the model struct).
-- Runs with search_path = '' so names in the output are qualified.
{% import "macros/model.sql" as m %}
SELECT '' AS schema, 'extension' AS kind,
    pg_catalog.json_build_object('name', e.extname, 'schema', n.nspname)::text AS data
FROM pg_catalog.pg_extension e
    JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
UNION ALL
SELECT n.nspname, 'schema',
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', n.nspname,
        'comment', pg_catalog.obj_description(n.oid, 'pg_namespace')))::text
FROM pg_catalog.pg_namespace n
WHERE {{ m.user_schema("n") }}
