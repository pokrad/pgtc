-- Tree nodes: FTS parsers.
-- Adapted from pgAdmin 4 (databases/schemas/fts_parsers/templates/fts_parsers/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT prs.oid, prs.prsname AS name
FROM pg_catalog.pg_ts_parser prs
WHERE prs.prsnamespace = $1::oid
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_ts_parser'::regclass AND dep.objid = prs.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY prs.prsname
