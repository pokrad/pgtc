-- Object definition: sequence.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/sequences/templates/sequences/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Sequence' AS kind, x.q AS name,
    'DROP SEQUENCE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE SEQUENCE IF NOT EXISTS ' || x.q
            || E'\n    AS ' || pg_catalog.format_type(s.seqtypid, NULL)
            || E'\n    INCREMENT ' || s.seqincrement
            || E'\n    START ' || s.seqstart
            || E'\n    MINVALUE ' || s.seqmin
            || E'\n    MAXVALUE ' || s.seqmax
            || E'\n    CACHE ' || s.seqcache
            || CASE WHEN s.seqcycle THEN E'\n    CYCLE' ELSE '' END || ';',
        (SELECT 'ALTER SEQUENCE ' || x.q || E'\n    OWNED BY ' || pg_catalog.quote_ident(tn.nspname) || '.'
                || pg_catalog.quote_ident(tc.relname) || '.' || pg_catalog.quote_ident(a.attname) || ';'
            FROM pg_catalog.pg_depend d
                JOIN pg_catalog.pg_class tc ON tc.oid = d.refobjid
                JOIN pg_catalog.pg_namespace tn ON tn.oid = tc.relnamespace
                JOIN pg_catalog.pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid
            WHERE d.classid = 'pg_catalog.pg_class'::regclass AND d.objid = rel.oid
                AND d.refclassid = 'pg_catalog.pg_class'::regclass AND d.deptype = 'a'
            LIMIT 1),
        'ALTER SEQUENCE ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(rel.relowner)) || ';',
        'COMMENT ON SEQUENCE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(rel.oid, 'pg_class')) || ';',
        {{ acl.grants("rel.relacl", "'SEQUENCE'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_class rel
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    JOIN pg_catalog.pg_sequence s ON s.seqrelid = rel.oid
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q) x
WHERE rel.oid = $1::oid
