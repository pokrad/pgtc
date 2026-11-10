-- Object definition: index.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/tables/indexes/templates/indexes/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Index' AS kind, x.q AS name,
    'DROP INDEX IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        pg_catalog.pg_get_indexdef(rel.oid) || ';',
        CASE WHEN rel.reltablespace <> 0
             THEN 'ALTER INDEX IF EXISTS ' || x.q || ' SET TABLESPACE ' || pg_catalog.quote_ident(ts.spcname) || ';' END,
        'COMMENT ON INDEX ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(rel.oid, 'pg_class')) || ';'
    ) AS body
FROM pg_catalog.pg_class rel
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    LEFT JOIN pg_catalog.pg_tablespace ts ON ts.oid = rel.reltablespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE rel.oid = $1::oid
