-- Object definition: operator.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/operators/templates/operators/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Operator' AS kind, x.sig AS name,
    'DROP OPERATOR IF EXISTS ' || x.sig || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE OPERATOR ' || x.op || E' (\n    ' || pg_catalog.concat_ws(E',\n    ',
            'FUNCTION = ' || o.oprcode::pg_catalog.regproc::text,
            CASE WHEN o.oprleft <> 0 THEN 'LEFTARG = ' || pg_catalog.format_type(o.oprleft, NULL) END,
            CASE WHEN o.oprright <> 0 THEN 'RIGHTARG = ' || pg_catalog.format_type(o.oprright, NULL) END,
            CASE WHEN o.oprcom <> 0 THEN 'COMMUTATOR = ' || (SELECT c.oprname FROM pg_catalog.pg_operator c WHERE c.oid = o.oprcom) END,
            CASE WHEN o.oprnegate <> 0 THEN 'NEGATOR = ' || (SELECT n.oprname FROM pg_catalog.pg_operator n WHERE n.oid = o.oprnegate) END,
            CASE WHEN o.oprrest <> 0 THEN 'RESTRICT = ' || o.oprrest::pg_catalog.regproc::text END,
            CASE WHEN o.oprjoin <> 0 THEN 'JOIN = ' || o.oprjoin::pg_catalog.regproc::text END,
            CASE WHEN o.oprcanhash THEN 'HASHES' END,
            CASE WHEN o.oprcanmerge THEN 'MERGES' END) || E'\n);',
        'ALTER OPERATOR ' || x.sig || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(o.oprowner)) || ';',
        'COMMENT ON OPERATOR ' || x.sig || E'\n    IS '
            || pg_catalog.quote_literal(pg_catalog.obj_description(o.oid, 'pg_operator')) || ';'
    ) AS body
FROM pg_catalog.pg_operator o
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = o.oprnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || o.oprname AS op,
                               pg_catalog.quote_ident(nsp.nspname) || '.' || o.oprname || ' (' || COALESCE(pg_catalog.format_type(NULLIF(o.oprleft, 0), NULL), 'NONE') || ', ' || COALESCE(pg_catalog.format_type(NULLIF(o.oprright, 0), NULL), 'NONE') || ')' AS sig) x
WHERE o.oid = $1::oid
