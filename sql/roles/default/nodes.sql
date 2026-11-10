-- Tree nodes: login and group roles.
-- Adapted from pgAdmin 4 (roles/templates/roles/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT r.oid, r.rolname AS name,
    CASE WHEN r.rolcanlogin THEN r.rolname ELSE r.rolname || ' (group)' END AS label
FROM pg_catalog.pg_roles r
{% if not show_system %}
WHERE r.rolname NOT LIKE 'pg\_%'
{% endif %}
ORDER BY r.rolname
