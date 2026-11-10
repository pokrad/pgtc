-- Tree nodes: catalogs (pg_catalog, information_schema), shown whatever the system
-- objects setting is.
-- Adapted from pgAdmin 4 (databases/schemas/templates/catalog/pg/default/sql/nodes.sql
-- and catalog/pg/macros/catalogs.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT nsp.oid, nsp.nspname AS name,
    CASE nsp.nspname
        WHEN 'pg_catalog' THEN 'PostgreSQL Catalog (pg_catalog)'
        WHEN 'information_schema' THEN 'ANSI (information_schema)'
    END AS label
FROM pg_catalog.pg_namespace nsp
WHERE (nsp.nspname = 'pg_catalog' AND EXISTS
        (SELECT 1 FROM pg_catalog.pg_class WHERE relname = 'pg_class' AND relnamespace = nsp.oid))
    OR (nsp.nspname = 'information_schema' AND EXISTS
        (SELECT 1 FROM pg_catalog.pg_class WHERE relname = 'tables' AND relnamespace = nsp.oid))
ORDER BY label
