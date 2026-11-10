-- Tree nodes: collations.
-- Adapted from pgAdmin 4 (databases/schemas/collations/templates/collations/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT c.oid, c.collname AS name
FROM pg_catalog.pg_collation c
WHERE c.collnamespace = $1::oid
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_collation'::regclass AND dep.objid = c.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY c.collname
