-- Object definition: foreign data wrapper.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/foreign_data_wrappers/templates/foreign_data_wrappers/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Foreign Data Wrapper' AS kind, fdw.fdwname::text AS name,
    'DROP FOREIGN DATA WRAPPER IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE FOREIGN DATA WRAPPER ' || x.q
            || CASE WHEN fdw.fdwhandler <> 0 THEN E'\n    HANDLER ' || fdw.fdwhandler::pg_catalog.regproc::text ELSE '' END
            || CASE WHEN fdw.fdwvalidator <> 0 THEN E'\n    VALIDATOR ' || fdw.fdwvalidator::pg_catalog.regproc::text ELSE '' END
            || COALESCE(E'\n    OPTIONS (' || (SELECT pg_catalog.string_agg(pg_catalog.quote_ident(o.option_name) || ' ' || pg_catalog.quote_literal(o.option_value), ', ') FROM pg_catalog.pg_options_to_table(fdw.fdwoptions) o) || ')', '') || ';',
        'ALTER FOREIGN DATA WRAPPER ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(fdw.fdwowner)) || ';',
        'COMMENT ON FOREIGN DATA WRAPPER ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(fdw.oid, 'pg_foreign_data_wrapper')) || ';',
        {{ acl.grants("fdw.fdwacl", "'FOREIGN DATA WRAPPER'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_foreign_data_wrapper fdw
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(fdw.fdwname) AS q) x
WHERE fdw.oid = $1::oid
