-- Tree nodes: tablespaces.
-- Adapted from pgAdmin 4 (tablespaces/templates/tablespaces/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT ts.oid, ts.spcname AS name
FROM pg_catalog.pg_tablespace ts
ORDER BY ts.spcname
