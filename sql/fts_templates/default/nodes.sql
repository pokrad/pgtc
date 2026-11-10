-- Tree nodes: FTS templates.
-- Adapted from pgAdmin 4 (databases/schemas/fts_templates/templates/fts_templates/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT tmpl.oid, tmpl.tmplname AS name
FROM pg_catalog.pg_ts_template tmpl
WHERE tmpl.tmplnamespace = $1::oid
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_ts_template'::regclass AND dep.objid = tmpl.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY tmpl.tmplname
