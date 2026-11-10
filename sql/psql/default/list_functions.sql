-- \df (D-054): functions, procedures, aggregates. $1 schema regex or NULL, $2 name
-- regex or NULL. Switches: plus, hide_system.
SELECT n.nspname::text AS "Schema",
    p.proname::text AS "Name",
    pg_catalog.pg_get_function_result(p.oid)::text AS "Result data type",
    pg_catalog.pg_get_function_arguments(p.oid)::text AS "Argument data types",
    CASE p.prokind WHEN 'a' THEN 'agg' WHEN 'w' THEN 'window' WHEN 'p' THEN 'proc'
        ELSE 'func' END::text AS "Type"
{% if plus %}
    , l.lanname::text AS "Language",
    pg_catalog.pg_get_userbyid(p.proowner)::text AS "Owner",
    pg_catalog.obj_description(p.oid, 'pg_proc')::text AS "Description"
{% endif %}
FROM pg_catalog.pg_proc p
JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
LEFT JOIN pg_catalog.pg_language l ON l.oid = p.prolang
WHERE ($1::text IS NULL OR n.nspname ~ $1::text)
    AND ($2::text IS NULL OR p.proname ~ $2::text)
{% if hide_system %}
    AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
{% endif %}
ORDER BY 1, 2, 4
