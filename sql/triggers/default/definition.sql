-- Object definition: trigger.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/tables/triggers/templates/triggers/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Trigger' AS kind, pg_catalog.quote_ident(tg.tgname) AS name,
    'DROP TRIGGER IF EXISTS ' || pg_catalog.quote_ident(tg.tgname) || ' ON ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        pg_catalog.pg_get_triggerdef(tg.oid, true) || ';',
        CASE tg.tgenabled WHEN 'D' THEN 'ALTER TABLE IF EXISTS ' || x.q || ' DISABLE TRIGGER '
                                        || pg_catalog.quote_ident(tg.tgname) || ';' END,
        'COMMENT ON TRIGGER ' || pg_catalog.quote_ident(tg.tgname) || ' ON ' || x.q || E'\n    IS '
            || pg_catalog.quote_literal(pg_catalog.obj_description(tg.oid, 'pg_trigger')) || ';'
    ) AS body
FROM pg_catalog.pg_trigger tg
    JOIN pg_catalog.pg_class rel ON rel.oid = tg.tgrelid
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE tg.oid = $1::oid
