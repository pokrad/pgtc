-- Tree nodes: procedures.
-- Adapted from pgAdmin 4 (databases/schemas/functions/templates/procedures/pg/sql/default/node.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT pr.oid, pr.proname AS name,
    pr.proname || '(' || COALESCE(pg_catalog.pg_get_function_identity_arguments(pr.oid), '') || ')' AS label
FROM pg_catalog.pg_proc pr
WHERE pr.pronamespace = $1::oid
    AND pr.prokind = 'p'
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_proc'::regclass AND dep.objid = pr.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY pr.proname, label
