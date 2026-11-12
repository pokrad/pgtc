-- Server dashboard (D-057): the Locks list.
-- Adapted from pgAdmin 4 (dashboard/templates/dashboard/sql/default/locks.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- $1 = database name, or NULL for the whole server. All columns are text; the locks of
-- the dashboard's own backend are left out.
SELECT
    l.pid::text AS pid,
    l.locktype,
    d.datname::text AS database,
    l.relation::regclass::text AS relation,
    l.page::text AS page,
    l.tuple::text AS tuple,
    l.virtualxid,
    l.transactionid::text AS transactionid,
    l.classid::regclass::text AS classid,
    l.objid::text AS objid,
    l.objsubid::text AS objsubid,
    l.virtualtransaction,
    l.mode,
    CASE WHEN l.granted THEN 'yes' ELSE 'no' END AS granted,
    CASE WHEN l.fastpath THEN 'yes' ELSE 'no' END AS fastpath
FROM pg_catalog.pg_locks l
    LEFT OUTER JOIN pg_catalog.pg_database d ON l.database = d.oid
WHERE l.pid IS DISTINCT FROM pg_catalog.pg_backend_pid()
    AND ($1::text IS NULL OR d.datname = $1::text)
ORDER BY l.pid, l.locktype
