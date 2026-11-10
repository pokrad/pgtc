-- Object definition: event trigger.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/event_triggers/templates/event_triggers/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'Event Trigger' AS kind, e.evtname::text AS name,
    'DROP EVENT TRIGGER IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE EVENT TRIGGER ' || x.q || E'\n    ON ' || e.evtevent
            || COALESCE(E'\n    WHEN TAG IN (' || (SELECT pg_catalog.string_agg(pg_catalog.quote_literal(tag), ', ')
                                                FROM pg_catalog.unnest(e.evttags) tag) || ')', '')
            || E'\n    EXECUTE FUNCTION ' || e.evtfoid::pg_catalog.regproc::text || '();',
        CASE e.evtenabled WHEN 'D' THEN 'ALTER EVENT TRIGGER ' || x.q || ' DISABLE;'
                          WHEN 'R' THEN 'ALTER EVENT TRIGGER ' || x.q || ' ENABLE REPLICA;'
                          WHEN 'A' THEN 'ALTER EVENT TRIGGER ' || x.q || ' ENABLE ALWAYS;' END,
        'ALTER EVENT TRIGGER ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(e.evtowner)) || ';',
        'COMMENT ON EVENT TRIGGER ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(e.oid, 'pg_event_trigger')) || ';'
    ) AS body
FROM pg_catalog.pg_event_trigger e
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(e.evtname) AS q) x
WHERE e.oid = $1::oid
