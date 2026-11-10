-- Structure model (D-048): views and materialized views.
-- Rows: schema, kind ('view' / 'materialized_view'), data (JSON of `View`).
-- Runs with search_path = ''.
{% import "macros/model.sql" as m %}
SELECT n.nspname AS schema,
    CASE c.relkind WHEN 'm' THEN 'materialized_view' ELSE 'view' END AS kind,
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', c.relname,
        'definition', pg_catalog.pg_get_viewdef(c.oid, true),
        'options', pg_catalog.to_json(c.reloptions),
        'indexes', CASE WHEN c.relkind = 'm' THEN {{ m.indexes("c.oid") }} END,
        'triggers', {{ m.triggers("c.oid", has("TriggerParentId")) }},
        'rules', {{ m.rules("c.oid") }},
        'comment', pg_catalog.obj_description(c.oid, 'pg_class')))::text AS data
FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('v', 'm')
    AND {{ m.user_schema("n") }}
    AND {{ m.not_ext("pg_class", "c.oid") }}
