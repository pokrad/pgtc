-- Tree nodes: procedural languages.
-- Adapted from pgAdmin 4 (databases/languages/templates/languages/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT lan.oid, lan.lanname AS name
FROM pg_catalog.pg_language lan
{% if not show_system %}
WHERE lan.lanispl
{% endif %}
ORDER BY lan.lanname
