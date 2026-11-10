-- Structure model (D-048): sequences (identity sequences belong to their column).
-- Rows: schema, kind ('sequence'), data (JSON of `Sequence`). Runs with search_path = ''.
{% import "macros/model.sql" as m %}
SELECT n.nspname AS schema, 'sequence' AS kind,
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', c.relname,
        'type', pg_catalog.format_type(s.seqtypid, NULL),
        'start', s.seqstart,
        'increment', s.seqincrement,
        'min_value', s.seqmin,
        'max_value', s.seqmax,
        'cache', s.seqcache,
        'cycle', s.seqcycle,
        'owned_by', (
            SELECT pg_catalog.json_build_object('schema', tn.nspname, 'table', tc.relname, 'column', a.attname)
            FROM pg_catalog.pg_depend d
                JOIN pg_catalog.pg_class tc ON tc.oid = d.refobjid
                JOIN pg_catalog.pg_namespace tn ON tn.oid = tc.relnamespace
                JOIN pg_catalog.pg_attribute a ON a.attrelid = tc.oid AND a.attnum = d.refobjsubid
            WHERE d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass AND d.objid = c.oid
                AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass AND d.deptype = 'a'
            LIMIT 1),
        'comment', pg_catalog.obj_description(c.oid, 'pg_class')))::text AS data
FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_sequence s ON s.seqrelid = c.oid
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind = 'S'
    AND {{ m.user_schema("n") }}
    AND {{ m.not_ext("pg_class", "c.oid") }}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d
        WHERE d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass AND d.objid = c.oid
            AND d.deptype = 'i')
