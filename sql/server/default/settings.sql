-- Server configuration on the server row (D-037): every setting the user may see,
-- sorted by name. All columns are text; sourcefile / sourceline are NULL unless the
-- user may read them (superuser, pg_read_all_settings).
SELECT s.name,
    pg_catalog.current_setting(s.name) AS value,
    s.source,
    s.sourcefile,
    s.sourceline::text AS sourceline,
    CASE WHEN s.pending_restart THEN 't' ELSE 'f' END AS pending_restart
FROM pg_catalog.pg_settings s
ORDER BY s.name
