-- Server dashboard (D-057): the Sessions list.
-- Adapted from pgAdmin 4 (dashboard/templates/dashboard/sql/default/activity.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- $1 = database name, or NULL for the whole server. All columns are text; the
-- dashboard's own backend is left out. The three durations are seconds (query: only
-- for sessions that are not idle); pgtc shows one of them, orders by it and groups
-- parallel workers under their leader.
SELECT
    s.pid::text AS pid,
    s.datname::text AS database,
    s.usename::text AS usename,
    s.application_name,
    s.client_addr::text AS client,
    s.state,
    s.wait_event_type || ': ' || s.wait_event AS wait_event,
    CASE WHEN s.wait_event_type = 'Lock'
        THEN pg_catalog.array_to_string(pg_catalog.pg_blocking_pids(s.pid), ', ')
        ELSE ''
    END AS blocked_by,
    CASE WHEN s.state IS DISTINCT FROM 'idle'
        THEN extract(epoch FROM pg_catalog.clock_timestamp() - s.query_start)::text
    END AS query_secs,
    extract(epoch FROM pg_catalog.clock_timestamp() - s.xact_start)::text AS xact_secs,
    extract(epoch FROM pg_catalog.clock_timestamp() - s.backend_start)::text AS backend_secs,
    pg_catalog.age(s.backend_xmin)::text AS xmin_age,
    s.backend_xid::text AS xid,
{% if has("LeaderPid") %}
    s.leader_pid::text AS leader_pid,
{% else %}
    NULL::text AS leader_pid,
{% endif %}
{% if has("QueryId") %}
    s.query_id::text AS query_id,
{% else %}
    NULL::text AS query_id,
{% endif %}
    pg_catalog.to_char(s.backend_start, 'YYYY-MM-DD HH24:MI:SS') AS backend_start,
    pg_catalog.to_char(s.xact_start, 'YYYY-MM-DD HH24:MI:SS') AS xact_start,
    pg_catalog.to_char(s.query_start, 'YYYY-MM-DD HH24:MI:SS') AS query_start,
    s.backend_type,
    s.query
FROM pg_catalog.pg_stat_activity s
WHERE s.pid <> pg_catalog.pg_backend_pid()
    AND ($1::text IS NULL OR s.datname = $1::text)
ORDER BY s.pid
