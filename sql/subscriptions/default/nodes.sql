-- Tree nodes: subscriptions of the current database.
-- Adapted from pgAdmin 4 (databases/subscriptions/templates/subscriptions/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT s.oid, s.subname AS name
FROM pg_catalog.pg_subscription s
WHERE s.subdbid = (SELECT d.oid FROM pg_catalog.pg_database d WHERE d.datname = pg_catalog.current_database())
ORDER BY s.subname
