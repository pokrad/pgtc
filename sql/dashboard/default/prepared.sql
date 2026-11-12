-- Server dashboard (D-057): the Prepared transactions list.
-- Adapted from pgAdmin 4 (dashboard/templates/dashboard/sql/default/prepared.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- $1 = database name, or NULL for the whole server. All columns are text.
SELECT
    p.gid,
    p.database::text AS database,
    p.owner::text AS owner,
    p.transaction::text AS transaction,
    pg_catalog.to_char(p.prepared, 'YYYY-MM-DD HH24:MI:SS') AS prepared
FROM pg_catalog.pg_prepared_xacts p
WHERE $1::text IS NULL OR p.database = $1::text
ORDER BY p.gid, p.database, p.owner
