-- Tree nodes: triggers.
-- Adapted from pgAdmin 4 (databases/schemas/tables/triggers/templates/triggers/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = table oid
SELECT t.oid, t.tgname AS name
FROM pg_catalog.pg_trigger t
WHERE t.tgrelid = $1::oid
{% if not show_system %}
    AND NOT t.tgisinternal
{% endif %}
ORDER BY t.tgname
