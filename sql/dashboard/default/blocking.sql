-- Server dashboard (D-057): the Blocking list — one row per blocking and blocked
-- session (pg_blocking_pids), with the lock the blocked session waits for.
-- $1 = database name (of the blocked session), or NULL for the whole server. All
-- columns are text; the two durations are seconds. The dashboard's own backend is
-- left out.
SELECT
    b.pid::text AS blocker_pid,
    b.datname::text AS database,
    b.usename::text AS blocker_user,
    b.state AS blocker_state,
    extract(epoch FROM pg_catalog.clock_timestamp() - b.xact_start)::text AS xact_secs,
    w.pid::text AS blocked_pid,
    w.usename::text AS blocked_user,
    l.locktype,
    l.mode,
    l.relation::regclass::text AS relation,
    extract(epoch FROM pg_catalog.clock_timestamp() - w.query_start)::text AS wait_secs,
    b.query AS blocker_query,
    w.query AS blocked_query
FROM pg_catalog.pg_stat_activity w
    CROSS JOIN LATERAL pg_catalog.unnest(pg_catalog.pg_blocking_pids(w.pid)) AS bp(pid)
    JOIN pg_catalog.pg_stat_activity b ON b.pid = bp.pid
    LEFT JOIN LATERAL (
        SELECT k.locktype, k.mode, k.relation
        FROM pg_catalog.pg_locks k
        WHERE k.pid = w.pid AND NOT k.granted
        LIMIT 1
    ) l ON true
WHERE w.wait_event_type = 'Lock'
    AND w.pid <> pg_catalog.pg_backend_pid()
    AND b.pid <> pg_catalog.pg_backend_pid()
    AND ($1::text IS NULL OR w.datname = $1::text)
ORDER BY w.query_start, b.pid, w.pid
