-- Object definition: publication.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/publications/templates/publications/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Publication' AS kind, p.pubname::text AS name,
    'DROP PUBLICATION IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE PUBLICATION ' || x.q
            || CASE WHEN p.puballtables THEN E'\n    FOR ALL TABLES'
                    ELSE COALESCE(E'\n    FOR TABLE ' || (
                        SELECT pg_catalog.string_agg(pg_catalog.quote_ident(pt.schemaname) || '.'
                                   || pg_catalog.quote_ident(pt.tablename), ', ' ORDER BY pt.schemaname, pt.tablename)
                        FROM pg_catalog.pg_publication_tables pt WHERE pt.pubname = p.pubname), '') END
            || E'\n    WITH (' || pg_catalog.concat_ws(', ',
                'publish = ' || pg_catalog.quote_literal(pg_catalog.concat_ws(', ',
                    CASE WHEN p.pubinsert THEN 'insert' END, CASE WHEN p.pubupdate THEN 'update' END,
                    CASE WHEN p.pubdelete THEN 'delete' END, CASE WHEN p.pubtruncate THEN 'truncate' END))) || ');',
        'ALTER PUBLICATION ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(p.pubowner)) || ';',
        'COMMENT ON PUBLICATION ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(p.oid, 'pg_publication')) || ';'
    ) AS body
FROM pg_catalog.pg_publication p
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(p.pubname) AS q) x
WHERE p.oid = $1::oid
