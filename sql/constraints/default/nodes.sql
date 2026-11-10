-- Tree nodes: table constraints.
-- Adapted from pgAdmin 4 (databases/schemas/tables/constraints/templates/*/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = table oid
SELECT con.oid, con.conname AS name,
    con.conname || ' (' || CASE con.contype
        WHEN 'p' THEN 'primary key'
        WHEN 'u' THEN 'unique'
        WHEN 'f' THEN 'foreign key'
        WHEN 'c' THEN 'check'
        WHEN 'x' THEN 'exclusion'
        ELSE con.contype::text END || ')' AS label
FROM pg_catalog.pg_constraint con
WHERE con.conrelid = $1::oid
    AND con.contype IN ('p', 'u', 'f', 'c', 'x')
    -- FKs to a partitioned table get hidden copies on the same table: skip them
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint pc WHERE pc.oid = con.conparentid AND pc.conrelid = con.conrelid)
ORDER BY con.conname
