-- Object definition: text search template.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/fts_templates/templates/fts_templates/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'FTS Template' AS kind, x.q AS name,
    'DROP TEXT SEARCH TEMPLATE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE TEXT SEARCH TEMPLATE ' || x.q || E' (\n    ' || pg_catalog.concat_ws(E',\n    ',
            CASE WHEN tm.tmplinit <> 0 THEN 'INIT = ' || tm.tmplinit::pg_catalog.regproc::text END,
            'LEXIZE = ' || tm.tmpllexize::pg_catalog.regproc::text) || E'\n);',
        'COMMENT ON TEXT SEARCH TEMPLATE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(tm.oid, 'pg_ts_template')) || ';'
    ) AS body
FROM pg_catalog.pg_ts_template tm
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = tm.tmplnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(tm.tmplname) AS q) x
WHERE tm.oid = $1::oid
