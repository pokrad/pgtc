-- Tree nodes: indexes.
-- Adapted from pgAdmin 4 (databases/schemas/tables/indexes/templates/indexes/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = table oid
SELECT cls.oid, cls.relname AS name
FROM pg_catalog.pg_index idx
    JOIN pg_catalog.pg_class cls ON cls.oid = idx.indexrelid
WHERE idx.indrelid = $1::oid
ORDER BY cls.relname
