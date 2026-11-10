-- Object definition: aggregate.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/aggregates/templates/aggregates/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Aggregate' AS kind, x.sig AS name,
    'DROP AGGREGATE IF EXISTS ' || x.sig || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE AGGREGATE ' || x.sig || E' (\n    ' || pg_catalog.concat_ws(E',\n    ',
            'SFUNC = ' || ag.aggtransfn::pg_catalog.regproc::text,
            'STYPE = ' || pg_catalog.format_type(ag.aggtranstype, NULL),
            CASE WHEN ag.aggtransspace <> 0 THEN 'SSPACE = ' || ag.aggtransspace END,
            CASE WHEN ag.aggfinalfn <> 0 THEN 'FINALFUNC = ' || ag.aggfinalfn::pg_catalog.regproc::text END,
            CASE WHEN ag.aggfinalextra THEN 'FINALFUNC_EXTRA' END,
            CASE WHEN ag.aggcombinefn <> 0 THEN 'COMBINEFUNC = ' || ag.aggcombinefn::pg_catalog.regproc::text END,
            CASE WHEN ag.aggserialfn <> 0 THEN 'SERIALFUNC = ' || ag.aggserialfn::pg_catalog.regproc::text END,
            CASE WHEN ag.aggdeserialfn <> 0 THEN 'DESERIALFUNC = ' || ag.aggdeserialfn::pg_catalog.regproc::text END,
            CASE WHEN ag.agginitval IS NOT NULL THEN 'INITCOND = ' || pg_catalog.quote_literal(ag.agginitval) END,
            CASE WHEN ag.aggsortop <> 0
                 THEN 'SORTOP = ' || (SELECT o.oprname FROM pg_catalog.pg_operator o WHERE o.oid = ag.aggsortop) END
        ) || E'\n);',
        'ALTER AGGREGATE ' || x.sig || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(pr.proowner)) || ';',
        'COMMENT ON AGGREGATE ' || x.sig || E'\n    IS '
            || pg_catalog.quote_literal(pg_catalog.obj_description(pr.oid, 'pg_proc')) || ';',
        {{ acl.grants("pr.proacl", "'FUNCTION'", "x.sig") }}
    ) AS body
FROM pg_catalog.pg_proc pr
    JOIN pg_catalog.pg_aggregate ag ON ag.aggfnoid = pr.oid
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = pr.pronamespace
    CROSS JOIN LATERAL (SELECT
        pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(pr.proname)
            || '(' || pg_catalog.pg_get_function_identity_arguments(pr.oid) || ')' AS sig) x
WHERE pr.oid = $1::oid
