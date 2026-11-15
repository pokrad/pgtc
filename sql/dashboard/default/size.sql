-- Server dashboard (D-057): size of the databases for the summary strip (read at most
-- every 30 s: pg_database_size is expensive). $1 = database name, or NULL for the
-- whole server. Databases the user may not connect to are skipped. Text columns: bytes,
-- server clock (epoch seconds) for the growth rate.
SELECT
    COALESCE(sum(pg_catalog.pg_database_size(d.oid)), 0)::text AS bytes,
    extract(epoch FROM pg_catalog.clock_timestamp())::text AS t
FROM pg_catalog.pg_database d
WHERE pg_catalog.has_database_privilege(d.oid, 'CONNECT')
    AND ($1::text IS NULL OR d.datname = $1::text)
