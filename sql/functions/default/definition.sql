-- Object definition: function.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/functions/templates/functions/pg/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT CASE pr.prokind WHEN 'p' THEN 'Procedure' ELSE 'Function' END AS kind, x.sig AS name,
    'DROP ' || x.k || ' IF EXISTS ' || x.sig || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        pg_catalog.regexp_replace(pg_catalog.pg_get_functiondef(pr.oid), '\s+$', '') || ';',
        'ALTER ' || x.k || ' ' || x.sig || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(pr.proowner)) || ';',
        'COMMENT ON ' || x.k || ' ' || x.sig || E'\n    IS '
            || pg_catalog.quote_literal(pg_catalog.obj_description(pr.oid, 'pg_proc')) || ';',
        {{ acl.grants("pr.proacl", "x.k", "x.sig") }}
    ) AS body
FROM pg_catalog.pg_proc pr
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = pr.pronamespace
    CROSS JOIN LATERAL (SELECT
        pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(pr.proname)
            || '(' || pg_catalog.pg_get_function_identity_arguments(pr.oid) || ')' AS sig,
        CASE pr.prokind WHEN 'p' THEN 'PROCEDURE' ELSE 'FUNCTION' END AS k) x
WHERE pr.oid = $1::oid
