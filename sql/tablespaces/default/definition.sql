-- Object definition: tablespace.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (tablespaces/templates/tablespaces/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Tablespace' AS kind, ts.spcname::text AS name,
    'DROP TABLESPACE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE TABLESPACE ' || x.q || E'\n    OWNER ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(ts.spcowner))
            || E'\n    LOCATION ' || pg_catalog.quote_literal(pg_catalog.pg_tablespace_location(ts.oid)) || ';',
        CASE WHEN ts.spcoptions IS NOT NULL
             THEN 'ALTER TABLESPACE ' || x.q || ' SET (' || pg_catalog.array_to_string(ts.spcoptions, ', ') || ');' END,
        'COMMENT ON TABLESPACE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal((SELECT d.description FROM pg_catalog.pg_shdescription d WHERE d.objoid = ts.oid AND d.classoid = 'pg_catalog.pg_tablespace'::regclass)) || ';',
        {{ acl.grants("ts.spcacl", "'TABLESPACE'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_tablespace ts
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(ts.spcname) AS q) x
WHERE ts.oid = $1::oid
