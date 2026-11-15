-- Server dashboard (D-057): one row of counters for the graphs and the summary strip.
-- Graph counters adapted from pgAdmin 4 (dashboard/templates/dashboard/sql/default/dashboard_stats.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- $1 = database name, or NULL for the whole server. All columns are text; the
-- dashboard's own backend is not counted. `t` is the server clock (epoch seconds) for
-- the rates. Session counts follow $1; workers, WAL, slots and temporary files are the
-- whole server. Temporary files are NULL without the right to run pg_ls_tmpdir.
-- Oldest transaction / longest query / oldest xmin follow $1; deadlocks, conflicts and
-- checksum failures are cumulative (pgtc shows the change per refresh); replication
-- values are the whole server. Everything is read from memory: no table is scanned.
SELECT
    a.total::text AS total,
    a.active::text AS active,
    a.idle::text AS idle,
    COALESCE(d.commits, 0)::text AS commits,
    COALESCE(d.rollbacks, 0)::text AS rollbacks,
    COALESCE(d.inserted, 0)::text AS inserted,
    COALESCE(d.updated, 0)::text AS updated,
    COALESCE(d.deleted, 0)::text AS deleted,
    COALESCE(d.fetched, 0)::text AS fetched,
    COALESCE(d.returned, 0)::text AS returned,
    COALESCE(d.blks_read, 0)::text AS blks_read,
    COALESCE(d.blks_hit, 0)::text AS blks_hit,
    extract(epoch FROM pg_catalog.clock_timestamp())::text AS t,
    a.idle_in_xact::text AS idle_in_xact,
    a.idle_in_xact_aborted::text AS idle_in_xact_aborted,
    a.waiting::text AS waiting,
    pg_catalog.current_setting('max_connections') AS max_connections,
    w.autovacuum::text AS autovacuum,
    pg_catalog.current_setting('autovacuum_max_workers') AS autovacuum_max,
    w.parallel::text AS parallel,
    pg_catalog.current_setting('max_parallel_workers') AS parallel_max,
    w.logical::text AS logical,
    pg_catalog.current_setting('max_logical_replication_workers') AS logical_max,
    (SELECT count(*) FROM pg_catalog.pg_stat_replication)::text AS wal_senders,
    pg_catalog.current_setting('max_wal_senders') AS wal_senders_max,
    (SELECT count(*) FROM pg_catalog.pg_stat_wal_receiver)::text AS wal_receivers,
    (SELECT count(*) FROM pg_catalog.pg_replication_slots)::text AS slots,
    pg_catalog.current_setting('max_replication_slots') AS slots_max,
    extract(epoch FROM pg_catalog.clock_timestamp()
        - pg_catalog.pg_postmaster_start_time())::text AS uptime,
{% if has("TmpDirListing") %}
    CASE WHEN pg_catalog.has_function_privilege('pg_catalog.pg_ls_tmpdir(oid)', 'EXECUTE')
        THEN (SELECT count(*)
            FROM pg_catalog.pg_tablespace ts, LATERAL pg_catalog.pg_ls_tmpdir(ts.oid) f
            WHERE ts.spcname <> 'pg_global')::text
    END AS temp_files,
    CASE WHEN pg_catalog.has_function_privilege('pg_catalog.pg_ls_tmpdir(oid)', 'EXECUTE')
        THEN (SELECT COALESCE(sum(f.size), 0)
            FROM pg_catalog.pg_tablespace ts, LATERAL pg_catalog.pg_ls_tmpdir(ts.oid) f
            WHERE ts.spcname <> 'pg_global')::text
    END AS temp_bytes,
{% else %}
    NULL::text AS temp_files,
    NULL::text AS temp_bytes,
{% endif %}
    pg_catalog.pg_is_in_recovery()::text AS in_recovery,
    o.oldest_xact::text AS oldest_xact,
    o.longest_query::text AS longest_query,
    o.oldest_xmin::text AS oldest_xmin,
    o.oldest_xmin_pid::text AS oldest_xmin_pid,
    COALESCE(d.deadlocks, 0)::text AS deadlocks,
    COALESCE(d.conflicts, 0)::text AS conflicts,
{% if has("StatChecksums") %}
    COALESCE(d.checksum_failures, 0)::text AS checksum_failures,
{% else %}
    NULL::text AS checksum_failures,
{% endif %}
    -- on a standby pg_current_wal_lsn() is an error: CASE runs it only on a primary
    CASE WHEN NOT pg_catalog.pg_is_in_recovery()
        THEN (SELECT max(pg_catalog.pg_wal_lsn_diff(pg_catalog.pg_current_wal_lsn(), r.replay_lsn))
            FROM pg_catalog.pg_stat_replication r)::text
    END AS lag_bytes,
    (SELECT max(pg_catalog.pg_wal_lsn_diff(
            CASE WHEN pg_catalog.pg_is_in_recovery()
                THEN pg_catalog.pg_last_wal_replay_lsn()
                ELSE pg_catalog.pg_current_wal_lsn()
            END, sl.restart_lsn))
        FROM pg_catalog.pg_replication_slots sl)::text AS slot_wal_bytes
FROM (
    SELECT count(*) AS total,
        count(*) FILTER (WHERE s.state = 'active') AS active,
        count(*) FILTER (WHERE s.state = 'idle') AS idle,
        count(*) FILTER (WHERE s.state = 'idle in transaction') AS idle_in_xact,
        count(*) FILTER (WHERE s.state = 'idle in transaction (aborted)') AS idle_in_xact_aborted,
        count(*) FILTER (WHERE s.wait_event_type = 'Lock') AS waiting
    FROM pg_catalog.pg_stat_activity s
    WHERE s.backend_type = 'client backend'
        AND s.pid <> pg_catalog.pg_backend_pid()
        AND ($1::text IS NULL OR s.datname = $1::text)
) a, (
    SELECT count(*) FILTER (WHERE s.backend_type = 'autovacuum worker') AS autovacuum,
        count(*) FILTER (WHERE s.backend_type = 'parallel worker') AS parallel,
        count(*) FILTER (WHERE s.backend_type LIKE 'logical replication%worker'
            AND s.backend_type <> 'logical replication launcher') AS logical
    FROM pg_catalog.pg_stat_activity s
) w, (
    -- WAL senders run one long "query" from their start: left out
    SELECT
        extract(epoch FROM pg_catalog.clock_timestamp() - min(s.xact_start))
            AS oldest_xact,
        extract(epoch FROM pg_catalog.clock_timestamp()
            - min(s.query_start) FILTER (WHERE s.state = 'active')) AS longest_query,
        max(pg_catalog.age(s.backend_xmin)) AS oldest_xmin,
        (pg_catalog.array_agg(s.pid ORDER BY pg_catalog.age(s.backend_xmin) DESC)
            FILTER (WHERE s.backend_xmin IS NOT NULL))[1] AS oldest_xmin_pid
    FROM pg_catalog.pg_stat_activity s
    WHERE s.pid <> pg_catalog.pg_backend_pid()
        AND s.backend_type <> 'walsender'
        AND ($1::text IS NULL OR s.datname = $1::text)
) o, (
    SELECT sum(xact_commit) AS commits,
        sum(xact_rollback) AS rollbacks,
        sum(tup_inserted) AS inserted,
        sum(tup_updated) AS updated,
        sum(tup_deleted) AS deleted,
        sum(tup_fetched) AS fetched,
        sum(tup_returned) AS returned,
        sum(blks_read) AS blks_read,
        sum(blks_hit) AS blks_hit,
        sum(deadlocks) AS deadlocks,
        sum(conflicts) AS conflicts{% if has("StatChecksums") %},
        sum(checksum_failures) AS checksum_failures{% endif %}
    FROM pg_catalog.pg_stat_database
    WHERE $1::text IS NULL OR datname = $1::text
) d
