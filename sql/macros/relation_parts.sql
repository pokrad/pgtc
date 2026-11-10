-- Macros: indexes, triggers, rules and policies of a relation (shown below its CREATE).
{% macro indexes(oid, nsp) -%}
(SELECT pg_catalog.string_agg('-- Index: ' || pg_catalog.quote_ident(ic.relname)
        || E'\n\n-- DROP INDEX IF EXISTS ' || pg_catalog.quote_ident({{ nsp }}) || '.' || pg_catalog.quote_ident(ic.relname)
        || E';\n\n' || pg_catalog.pg_get_indexdef(i.indexrelid) || ';', E'\n\n' ORDER BY ic.relname)
    FROM pg_catalog.pg_index i
        JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
    WHERE i.indrelid = {{ oid }}
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint c
                        WHERE c.conindid = i.indexrelid AND c.conrelid = {{ oid }}
                            AND c.contype IN ('p', 'u', 'x')))
{%- endmacro %}
{% macro triggers(oid, q) -%}
(SELECT pg_catalog.string_agg('-- Trigger: ' || pg_catalog.quote_ident(tg.tgname)
        || E'\n\n-- DROP TRIGGER IF EXISTS ' || pg_catalog.quote_ident(tg.tgname) || ' ON ' || {{ q }}
        || E';\n\n' || pg_catalog.pg_get_triggerdef(tg.oid, true) || ';'
        || CASE tg.tgenabled WHEN 'D' THEN E'\n\nALTER TABLE IF EXISTS ' || {{ q }}
                                           || ' DISABLE TRIGGER ' || pg_catalog.quote_ident(tg.tgname) || ';'
                             ELSE '' END, E'\n\n' ORDER BY tg.tgname)
    FROM pg_catalog.pg_trigger tg
    WHERE tg.tgrelid = {{ oid }} AND NOT tg.tgisinternal)
{%- endmacro %}
{% macro rules(oid, q) -%}
(SELECT pg_catalog.string_agg('-- Rule: ' || pg_catalog.quote_ident(rw.rulename)
        || E'\n\n-- DROP RULE IF EXISTS ' || pg_catalog.quote_ident(rw.rulename) || ' ON ' || {{ q }}
        || E';\n\n' || pg_catalog.pg_get_ruledef(rw.oid, true), E'\n\n' ORDER BY rw.rulename)
    FROM pg_catalog.pg_rewrite rw
    WHERE rw.ev_class = {{ oid }} AND rw.rulename <> '_RETURN')
{%- endmacro %}
{% macro policy(pol, q) -%}
'CREATE POLICY ' || pg_catalog.quote_ident({{ pol }}.polname) || E'\n    ON ' || {{ q }}
    || E'\n    AS ' || CASE WHEN {{ pol }}.polpermissive THEN 'PERMISSIVE' ELSE 'RESTRICTIVE' END
    || E'\n    FOR ' || CASE {{ pol }}.polcmd WHEN 'r' THEN 'SELECT' WHEN 'a' THEN 'INSERT'
                                            WHEN 'w' THEN 'UPDATE' WHEN 'd' THEN 'DELETE' ELSE 'ALL' END
    || E'\n    TO ' || (SELECT pg_catalog.string_agg(CASE WHEN r = 0 THEN 'public'
                                     ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(r)) END, ', ')
                        FROM pg_catalog.unnest({{ pol }}.polroles) r)
    || COALESCE(E'\n    USING (' || pg_catalog.pg_get_expr({{ pol }}.polqual, {{ pol }}.polrelid) || ')', '')
    || COALESCE(E'\n    WITH CHECK (' || pg_catalog.pg_get_expr({{ pol }}.polwithcheck, {{ pol }}.polrelid) || ')', '')
    || ';'
{%- endmacro %}
{% macro policies(oid, q) -%}
(SELECT pg_catalog.string_agg('-- Policy: ' || pg_catalog.quote_ident(p.polname)
        || E'\n\n-- DROP POLICY IF EXISTS ' || pg_catalog.quote_ident(p.polname) || ' ON ' || {{ q }}
        || E';\n\n' || {{ policy("p", q) }}, E'\n\n' ORDER BY p.polname)
    FROM pg_catalog.pg_policy p
    WHERE p.polrelid = {{ oid }})
{%- endmacro %}
