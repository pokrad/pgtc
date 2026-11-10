-- Object definition: table constraint.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/tables/constraints/templates/*/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT CASE c.contype WHEN 'p' THEN 'Primary Key' WHEN 'u' THEN 'Unique Constraint' WHEN 'f' THEN 'Foreign Key'
                      WHEN 'c' THEN 'Check Constraint' WHEN 'x' THEN 'Exclusion Constraint' ELSE 'Constraint' END AS kind,
    pg_catalog.quote_ident(c.conname) AS name,
    'ALTER TABLE IF EXISTS ' || x.q || ' DROP CONSTRAINT IF EXISTS ' || pg_catalog.quote_ident(c.conname) || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'ALTER TABLE IF EXISTS ' || x.q || E'\n    ADD CONSTRAINT ' || pg_catalog.quote_ident(c.conname) || ' '
            || pg_catalog.pg_get_constraintdef(c.oid, true) || ';',
        'COMMENT ON CONSTRAINT ' || pg_catalog.quote_ident(c.conname) || ' ON ' || x.q || E'\n    IS '
            || pg_catalog.quote_literal(pg_catalog.obj_description(c.oid, 'pg_constraint')) || ';'
    ) AS body
FROM pg_catalog.pg_constraint c
    JOIN pg_catalog.pg_class rel ON rel.oid = c.conrelid
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE c.oid = $1::oid
