-- Tree nodes: foreign data wrappers.
-- Adapted from pgAdmin 4 (databases/foreign_data_wrappers/templates/foreign_data_wrappers/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT fdw.oid, fdw.fdwname AS name
FROM pg_catalog.pg_foreign_data_wrapper fdw
ORDER BY fdw.fdwname
