-- Object definition: foreign table.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/foreign_tables/templates/foreign_tables/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Foreign Table' AS kind, x.q AS name,
    'DROP FOREIGN TABLE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE FOREIGN TABLE IF NOT EXISTS ' || x.q || E'\n(\n'
            || COALESCE((SELECT pg_catalog.string_agg('    ' || {{ col.coldef(has("GeneratedColumns")) }}, E',\n' ORDER BY a.attnum)
                         FROM pg_catalog.pg_attribute a
                             {{ col.coljoins() }}
                         WHERE a.attrelid = rel.oid AND a.attnum > 0 AND NOT a.attisdropped), '')
            || E'\n)\n    SERVER ' || pg_catalog.quote_ident(srv.srvname) || COALESCE(E'\n    OPTIONS (' || (SELECT pg_catalog.string_agg(pg_catalog.quote_ident(o.option_name) || ' ' || pg_catalog.quote_literal(o.option_value), ', ') FROM pg_catalog.pg_options_to_table(ft.ftoptions) o) || ')', '') || ';',
        'ALTER FOREIGN TABLE IF EXISTS ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(rel.relowner)) || ';',
        'COMMENT ON FOREIGN TABLE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(rel.oid, 'pg_class')) || ';',
        (SELECT pg_catalog.string_agg('COMMENT ON COLUMN ' || x.q || '.' || pg_catalog.quote_ident(a.attname)
                || E'\n    IS ' || pg_catalog.quote_literal(d.description) || ';', E'\n\n' ORDER BY a.attnum)
            FROM pg_catalog.pg_attribute a
                JOIN pg_catalog.pg_description d ON d.objoid = a.attrelid
                    AND d.classoid = 'pg_catalog.pg_class'::regclass AND d.objsubid = a.attnum
            WHERE a.attrelid = rel.oid AND a.attnum > 0 AND NOT a.attisdropped),
        {{ acl.grants("rel.relacl", "'TABLE'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_class rel
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    JOIN pg_catalog.pg_foreign_table ft ON ft.ftrelid = rel.oid
    JOIN pg_catalog.pg_foreign_server srv ON srv.oid = ft.ftserver
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE rel.oid = $1::oid
