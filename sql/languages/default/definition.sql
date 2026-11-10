-- Object definition: procedural language.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/languages/templates/languages/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Language' AS kind, lan.lanname::text AS name,
    'DROP LANGUAGE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE ' || CASE WHEN lan.lanpltrusted THEN 'TRUSTED ' ELSE '' END || 'PROCEDURAL LANGUAGE ' || x.q
            || CASE WHEN lan.lanplcallfoid <> 0 THEN E'\n    HANDLER ' || lan.lanplcallfoid::pg_catalog.regproc::text ELSE '' END
            || CASE WHEN lan.laninline <> 0 THEN E'\n    INLINE ' || lan.laninline::pg_catalog.regproc::text ELSE '' END
            || CASE WHEN lan.lanvalidator <> 0 THEN E'\n    VALIDATOR ' || lan.lanvalidator::pg_catalog.regproc::text ELSE '' END
            || ';',
        'ALTER LANGUAGE ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(lan.lanowner)) || ';',
        'COMMENT ON LANGUAGE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(lan.oid, 'pg_language')) || ';',
        {{ acl.grants("lan.lanacl", "'LANGUAGE'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_language lan
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(lan.lanname) AS q) x
WHERE lan.oid = $1::oid
