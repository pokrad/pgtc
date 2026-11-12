-- SQL profiler (D-058): applies the changed logging settings (no restart needed for
-- log_destination and log_min_duration_statement). One text column.
SELECT pg_catalog.pg_reload_conf()::text
