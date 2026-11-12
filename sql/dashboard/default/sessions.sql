-- Server dashboard (D-057): the Sessions list.
-- Adapted from pgAdmin 4 (dashboard/templates/dashboard/sql/default/activity.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- $1 = database name, or NULL for the whole server. All columns are text, in the order
-- of the dashboard's Sessions columns; the dashboard's own backend is left out.
SELECT
    s.pid::text AS pid,
    s.datname::text AS database,
    s.usename::text AS usename,
    s.application_name,
    s.client_addr::text AS client,
    pg_catalog.to_char(s.backend_start, 'YYYY-MM-DD HH24:MI:SS') AS backend_start,
    s.state,
    s.wait_event_type || ': ' || s.wait_event AS wait_event,
    pg_catalog.array_to_string(pg_catalog.pg_blocking_pids(s.pid), ', ') AS blocked_by,
    pg_catalog.to_char(s.xact_start, 'YYYY-MM-DD HH24:MI:SS') AS xact_start,
    pg_catalog.to_char(s.query_start, 'YYYY-MM-DD HH24:MI:SS') AS query_start,
    CASE WHEN s.state = 'active'
        THEN pg_catalog.date_trunc('second', pg_catalog.clock_timestamp() - s.query_start)::text
    END AS running_for,
    s.backend_type,
    s.query
FROM pg_catalog.pg_stat_activity s
WHERE s.pid <> pg_catalog.pg_backend_pid()
    AND ($1::text IS NULL OR s.datname = $1::text)
ORDER BY s.pid
