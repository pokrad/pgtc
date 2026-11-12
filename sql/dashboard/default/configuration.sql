-- Server dashboard (D-057): the Configuration list (the same for the whole server).
-- Adapted from pgAdmin 4 (dashboard/templates/dashboard/sql/default/config.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- No parameters. All columns are text.
SELECT
    s.name,
    s.category,
    s.setting,
    s.unit,
    s.short_desc
FROM pg_catalog.pg_settings s
ORDER BY s.category, s.name
