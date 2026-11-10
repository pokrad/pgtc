-- Object definition: row level security policy.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/tables/row_security_policies/templates/row_security_policies/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Policy' AS kind, pg_catalog.quote_ident(pol.polname) AS name,
    'DROP POLICY IF EXISTS ' || pg_catalog.quote_ident(pol.polname) || ' ON ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        {{ parts.policy("pol", "x.q") }},
        'COMMENT ON POLICY ' || pg_catalog.quote_ident(pol.polname) || ' ON ' || x.q || E'\n    IS '
            || pg_catalog.quote_literal(pg_catalog.obj_description(pol.oid, 'pg_policy')) || ';'
    ) AS body
FROM pg_catalog.pg_policy pol
    JOIN pg_catalog.pg_class rel ON rel.oid = pol.polrelid
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE pol.oid = $1::oid
