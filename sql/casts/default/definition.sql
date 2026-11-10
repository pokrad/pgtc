-- Object definition: cast.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/casts/templates/casts/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Cast' AS kind, x.src || '->' || x.tgt AS name,
    'DROP CAST IF EXISTS (' || x.src || ' AS ' || x.tgt || ');' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE CAST (' || x.src || ' AS ' || x.tgt || E')\n    '
            || CASE ca.castmethod WHEN 'f' THEN 'WITH FUNCTION ' || ca.castfunc::pg_catalog.regprocedure::text
                                  WHEN 'i' THEN 'WITH INOUT' ELSE 'WITHOUT FUNCTION' END
            || CASE ca.castcontext WHEN 'a' THEN E'\n    AS ASSIGNMENT' WHEN 'i' THEN E'\n    AS IMPLICIT' ELSE '' END
            || ';',
        'COMMENT ON CAST (' || x.src || ' AS ' || x.tgt || E')\n    IS '
            || pg_catalog.quote_literal(pg_catalog.obj_description(ca.oid, 'pg_cast')) || ';'
    ) AS body
FROM pg_catalog.pg_cast ca
    CROSS JOIN LATERAL (SELECT pg_catalog.format_type(ca.castsource, NULL) AS src,
                               pg_catalog.format_type(ca.casttarget, NULL) AS tgt) x
WHERE ca.oid = $1::oid
