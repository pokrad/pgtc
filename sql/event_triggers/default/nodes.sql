-- Tree nodes: event triggers.
-- Adapted from pgAdmin 4 (databases/event_triggers/templates/event_triggers/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT e.oid, e.evtname AS name
FROM pg_catalog.pg_event_trigger e
{% if not show_system %}
WHERE NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_event_trigger'::regclass AND dep.objid = e.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY e.evtname
