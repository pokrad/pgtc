-- Object definition: rule.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/tables/rules/templates/rules/sql/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Rule' AS kind, pg_catalog.quote_ident(rw.rulename) AS name,
    'DROP RULE IF EXISTS ' || pg_catalog.quote_ident(rw.rulename) || ' ON ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        pg_catalog.pg_get_ruledef(rw.oid, true),
        'COMMENT ON RULE ' || pg_catalog.quote_ident(rw.rulename) || ' ON ' || x.q || E'\n    IS '
            || pg_catalog.quote_literal(pg_catalog.obj_description(rw.oid, 'pg_rewrite')) || ';'
    ) AS body
FROM pg_catalog.pg_rewrite rw
    JOIN pg_catalog.pg_class rel ON rel.oid = rw.ev_class
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE rw.oid = $1::oid
