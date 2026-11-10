-- Object definition: database.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/templates/databases/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Database' AS kind, db.datname::text AS name,
    'DROP DATABASE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE DATABASE ' || x.q
            || E'\n    WITH\n    OWNER = ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(db.datdba))
            || E'\n    ENCODING = ' || pg_catalog.quote_literal(pg_catalog.pg_encoding_to_char(db.encoding))
            || E'\n    LC_COLLATE = ' || pg_catalog.quote_literal(db.datcollate)
            || E'\n    LC_CTYPE = ' || pg_catalog.quote_literal(db.datctype)
            || E'\n    TABLESPACE = ' || pg_catalog.quote_ident(ts.spcname)
            || E'\n    CONNECTION LIMIT = ' || db.datconnlimit
            || CASE WHEN db.datistemplate THEN E'\n    IS_TEMPLATE = True' ELSE '' END
            || ';',
        'COMMENT ON DATABASE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal((SELECT d.description FROM pg_catalog.pg_shdescription d WHERE d.objoid = db.oid AND d.classoid = 'pg_catalog.pg_database'::regclass)) || ';',
        {{ acl.grants("db.datacl", "'DATABASE'", "x.q") }}
    ) AS body
FROM pg_catalog.pg_database db
    JOIN pg_catalog.pg_tablespace ts ON ts.oid = db.dattablespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(db.datname) AS q) x
WHERE db.oid = $1::oid
