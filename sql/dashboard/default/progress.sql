-- Server dashboard (D-057): the Progress list — running maintenance commands from the
-- pg_stat_progress_* views (in memory, one row per running command). $1 = database
-- name, or NULL for the whole server (base backups have no database: whole server
-- only). All columns are text: done / total are blocks or bytes (`unit`), running_secs
-- is seconds since the command's query started. No size functions: no file is touched.
WITH p AS (
    SELECT v.pid, v.datname, 'VACUUM'::text AS command, v.relid, v.phase::text AS phase,
        v.heap_blks_scanned::int8 AS done, v.heap_blks_total::int8 AS total,
        'blocks'::text AS unit
    FROM pg_catalog.pg_stat_progress_vacuum v
{% if has("ProgressCluster") %}
    UNION ALL
    SELECT c.pid, c.datname, c.command::text, c.relid, c.phase::text,
        c.heap_blks_scanned::int8, c.heap_blks_total::int8, 'blocks'
    FROM pg_catalog.pg_stat_progress_cluster c
{% endif %}
{% if has("ProgressCreateIndex") %}
    UNION ALL
    SELECT i.pid, i.datname, i.command::text, i.relid, i.phase::text,
        i.blocks_done::int8, i.blocks_total::int8, 'blocks'
    FROM pg_catalog.pg_stat_progress_create_index i
{% endif %}
{% if has("ProgressAnalyze") %}
    UNION ALL
    SELECT a.pid, a.datname, 'ANALYZE', a.relid, a.phase::text,
        a.sample_blks_scanned::int8, a.sample_blks_total::int8, 'blocks'
    FROM pg_catalog.pg_stat_progress_analyze a
{% endif %}
{% if has("ProgressBaseBackup") %}
    UNION ALL
    SELECT b.pid, NULL::name, 'BASE BACKUP', NULL::oid, b.phase::text,
        b.backup_streamed::int8, b.backup_total::int8, 'bytes'
    FROM pg_catalog.pg_stat_progress_basebackup b
{% endif %}
{% if has("ProgressCopy") %}
    UNION ALL
    SELECT k.pid, k.datname, k.command::text, k.relid, k.type::text,
        k.bytes_processed::int8, k.bytes_total::int8, 'bytes'
    FROM pg_catalog.pg_stat_progress_copy k
{% endif %}
)
SELECT
    p.pid::text AS pid,
    p.datname::text AS database,
    s.usename::text AS usename,
    p.command,
    CASE WHEN p.relid <> 0 THEN p.relid::regclass::text END AS relation,
    p.phase,
    p.done::text AS done,
    p.total::text AS total,
    p.unit,
    extract(epoch FROM pg_catalog.clock_timestamp() - s.query_start)::text AS running_secs,
    s.query
FROM p
    LEFT JOIN pg_catalog.pg_stat_activity s ON s.pid = p.pid
WHERE p.pid <> pg_catalog.pg_backend_pid()
    AND ($1::text IS NULL OR p.datname = $1::text)
ORDER BY s.query_start NULLS LAST, p.pid
