-- Macros for the structure model queries (D-048).
-- user_schema(n): `n` (pg_namespace) is a user schema: no system or temporary schema,
-- not created by an extension.
{% macro user_schema(n) -%}
{{ n }}.nspname NOT IN ('pg_catalog', 'information_schema')
    AND {{ n }}.nspname NOT LIKE 'pg\_toast%'
    AND {{ n }}.nspname NOT LIKE 'pg\_temp\_%'
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend xd
        WHERE xd.classid = 'pg_catalog.pg_namespace'::pg_catalog.regclass
            AND xd.objid = {{ n }}.oid AND xd.deptype = 'e')
{%- endmacro %}
-- not_ext(catalog, oid): the object is not a member of an extension.
{% macro not_ext(catalog, oid) -%}
NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend xe
    WHERE xe.classid = 'pg_catalog.{{ catalog }}'::pg_catalog.regclass
        AND xe.objid = {{ oid }} AND xe.deptype = 'e')
{%- endmacro %}
-- collation(c, n): qualified collation name of pg_collation `c` in pg_namespace `n`.
{% macro collation(c, n) -%}
pg_catalog.quote_ident({{ n }}.nspname) || '.' || pg_catalog.quote_ident({{ c }}.collname)
{%- endmacro %}
-- indexes(oid): JSON array of the relation's indexes that belong to no constraint and
-- are not partitions of a parent index.
{% macro indexes(oid) -%}
(SELECT pg_catalog.json_agg(pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', ic.relname,
        'definition', pg_catalog.pg_get_indexdef(i.indexrelid),
        'comment', pg_catalog.obj_description(i.indexrelid, 'pg_class'))) ORDER BY ic.relname)
    FROM pg_catalog.pg_index i
        JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
    WHERE i.indrelid = {{ oid }}
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k
            WHERE k.conindid = i.indexrelid AND k.contype IN ('p', 'u', 'x'))
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_inherits ih WHERE ih.inhrelid = i.indexrelid))
{%- endmacro %}
-- triggers(oid, parent_id): JSON array of the relation's own triggers (no internal or
-- cloned ones); parent_id = has("TriggerParentId") (macros do not see the context).
{% macro triggers(oid, parent_id) -%}
(SELECT pg_catalog.json_agg(pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', tg.tgname,
        'definition', pg_catalog.pg_get_triggerdef(tg.oid),
        'comment', pg_catalog.obj_description(tg.oid, 'pg_trigger'))) ORDER BY tg.tgname)
    FROM pg_catalog.pg_trigger tg
    WHERE tg.tgrelid = {{ oid }} AND NOT tg.tgisinternal
{% if parent_id %}
        AND tg.tgparentid = 0
{% endif %}
    )
{%- endmacro %}
-- rules(oid): JSON array of the relation's rules (not the view's _RETURN rule).
{% macro rules(oid) -%}
(SELECT pg_catalog.json_agg(pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', rw.rulename,
        'definition', pg_catalog.pg_get_ruledef(rw.oid),
        'comment', pg_catalog.obj_description(rw.oid, 'pg_rewrite'))) ORDER BY rw.rulename)
    FROM pg_catalog.pg_rewrite rw
    WHERE rw.ev_class = {{ oid }} AND rw.rulename <> '_RETURN')
{%- endmacro %}
