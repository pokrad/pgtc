-- SQL profiler (D-058): the logging settings Start needs, as one row of text columns.
-- *_file: where a setting comes from: its file; 'default' for the built-in default;
-- 'other' when the value of this session is not the server's (ALTER ROLE /
-- DATABASE ... SET, the server's command line, ...); empty when it is from a file
-- the user may not see. No parameters.
SELECT
    pg_catalog.current_setting('logging_collector') AS logging_collector,
    d.setting AS log_destination,
    COALESCE(d.sourcefile, CASE d.source WHEN 'default' THEN 'default'
        WHEN 'configuration file' THEN '' ELSE 'other' END) AS log_destination_file,
    m.setting AS log_min_duration_statement,
    COALESCE(m.sourcefile, CASE m.source WHEN 'default' THEN 'default'
        WHEN 'configuration file' THEN '' ELSE 'other' END) AS log_min_duration_statement_file,
    pg_catalog.pg_backend_pid()::text AS own_pid
FROM pg_catalog.pg_settings d, pg_catalog.pg_settings m
WHERE d.name = 'log_destination'
    AND m.name = 'log_min_duration_statement'
