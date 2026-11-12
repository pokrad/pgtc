-- Server dashboard (D-057): one row of counters for the graphs.
-- Adapted from pgAdmin 4 (dashboard/templates/dashboard/sql/default/dashboard_stats.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- $1 = database name, or NULL for the whole server. All columns are text; the
-- dashboard's own backend is not counted. `t` is the server clock (epoch seconds) for
-- the rates.
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
    extract(epoch FROM pg_catalog.clock_timestamp())::text AS t
FROM (
    SELECT count(*) AS total,
        count(*) FILTER (WHERE s.state = 'active') AS active,
        count(*) FILTER (WHERE s.state = 'idle') AS idle
    FROM pg_catalog.pg_stat_activity s
    WHERE s.backend_type = 'client backend'
        AND s.pid <> pg_catalog.pg_backend_pid()
        AND ($1::text IS NULL OR s.datname = $1::text)
) a, (
    SELECT sum(xact_commit) AS commits,
        sum(xact_rollback) AS rollbacks,
        sum(tup_inserted) AS inserted,
        sum(tup_updated) AS updated,
        sum(tup_deleted) AS deleted,
        sum(tup_fetched) AS fetched,
        sum(tup_returned) AS returned,
        sum(blks_read) AS blks_read,
        sum(blks_hit) AS blks_hit
    FROM pg_catalog.pg_stat_database
    WHERE $1::text IS NULL OR datname = $1::text
) d
