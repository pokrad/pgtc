-- Structure model (D-048): functions and procedures (aggregates are not in the model).
-- Rows: schema, kind ('function'), data (JSON of `Function`). Runs with search_path = ''.
{% import "macros/model.sql" as m %}
SELECT n.nspname AS schema, 'function' AS kind,
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', p.proname,
        'args', pg_catalog.pg_get_function_identity_arguments(p.oid),
        'procedure', p.prokind = 'p',
        'result', CASE WHEN p.prokind <> 'p' THEN pg_catalog.pg_get_function_result(p.oid) END,
        'definition', pg_catalog.pg_get_functiondef(p.oid),
        'comment', pg_catalog.obj_description(p.oid, 'pg_proc')))::text AS data
FROM pg_catalog.pg_proc p
    JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
WHERE p.prokind IN ('f', 'p', 'w')
    AND {{ m.user_schema("n") }}
    AND {{ m.not_ext("pg_proc", "p.oid") }}
    -- constructors created with a range type belong to it
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend xi
        WHERE xi.classid = 'pg_catalog.pg_proc'::pg_catalog.regclass
            AND xi.objid = p.oid AND xi.deptype = 'i')
