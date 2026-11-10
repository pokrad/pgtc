-- Structure model (D-048): tables and partitions with their columns, constraints,
-- indexes, triggers, rules and policies. Only what is defined in the table itself is
-- read (columns, constraints, indexes and triggers a partition gets from its parent
-- are left out).
-- Rows: schema, kind ('table'), data (JSON of `Table`). Runs with search_path = ''.
{% import "macros/model.sql" as m %}
SELECT n.nspname AS schema, 'table' AS kind,
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', c.relname,
        'unlogged', c.relpersistence = 'u',
        'columns', (
            SELECT pg_catalog.json_agg(pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
                    'name', a.attname,
                    'type', pg_catalog.format_type(a.atttypid, a.atttypmod),
                    'not_null', a.attnotnull,
{% if has("GeneratedColumns") %}
                    'default', CASE WHEN a.attgenerated = '' THEN pg_catalog.pg_get_expr(ad.adbin, ad.adrelid) END,
                    'generated', CASE WHEN a.attgenerated <> '' THEN pg_catalog.pg_get_expr(ad.adbin, ad.adrelid) END,
                    'virtual', a.attgenerated = 'v',
{% else %}
                    'default', pg_catalog.pg_get_expr(ad.adbin, ad.adrelid),
{% endif %}
                    'identity', CASE a.attidentity WHEN 'a' THEN 'always' WHEN 'd' THEN 'by default' END,
                    'collation', CASE WHEN a.attcollation <> 0 AND a.attcollation <> ty.typcollation
                                      THEN {{ m.collation("co", "cn") }} END,
                    'comment', pg_catalog.col_description(a.attrelid, a.attnum)))
                ORDER BY a.attnum)
            FROM pg_catalog.pg_attribute a
                JOIN pg_catalog.pg_type ty ON ty.oid = a.atttypid
                LEFT JOIN pg_catalog.pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum
                LEFT JOIN pg_catalog.pg_collation co ON co.oid = a.attcollation
                LEFT JOIN pg_catalog.pg_namespace cn ON cn.oid = co.collnamespace
            WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped AND a.attislocal),
        'partition_by', CASE WHEN c.relkind = 'p' THEN pg_catalog.pg_get_partkeydef(c.oid) END,
        'partition_of', CASE WHEN c.relispartition THEN (
            SELECT pg_catalog.json_build_object(
                'parent', pg_catalog.json_build_object('schema', pn.nspname, 'name', pc.relname),
                'bound', pg_catalog.pg_get_expr(c.relpartbound, c.oid))
            FROM pg_catalog.pg_inherits ih
                JOIN pg_catalog.pg_class pc ON pc.oid = ih.inhparent
                JOIN pg_catalog.pg_namespace pn ON pn.oid = pc.relnamespace
            WHERE ih.inhrelid = c.oid) END,
        'options', pg_catalog.to_json(c.reloptions),
        'row_security', c.relrowsecurity,
        'force_row_security', c.relforcerowsecurity,
        'constraints', (
            SELECT pg_catalog.json_agg(pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
                    'name', k.conname,
                    'kind', CASE k.contype WHEN 'p' THEN 'primary_key' WHEN 'u' THEN 'unique'
                                           WHEN 'f' THEN 'foreign_key' WHEN 'c' THEN 'check'
                                           ELSE 'exclude' END,
                    'definition', pg_catalog.pg_get_constraintdef(k.oid),
                    'references', CASE WHEN k.contype = 'f' THEN pg_catalog.json_build_object(
                        'schema', fn.nspname, 'name', fc.relname) END,
                    'comment', pg_catalog.obj_description(k.oid, 'pg_constraint')))
                ORDER BY k.conname)
            FROM pg_catalog.pg_constraint k
                LEFT JOIN pg_catalog.pg_class fc ON fc.oid = k.confrelid
                LEFT JOIN pg_catalog.pg_namespace fn ON fn.oid = fc.relnamespace
            WHERE k.conrelid = c.oid AND k.contype IN ('p', 'u', 'f', 'c', 'x')
                AND k.conislocal AND k.conparentid = 0),
        'indexes', {{ m.indexes("c.oid") }},
        'triggers', {{ m.triggers("c.oid", has("TriggerParentId")) }},
        'rules', {{ m.rules("c.oid") }},
        'policies', (
            SELECT pg_catalog.json_agg(pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
                    'name', po.polname,
                    'restrictive', NOT po.polpermissive,
                    'command', CASE po.polcmd WHEN 'r' THEN 'SELECT' WHEN 'a' THEN 'INSERT'
                                              WHEN 'w' THEN 'UPDATE' WHEN 'd' THEN 'DELETE'
                                              ELSE 'ALL' END,
                    'roles', (SELECT pg_catalog.json_agg(ro.rolname ORDER BY ro.rolname)
                              FROM pg_catalog.pg_roles ro WHERE ro.oid = ANY (po.polroles)),
                    'using', pg_catalog.pg_get_expr(po.polqual, po.polrelid),
                    'with_check', pg_catalog.pg_get_expr(po.polwithcheck, po.polrelid)))
                ORDER BY po.polname)
            FROM pg_catalog.pg_policy po WHERE po.polrelid = c.oid),
        'comment', pg_catalog.obj_description(c.oid, 'pg_class')))::text AS data
FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r', 'p')
    AND {{ m.user_schema("n") }}
    AND {{ m.not_ext("pg_class", "c.oid") }}
