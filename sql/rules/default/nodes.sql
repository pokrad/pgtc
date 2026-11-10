-- Tree nodes: rules.
-- Adapted from pgAdmin 4 (databases/schemas/tables/rules/templates/rules/sql/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = table oid
SELECT rw.oid, rw.rulename AS name
FROM pg_catalog.pg_rewrite rw
WHERE rw.ev_class = $1::oid
{% if not show_system %}
    AND rw.rulename <> '_RETURN'
{% endif %}
ORDER BY rw.rulename
