-- Tree nodes: schemas.
-- Adapted from pgAdmin 4 (databases/schemas/templates/schemas/pg/default/sql/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
-- The catalogs (pg_catalog, information_schema) have their own group (D-036).
SELECT nsp.oid, nsp.nspname AS name
FROM pg_catalog.pg_namespace nsp
WHERE NOT ((nsp.nspname = 'pg_catalog' AND EXISTS
        (SELECT 1 FROM pg_catalog.pg_class WHERE relname = 'pg_class' AND relnamespace = nsp.oid))
    OR (nsp.nspname = 'information_schema' AND EXISTS
        (SELECT 1 FROM pg_catalog.pg_class WHERE relname = 'tables' AND relnamespace = nsp.oid)))
{% if not show_system %}
    AND nsp.nspname NOT LIKE 'pg\_%'
    AND nsp.nspname <> 'information_schema'
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_namespace'::regclass AND dep.objid = nsp.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY nsp.nspname
