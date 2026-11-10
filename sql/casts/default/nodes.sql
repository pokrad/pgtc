-- Tree nodes: casts.
-- Adapted from pgAdmin 4 (databases/casts/templates/casts/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. No parameters.
SELECT ca.oid,
    pg_catalog.format_type(ca.castsource, NULL) || '->' || pg_catalog.format_type(ca.casttarget, NULL) AS name
FROM pg_catalog.pg_cast ca
{% if not show_system %}
WHERE ca.oid >= 16384
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_cast'::regclass AND dep.objid = ca.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY name
