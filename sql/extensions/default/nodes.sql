-- Tree nodes: extensions.
-- Adapted from pgAdmin 4 (databases/extensions/templates/extensions/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT x.oid, x.extname AS name
FROM pg_catalog.pg_extension x
ORDER BY x.extname
