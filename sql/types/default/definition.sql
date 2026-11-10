-- Object definition: composite, enum, range or base type.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/types/templates/types/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Type' AS kind, x.q AS name,
    'DROP TYPE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE TYPE ' || x.q || CASE t.typtype
            WHEN 'c' THEN E' AS\n(\n' || COALESCE((
                SELECT pg_catalog.string_agg('    ' || pg_catalog.quote_ident(a.attname) || ' '
                           || pg_catalog.format_type(a.atttypid, a.atttypmod), E',\n' ORDER BY a.attnum)
                FROM pg_catalog.pg_attribute a
                WHERE a.attrelid = t.typrelid AND a.attnum > 0 AND NOT a.attisdropped), '') || E'\n)'
            WHEN 'e' THEN E' AS ENUM\n    (' || COALESCE((
                SELECT pg_catalog.string_agg(pg_catalog.quote_literal(e.enumlabel), ', ' ORDER BY e.enumsortorder)
                FROM pg_catalog.pg_enum e WHERE e.enumtypid = t.oid), '') || ')'
            WHEN 'r' THEN E' AS RANGE\n(\n    ' || (
                SELECT pg_catalog.concat_ws(E',\n    ',
                    'SUBTYPE = ' || pg_catalog.format_type(r.rngsubtype, NULL),
                    CASE WHEN r.rngsubopc <> 0 THEN 'SUBTYPE_OPCLASS = ' || (
                        SELECT pg_catalog.quote_ident(opc.opcname) FROM pg_catalog.pg_opclass opc
                        WHERE opc.oid = r.rngsubopc) END,
                    CASE WHEN r.rngcollation <> 0 THEN 'COLLATION = ' || (
                        SELECT pg_catalog.quote_ident(cn.nspname) || '.' || pg_catalog.quote_ident(co.collname)
                        FROM pg_catalog.pg_collation co JOIN pg_catalog.pg_namespace cn ON cn.oid = co.collnamespace
                        WHERE co.oid = r.rngcollation) END,
                    CASE WHEN r.rngcanonical <> 0 THEN 'CANONICAL = ' || r.rngcanonical::pg_catalog.regproc::text END,
                    CASE WHEN r.rngsubdiff <> 0 THEN 'SUBTYPE_DIFF = ' || r.rngsubdiff::pg_catalog.regproc::text END)
                FROM pg_catalog.pg_range r WHERE r.rngtypid = t.oid) || E'\n)'
            ELSE E'\n(\n    ' || pg_catalog.concat_ws(E',\n    ',
                'INPUT = ' || t.typinput::pg_catalog.regproc::text,
                'OUTPUT = ' || t.typoutput::pg_catalog.regproc::text,
                CASE WHEN t.typreceive <> 0 THEN 'RECEIVE = ' || t.typreceive::pg_catalog.regproc::text END,
                CASE WHEN t.typsend <> 0 THEN 'SEND = ' || t.typsend::pg_catalog.regproc::text END,
                CASE WHEN t.typmodin <> 0 THEN 'TYPMOD_IN = ' || t.typmodin::pg_catalog.regproc::text END,
                CASE WHEN t.typmodout <> 0 THEN 'TYPMOD_OUT = ' || t.typmodout::pg_catalog.regproc::text END,
                CASE WHEN t.typanalyze <> 0 THEN 'ANALYZE = ' || t.typanalyze::pg_catalog.regproc::text END,
                'INTERNALLENGTH = ' || CASE WHEN t.typlen = -1 THEN 'VARIABLE' ELSE t.typlen::text END,
                CASE WHEN t.typbyval THEN 'PASSEDBYVALUE' END,
                'ALIGNMENT = ' || CASE t.typalign WHEN 'c' THEN 'char' WHEN 's' THEN 'int2'
                                                  WHEN 'i' THEN 'int4' ELSE 'double' END,
                'STORAGE = ' || CASE t.typstorage WHEN 'p' THEN 'plain' WHEN 'e' THEN 'external'
                                                  WHEN 'm' THEN 'main' ELSE 'extended' END,
                'CATEGORY = ' || pg_catalog.quote_literal(t.typcategory::text),
                CASE WHEN t.typispreferred THEN 'PREFERRED = true' END,
                CASE WHEN t.typdelim <> ',' THEN 'DELIMITER = ' || pg_catalog.quote_literal(t.typdelim::text) END) || E'\n)'
        END || ';',
        'ALTER TYPE ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(t.typowner)) || ';',
        'COMMENT ON TYPE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(t.oid, 'pg_type')) || ';',
        {{ acl.grants("t.typacl", "'TYPE'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_type t
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = t.typnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(t.typname) AS q) x
WHERE t.oid = $1::oid
