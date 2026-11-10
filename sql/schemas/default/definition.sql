-- Object definition: schema.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/templates/schemas/pg/default/sql/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Schema' AS kind, nsp.nspname::text AS name,
    'DROP SCHEMA IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE SCHEMA IF NOT EXISTS ' || x.q || E'\n    AUTHORIZATION ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(nsp.nspowner)) || ';',
        'COMMENT ON SCHEMA ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(nsp.oid, 'pg_namespace')) || ';',
        {{ acl.grants("nsp.nspacl", "'SCHEMA'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_namespace nsp
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) AS q) x
WHERE nsp.oid = $1::oid
