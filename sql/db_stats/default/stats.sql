-- Database statistics (D-062): one row per statistic of the connected database.
-- Partly adapted from pgAdmin 4 (databases/templates/databases/sql/default/stats.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns (all text): key, group, statistic, kind, value; kind is how pgtc shows the
-- value: count, bytes, ms, percent, time or text. Rows in display order.
SELECT v.key, v.grp, v.label, v.kind, v.value
FROM pg_catalog.pg_database d
    LEFT JOIN pg_catalog.pg_stat_database s ON s.datid = d.oid
    LEFT JOIN pg_catalog.pg_stat_database_conflicts c ON c.datid = d.oid
    CROSS JOIN LATERAL (VALUES
        (1, 'read_at', 'Database', 'Read at', 'time',
            pg_catalog.date_trunc('second', pg_catalog.statement_timestamp())::text),
        (2, 'size', 'Database', 'Size', 'bytes', pg_catalog.pg_database_size(d.oid)::text),
        (3, 'backends', 'Database', 'Backends', 'count', s.numbackends::text),
        (4, 'conn_limit', 'Database', 'Connection limit', 'text',
            CASE WHEN d.datconnlimit < 0 THEN 'no limit' ELSE d.datconnlimit::text END),
        (5, 'xid_age', 'Database', 'Transaction ID age', 'count',
            pg_catalog.age(d.datfrozenxid)::text),
        (6, 'mxid_age', 'Database', 'Multixact ID age', 'count',
            pg_catalog.mxid_age(d.datminmxid)::text),
        (7, 'stats_reset', 'Database', 'Statistics reset', 'time',
            pg_catalog.date_trunc('second', s.stats_reset)::text),
        (10, 'xact_commit', 'Transactions', 'Committed', 'count', s.xact_commit::text),
        (11, 'xact_rollback', 'Transactions', 'Rolled back', 'count', s.xact_rollback::text),
        (12, 'deadlocks', 'Transactions', 'Deadlocks', 'count', s.deadlocks::text),
        (20, 'blks_read', 'Block I/O', 'Blocks read', 'count', s.blks_read::text),
        (21, 'blks_hit', 'Block I/O', 'Blocks hit', 'count', s.blks_hit::text),
        (22, 'cache_hit', 'Block I/O', 'Cache hit ratio', 'percent',
            CASE WHEN s.blks_read + s.blks_hit > 0
                THEN (100.0 * s.blks_hit / (s.blks_read + s.blks_hit))::text
            END),
        (23, 'blk_read_time', 'Block I/O', 'Read time', 'ms', s.blk_read_time::text),
        (24, 'blk_write_time', 'Block I/O', 'Write time', 'ms', s.blk_write_time::text),
        (30, 'tup_returned', 'Tuples', 'Returned', 'count', s.tup_returned::text),
        (31, 'tup_fetched', 'Tuples', 'Fetched', 'count', s.tup_fetched::text),
        (32, 'tup_inserted', 'Tuples', 'Inserted', 'count', s.tup_inserted::text),
        (33, 'tup_updated', 'Tuples', 'Updated', 'count', s.tup_updated::text),
        (34, 'tup_deleted', 'Tuples', 'Deleted', 'count', s.tup_deleted::text),
        (40, 'temp_files', 'Temporary files', 'Files', 'count', s.temp_files::text),
        (41, 'temp_bytes', 'Temporary files', 'Size', 'bytes', s.temp_bytes::text),
{% if has("StatSessions") %}
        (50, 'sessions', 'Sessions', 'Sessions', 'count', s.sessions::text),
        (51, 'sessions_abandoned', 'Sessions', 'Abandoned', 'count', s.sessions_abandoned::text),
        (52, 'sessions_fatal', 'Sessions', 'Ended by fatal errors', 'count', s.sessions_fatal::text),
        (53, 'sessions_killed', 'Sessions', 'Terminated', 'count', s.sessions_killed::text),
        (54, 'session_time', 'Sessions', 'Session time', 'ms', s.session_time::text),
        (55, 'active_time', 'Sessions', 'Active time', 'ms', s.active_time::text),
        (56, 'idle_in_transaction_time', 'Sessions', 'Idle in transaction time', 'ms',
            s.idle_in_transaction_time::text),
{% endif %}
{% if has("StatChecksums") %}
        (60, 'checksum_failures', 'Checksums', 'Failures', 'count',
            COALESCE(s.checksum_failures::text, 'checksums off')),
        (61, 'checksum_last_failure', 'Checksums', 'Last failure', 'time',
            pg_catalog.date_trunc('second', s.checksum_last_failure)::text),
{% endif %}
{% if has("StatParallelWorkers") %}
        (70, 'parallel_workers_to_launch', 'Parallel workers', 'Planned', 'count',
            s.parallel_workers_to_launch::text),
        (71, 'parallel_workers_launched', 'Parallel workers', 'Launched', 'count',
            s.parallel_workers_launched::text),
{% endif %}
{% if has("StatLogicalSlotConflicts") %}
        (86, 'confl_active_logicalslot', 'Recovery conflicts', 'Logical slot', 'count',
            c.confl_active_logicalslot::text),
{% endif %}
        (80, 'conflicts', 'Recovery conflicts', 'Total', 'count', s.conflicts::text),
        (81, 'confl_tablespace', 'Recovery conflicts', 'Tablespace', 'count', c.confl_tablespace::text),
        (82, 'confl_lock', 'Recovery conflicts', 'Lock', 'count', c.confl_lock::text),
        (83, 'confl_snapshot', 'Recovery conflicts', 'Snapshot', 'count', c.confl_snapshot::text),
        (84, 'confl_bufferpin', 'Recovery conflicts', 'Buffer pin', 'count', c.confl_bufferpin::text),
        (85, 'confl_deadlock', 'Recovery conflicts', 'Deadlock', 'count', c.confl_deadlock::text)
    ) AS v(ord, key, grp, label, kind, value)
WHERE d.datname = pg_catalog.current_database()
ORDER BY v.ord
