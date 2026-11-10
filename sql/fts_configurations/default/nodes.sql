-- Tree nodes: FTS configurations.
-- Adapted from pgAdmin 4 (databases/schemas/fts_configurations/templates/fts_configurations/sql/default/nodes.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Columns: oid, name [, label] [, partitioned]. $1 = schema oid
SELECT cfg.oid, cfg.cfgname AS name
FROM pg_catalog.pg_ts_config cfg
WHERE cfg.cfgnamespace = $1::oid
{% if not show_system %}
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_ts_config'::regclass AND dep.objid = cfg.oid AND dep.deptype = 'e')
{% endif %}
ORDER BY cfg.cfgname
