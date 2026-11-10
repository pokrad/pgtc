-- Tree nodes: foreign tables.
-- Adapted from pgAdmin 4 (databases/schemas/foreign_tables/templates/foreign_tables/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT rel.oid, rel.relname AS name
FROM pg_catalog.pg_class rel
WHERE rel.relnamespace = $1::oid
    AND rel.relkind IN ('f')
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_class'::regclass AND dep.objid = rel.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY rel.relname
