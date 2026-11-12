-- SQL profiler (D-058): the logging settings Start needs, as one row of text columns.
-- *_file: where a setting comes from (its file; 'default' for the built-in default;
-- empty when the user may not see it). No parameters.
SELECT
    pg_catalog.current_setting('logging_collector') AS logging_collector,
    d.setting AS log_destination,
    COALESCE(d.sourcefile, CASE WHEN d.source = 'default' THEN 'default' ELSE '' END)
        AS log_destination_file,
    m.setting AS log_min_duration_statement,
    COALESCE(m.sourcefile, CASE WHEN m.source = 'default' THEN 'default' ELSE '' END)
        AS log_min_duration_statement_file,
    pg_catalog.pg_backend_pid()::text AS own_pid
FROM pg_catalog.pg_settings d, pg_catalog.pg_settings m
WHERE d.name = 'log_destination'
    AND m.name = 'log_min_duration_statement'
