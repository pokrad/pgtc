-- Object definition: column.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/tables/columns/templates/columns/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = column number (as oid), $2 = table oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Column' AS kind, x.q || '.' || pg_catalog.quote_ident(a.attname) AS name,
    'ALTER TABLE IF EXISTS ' || x.q || ' DROP COLUMN IF EXISTS ' || pg_catalog.quote_ident(a.attname) || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'ALTER TABLE IF EXISTS ' || x.q || E'\n    ADD COLUMN ' || {{ col.coldef(has("GeneratedColumns")) }} || ';',
        'COMMENT ON COLUMN ' || x.q || '.' || pg_catalog.quote_ident(a.attname) || E'\n    IS '
            || pg_catalog.quote_literal(pg_catalog.col_description(a.attrelid, a.attnum)) || ';'
    ) AS body
FROM pg_catalog.pg_attribute a
    {{ col.coljoins() }}
    JOIN pg_catalog.pg_class rel ON rel.oid = a.attrelid
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE a.attrelid = $2::oid AND a.attnum = $1::oid::int8
