-- Object definition: subscription.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/subscriptions/templates/subscriptions/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Subscription' AS kind, s.subname::text AS name,
    'DROP SUBSCRIPTION IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE SUBSCRIPTION ' || x.q
            || E'\n    CONNECTION ' || pg_catalog.quote_literal(s.subconninfo)
            || E'\n    PUBLICATION ' || (SELECT pg_catalog.string_agg(pg_catalog.quote_ident(pub), ', ')
                                        FROM pg_catalog.unnest(s.subpublications) pub)
            || E'\n    WITH (enabled = ' || s.subenabled::text || ', create_slot = false, slot_name = '
            || COALESCE(pg_catalog.quote_literal(s.subslotname::text), 'NONE')
            || ', synchronous_commit = ' || pg_catalog.quote_literal(s.subsynccommit) || ');',
        'ALTER SUBSCRIPTION ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(s.subowner)) || ';',
        'COMMENT ON SUBSCRIPTION ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(s.oid, 'pg_subscription')) || ';'
    ) AS body
FROM pg_catalog.pg_subscription s
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(s.subname) AS q) x
WHERE s.oid = $1::oid
