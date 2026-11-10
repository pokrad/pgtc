-- Tree nodes: databases.
-- Adapted from pgAdmin 4 (databases/templates/databases/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT db.oid, db.datname AS name
FROM pg_catalog.pg_database db
{% if not show_system %}
WHERE NOT db.datistemplate
{% endif %}
ORDER BY db.datname
