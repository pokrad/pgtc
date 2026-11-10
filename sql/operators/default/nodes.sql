-- Tree nodes: operators.
-- Adapted from pgAdmin 4 (databases/schemas/operators/templates/operators/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT o.oid, o.oprname AS name,
    o.oprname || ' (' || COALESCE(pg_catalog.format_type(NULLIF(o.oprleft, 0), NULL), 'NONE') || ', '
        || COALESCE(pg_catalog.format_type(NULLIF(o.oprright, 0), NULL), 'NONE') || ')' AS label
FROM pg_catalog.pg_operator o
WHERE o.oprnamespace = $1::oid
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_operator'::regclass AND dep.objid = o.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY o.oprname, label
