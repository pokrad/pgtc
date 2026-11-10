-- Tree nodes: types (no domains, arrays, pseudo or table row types).
-- Adapted from pgAdmin 4 (databases/schemas/types/templates/types/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT t.oid, t.typname AS name, pg_catalog.format_type(t.oid, NULL) AS label
FROM pg_catalog.pg_type t
WHERE t.typnamespace = $1::oid
    AND t.typtype IN ('b', 'c', 'e', 'r')
    AND (t.typrelid = 0
         OR (SELECT c.relkind = 'c' FROM pg_catalog.pg_class c WHERE c.oid = t.typrelid))
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_type el WHERE el.typarray = t.oid)
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_type'::regclass AND dep.objid = t.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY t.typname
