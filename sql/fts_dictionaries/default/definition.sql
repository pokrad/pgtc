-- Object definition: text search dictionary.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/fts_dictionaries/templates/fts_dictionaries/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'FTS Dictionary' AS kind, x.q AS name,
    'DROP TEXT SEARCH DICTIONARY IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE TEXT SEARCH DICTIONARY ' || x.q || E' (\n    TEMPLATE = '
            || (SELECT pg_catalog.quote_ident(tn.nspname) || '.' || pg_catalog.quote_ident(tm.tmplname)
                FROM pg_catalog.pg_ts_template tm JOIN pg_catalog.pg_namespace tn ON tn.oid = tm.tmplnamespace
                WHERE tm.oid = d.dicttemplate)
            || COALESCE(E',\n    ' || d.dictinitoption, '') || E'\n);',
        'ALTER TEXT SEARCH DICTIONARY ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(d.dictowner)) || ';',
        'COMMENT ON TEXT SEARCH DICTIONARY ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(d.oid, 'pg_ts_dict')) || ';'
    ) AS body
FROM pg_catalog.pg_ts_dict d
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = d.dictnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(d.dictname) AS q) x
WHERE d.oid = $1::oid
