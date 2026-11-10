-- Tree nodes: row level security policies.
-- Adapted from pgAdmin 4 (databases/schemas/tables/row_security_policies/templates/row_security_policies/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = table oid
SELECT pol.oid, pol.polname AS name
FROM pg_catalog.pg_policy pol
WHERE pol.polrelid = $1::oid
ORDER BY pol.polname
