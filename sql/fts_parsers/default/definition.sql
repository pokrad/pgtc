-- Object definition: text search parser.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/fts_parsers/templates/fts_parsers/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'FTS Parser' AS kind, x.q AS name,
    'DROP TEXT SEARCH PARSER IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE TEXT SEARCH PARSER ' || x.q || E' (\n    ' || pg_catalog.concat_ws(E',\n    ',
            'START = ' || p.prsstart::pg_catalog.regproc::text,
            'GETTOKEN = ' || p.prstoken::pg_catalog.regproc::text,
            'END = ' || p.prsend::pg_catalog.regproc::text,
            'LEXTYPES = ' || p.prslextype::pg_catalog.regproc::text,
            CASE WHEN p.prsheadline <> 0 THEN 'HEADLINE = ' || p.prsheadline::pg_catalog.regproc::text END) || E'\n);',
        'COMMENT ON TEXT SEARCH PARSER ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(p.oid, 'pg_ts_parser')) || ';'
    ) AS body
FROM pg_catalog.pg_ts_parser p
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = p.prsnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(p.prsname) AS q) x
WHERE p.oid = $1::oid
