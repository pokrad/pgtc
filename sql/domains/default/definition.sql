-- Object definition: domain.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/domains/templates/domains/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Domain' AS kind, x.q AS name,
    'DROP DOMAIN IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE DOMAIN ' || x.q || E'\n    AS ' || pg_catalog.format_type(t.typbasetype, t.typtypmod)
            || CASE WHEN t.typcollation <> 0 AND t.typcollation <> bt.typcollation
                    THEN E'\n    COLLATE ' || pg_catalog.quote_ident(cn.nspname) || '.' || pg_catalog.quote_ident(co.collname)
                    ELSE '' END
            || CASE WHEN t.typdefault IS NOT NULL THEN E'\n    DEFAULT ' || t.typdefault ELSE '' END
            || CASE WHEN t.typnotnull THEN E'\n    NOT NULL' ELSE '' END
            || COALESCE((SELECT pg_catalog.string_agg(E'\n    CONSTRAINT ' || pg_catalog.quote_ident(c.conname) || ' '
                                    || pg_catalog.pg_get_constraintdef(c.oid, true), '' ORDER BY c.conname)
                         FROM pg_catalog.pg_constraint c WHERE c.contypid = t.oid AND c.contype = 'c'), '')
            || ';',
        'ALTER DOMAIN ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(t.typowner)) || ';',
        'COMMENT ON DOMAIN ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(t.oid, 'pg_type')) || ';',
        {{ acl.grants("t.typacl", "'DOMAIN'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_type t
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = t.typnamespace
    JOIN pg_catalog.pg_type bt ON bt.oid = t.typbasetype
    LEFT JOIN pg_catalog.pg_collation co ON co.oid = t.typcollation
    LEFT JOIN pg_catalog.pg_namespace cn ON cn.oid = co.collnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(t.typname) AS q) x
WHERE t.oid = $1::oid
