-- SQL profiler (D-058): the current log file of a format and the next part of the file
-- being read. Adapted from pgAdmin 4 (dashboard/templates/dashboard/sql/default/
-- logs.sql and log_stat.sql), Copyright (C) 2013 - 2026, The pgAdmin Development Team,
-- PostgreSQL Licence (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- $1 = format ('jsonlog' / 'csvlog'), $2 = file being read (NULL: only find the current
-- file and its size), $3 = offset, $4 = length (all text). Columns (text):
-- current_file (NULL: the server writes no such log), current_size, size of $2 (NULL:
-- the file is gone), data (hex).
SELECT
    f.cur AS current_file,
    (pg_catalog.pg_stat_file(f.cur, true)).size::text AS current_size,
    (pg_catalog.pg_stat_file($2::text, true)).size::text AS size,
    pg_catalog.encode(
        pg_catalog.pg_read_binary_file($2::text, $3::text::int8, $4::text::int8, true),
        'hex'
    ) AS data
FROM (SELECT pg_catalog.pg_current_logfile($1::text) AS cur) f
