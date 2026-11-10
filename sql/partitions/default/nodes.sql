-- Tree nodes: partitions of a partitioned table.
-- Adapted from pgAdmin 4 (databases/schemas/tables/partitions/templates/partitions/sql/pg/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = table oid
SELECT rel.oid, rel.relname AS name,
    CASE WHEN rel.relnamespace = parent.relnamespace THEN rel.relname
         ELSE nsp.nspname || '.' || rel.relname END AS label,
    rel.relkind = 'p' AS partitioned
FROM pg_catalog.pg_inherits inh
    JOIN pg_catalog.pg_class rel ON rel.oid = inh.inhrelid
    JOIN pg_catalog.pg_class parent ON parent.oid = inh.inhparent
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
WHERE inh.inhparent = $1::oid
    AND rel.relispartition
ORDER BY rel.relname
