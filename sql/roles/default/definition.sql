-- Object definition: login / group role (passwords are never shown).
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (roles/templates/roles/sql/default/sql.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT CASE WHEN r.rolcanlogin THEN 'Role' ELSE 'Group role' END AS kind, r.rolname::text AS name,
    'DROP ROLE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE ROLE ' || x.q || E' WITH\n  ' || pg_catalog.concat_ws(E'\n  ',
            CASE WHEN r.rolcanlogin THEN 'LOGIN' ELSE 'NOLOGIN' END,
            CASE WHEN r.rolsuper THEN 'SUPERUSER' ELSE 'NOSUPERUSER' END,
            CASE WHEN r.rolinherit THEN 'INHERIT' ELSE 'NOINHERIT' END,
            CASE WHEN r.rolcreatedb THEN 'CREATEDB' ELSE 'NOCREATEDB' END,
            CASE WHEN r.rolcreaterole THEN 'CREATEROLE' ELSE 'NOCREATEROLE' END,
            CASE WHEN r.rolreplication THEN 'REPLICATION' ELSE 'NOREPLICATION' END,
            CASE WHEN r.rolbypassrls THEN 'BYPASSRLS' ELSE 'NOBYPASSRLS' END,
            CASE WHEN r.rolconnlimit <> -1 THEN 'CONNECTION LIMIT ' || r.rolconnlimit END,
            CASE WHEN r.rolvaliduntil IS NOT NULL
                 THEN 'VALID UNTIL ' || pg_catalog.quote_literal(r.rolvaliduntil::text) END) || ';',
        (SELECT pg_catalog.string_agg('GRANT ' || pg_catalog.quote_ident(g.rolname) || ' TO ' || x.q
                || CASE WHEN m.admin_option THEN ' WITH ADMIN OPTION' ELSE '' END || ';', E'\n' ORDER BY g.rolname)
            FROM pg_catalog.pg_auth_members m
                JOIN pg_catalog.pg_roles g ON g.oid = m.roleid
            WHERE m.member = r.oid),
        (SELECT pg_catalog.string_agg('ALTER ROLE ' || x.q || ' SET ' || pg_catalog.split_part(cfg, '=', 1)
                || ' TO ' || pg_catalog.quote_literal(pg_catalog.substr(cfg, pg_catalog.strpos(cfg, '=') + 1)) || ';', E'\n')
            FROM pg_catalog.unnest(r.rolconfig) cfg),
        'COMMENT ON ROLE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal((SELECT d.description FROM pg_catalog.pg_shdescription d WHERE d.objoid = r.oid AND d.classoid = 'pg_catalog.pg_authid'::regclass)) || ';'
    ) AS body
FROM pg_catalog.pg_roles r
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(r.rolname) AS q) x
WHERE r.oid = $1::oid
