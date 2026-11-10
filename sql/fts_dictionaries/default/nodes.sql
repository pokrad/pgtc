-- Tree nodes: FTS dictionaries.
-- Adapted from pgAdmin 4 (databases/schemas/fts_dictionaries/templates/fts_dictionaries/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT dict.oid, dict.dictname AS name
FROM pg_catalog.pg_ts_dict dict
WHERE dict.dictnamespace = $1::oid
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_ts_dict'::regclass AND dep.objid = dict.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY dict.dictname
