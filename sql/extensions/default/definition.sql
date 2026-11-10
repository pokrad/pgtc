-- Object definition: extension.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/extensions/templates/extensions/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Extension' AS kind, e.extname::text AS name,
    'DROP EXTENSION IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE EXTENSION IF NOT EXISTS ' || x.q
            || E'\n    SCHEMA ' || pg_catalog.quote_ident(n.nspname)
            || E'\n    VERSION ' || pg_catalog.quote_literal(e.extversion) || ';',
        'COMMENT ON EXTENSION ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(e.oid, 'pg_extension')) || ';'
    ) AS body
FROM pg_catalog.pg_extension e
    JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(e.extname) AS q) x
WHERE e.oid = $1::oid
