-- Tree nodes: domains.
-- Adapted from pgAdmin 4 (databases/schemas/domains/templates/domains/sql/default/node.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT t.oid, t.typname AS name
FROM pg_catalog.pg_type t
WHERE t.typnamespace = $1::oid
    AND t.typtype = 'd'
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_type'::regclass AND dep.objid = t.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY t.typname
