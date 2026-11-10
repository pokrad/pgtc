-- Object definition: collation (ICU locale column colliculocale in PostgreSQL 15-16).
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/collations/templates/collations/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Collation' AS kind, x.q AS name,
    'DROP COLLATION IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE COLLATION IF NOT EXISTS ' || x.q || E'\n    (' || pg_catalog.concat_ws(', ',
                CASE WHEN c.collcollate IS NOT NULL THEN 'LC_COLLATE = ' || pg_catalog.quote_literal(c.collcollate) END,
                CASE WHEN c.collctype IS NOT NULL THEN 'LC_CTYPE = ' || pg_catalog.quote_literal(c.collctype) END,
                CASE WHEN c.colliculocale IS NOT NULL THEN 'LOCALE = ' || pg_catalog.quote_literal(c.colliculocale) END,
                'PROVIDER = ' || CASE c.collprovider WHEN 'i' THEN 'icu' WHEN 'c' THEN 'libc' WHEN 'b' THEN 'builtin' ELSE 'default' END,
                CASE WHEN NOT c.collisdeterministic THEN 'DETERMINISTIC = false' END) || ');',
        'ALTER COLLATION ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(c.collowner)) || ';',
        'COMMENT ON COLLATION ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(c.oid, 'pg_collation')) || ';'
    ) AS body
FROM pg_catalog.pg_collation c
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = c.collnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(c.collname) AS q) x
WHERE c.oid = $1::oid
