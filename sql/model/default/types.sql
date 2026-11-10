-- Structure model (D-048): enum, composite, domain and range types.
-- Rows: schema, kind ('type'), data (JSON of `Type`). Runs with search_path = ''.
{% import "macros/model.sql" as m %}
SELECT n.nspname AS schema, 'type' AS kind,
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', t.typname,
        'kind', CASE t.typtype WHEN 'e' THEN 'enum' WHEN 'c' THEN 'composite'
                               WHEN 'd' THEN 'domain' ELSE 'range' END,
        'labels', CASE WHEN t.typtype = 'e' THEN (
            SELECT pg_catalog.json_agg(e.enumlabel ORDER BY e.enumsortorder)
            FROM pg_catalog.pg_enum e WHERE e.enumtypid = t.oid) END,
        'attributes', CASE WHEN t.typtype = 'c' THEN (
            SELECT pg_catalog.json_agg(pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
                    'name', a.attname,
                    'type', pg_catalog.format_type(a.atttypid, a.atttypmod),
                    'collation', CASE WHEN a.attcollation <> 0 AND a.attcollation <> at.typcollation
                                      THEN {{ m.collation("co", "cn") }} END))
                ORDER BY a.attnum)
            FROM pg_catalog.pg_attribute a
                JOIN pg_catalog.pg_type at ON at.oid = a.atttypid
                LEFT JOIN pg_catalog.pg_collation co ON co.oid = a.attcollation
                LEFT JOIN pg_catalog.pg_namespace cn ON cn.oid = co.collnamespace
            WHERE a.attrelid = t.typrelid AND a.attnum > 0 AND NOT a.attisdropped) END,
        'base', CASE WHEN t.typtype = 'd' THEN pg_catalog.format_type(t.typbasetype, t.typtypmod) END,
        'not_null', CASE WHEN t.typtype = 'd' THEN t.typnotnull END,
        'default', t.typdefault,
        'collation', CASE WHEN t.typtype = 'd' AND t.typcollation <> 0 AND t.typcollation <> bt.typcollation
                          THEN {{ m.collation("tco", "tcn") }}
                          WHEN t.typtype = 'r' AND r.rngcollation <> 0 AND r.rngcollation <> st.typcollation
                          THEN {{ m.collation("rco", "rcn") }} END,
        'checks', CASE WHEN t.typtype = 'd' THEN (
            SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
                    'name', k.conname, 'definition', pg_catalog.pg_get_constraintdef(k.oid))
                ORDER BY k.conname)
            FROM pg_catalog.pg_constraint k WHERE k.contypid = t.oid AND k.contype = 'c') END,
        'subtype', CASE WHEN t.typtype = 'r' THEN pg_catalog.format_type(r.rngsubtype, NULL) END,
        'subtype_opclass', CASE WHEN r.rngsubopc IS NOT NULL AND NOT oc.opcdefault
                                THEN pg_catalog.quote_ident(ocn.nspname) || '.' || pg_catalog.quote_ident(oc.opcname) END,
        'canonical', CASE WHEN r.rngcanonical <> 0 THEN r.rngcanonical::pg_catalog.regproc::text END,
        'subtype_diff', CASE WHEN r.rngsubdiff <> 0 THEN r.rngsubdiff::pg_catalog.regproc::text END,
        'comment', pg_catalog.obj_description(t.oid, 'pg_type')))::text AS data
FROM pg_catalog.pg_type t
    JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace
    LEFT JOIN pg_catalog.pg_class rel ON rel.oid = t.typrelid
    LEFT JOIN pg_catalog.pg_type bt ON bt.oid = t.typbasetype
    LEFT JOIN pg_catalog.pg_collation tco ON tco.oid = t.typcollation
    LEFT JOIN pg_catalog.pg_namespace tcn ON tcn.oid = tco.collnamespace
    LEFT JOIN pg_catalog.pg_range r ON r.rngtypid = t.oid
    LEFT JOIN pg_catalog.pg_type st ON st.oid = r.rngsubtype
    LEFT JOIN pg_catalog.pg_opclass oc ON oc.oid = r.rngsubopc
    LEFT JOIN pg_catalog.pg_namespace ocn ON ocn.oid = oc.opcnamespace
    LEFT JOIN pg_catalog.pg_collation rco ON rco.oid = r.rngcollation
    LEFT JOIN pg_catalog.pg_namespace rcn ON rcn.oid = rco.collnamespace
WHERE {{ m.user_schema("n") }}
    AND {{ m.not_ext("pg_type", "t.oid") }}
    AND (t.typtype IN ('e', 'd', 'r') OR (t.typtype = 'c' AND rel.relkind = 'c'))
