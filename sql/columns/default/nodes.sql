-- Tree nodes: columns; the column number is returned as oid.
-- Adapted from pgAdmin 4 (databases/schemas/tables/columns/templates/columns/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = table oid
SELECT att.attnum::pg_catalog.int4::pg_catalog.oid AS oid, att.attname AS name,
    att.attname || ' (' || pg_catalog.format_type(att.atttypid, att.atttypmod) || ')' AS label
FROM pg_catalog.pg_attribute att
WHERE att.attrelid = $1::oid
    AND att.attnum > 0
    AND NOT att.attisdropped
ORDER BY att.attnum
