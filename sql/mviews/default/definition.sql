-- Object definition: materialized view, with its indexes.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/views/templates/mviews/pg/default/sql/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Materialized View' AS kind, x.q AS name,
    'DROP MATERIALIZED VIEW IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE MATERIALIZED VIEW IF NOT EXISTS ' || x.q
            || CASE WHEN rel.reloptions IS NOT NULL
                    THEN E'\nWITH (' || pg_catalog.array_to_string(rel.reloptions, ', ') || ')' ELSE '' END
            || E'\nTABLESPACE ' || COALESCE(pg_catalog.quote_ident(ts.spcname), 'pg_default')
            || E'\nAS\n' || pg_catalog.regexp_replace(pg_catalog.pg_get_viewdef(rel.oid, true), ';\s*$', '')
            || E'\nWITH ' || CASE WHEN rel.relispopulated THEN 'DATA' ELSE 'NO DATA' END || ';',
        'ALTER TABLE IF EXISTS ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(rel.relowner)) || ';',
        'COMMENT ON MATERIALIZED VIEW ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(rel.oid, 'pg_class')) || ';',
        (SELECT pg_catalog.string_agg('COMMENT ON COLUMN ' || x.q || '.' || pg_catalog.quote_ident(a.attname)
                || E'\n    IS ' || pg_catalog.quote_literal(d.description) || ';', E'\n\n' ORDER BY a.attnum)
            FROM pg_catalog.pg_attribute a
                JOIN pg_catalog.pg_description d ON d.objoid = a.attrelid
                    AND d.classoid = 'pg_catalog.pg_class'::regclass AND d.objsubid = a.attnum
            WHERE a.attrelid = rel.oid AND a.attnum > 0 AND NOT a.attisdropped),
        {{ acl.grants("rel.relacl", "'TABLE'", "x.q") }},
        {{ parts.indexes("rel.oid", "nsp.nspname") }}
    ) AS body
FROM pg_catalog.pg_class rel
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    LEFT JOIN pg_catalog.pg_tablespace ts ON ts.oid = rel.reltablespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE rel.oid = $1::oid
