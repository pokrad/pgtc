-- Tree nodes: publications.
-- Adapted from pgAdmin 4 (databases/publications/templates/publications/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT p.oid, p.pubname AS name
FROM pg_catalog.pg_publication p
ORDER BY p.pubname
