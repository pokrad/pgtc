-- Server dashboard (D-057): Cancel query of a session. $1 = PID, $2 = its backend start as the
-- Sessions list shows it (NULL: not known, not checked). One text column, 'true' when
-- the signal was sent; no row when the session is gone — also when another session
-- has its PID by now.
SELECT pg_catalog.pg_cancel_backend(s.pid)::text
FROM pg_catalog.pg_stat_activity s
WHERE s.pid = $1::text::int4
    AND ($2::text IS NULL
        OR pg_catalog.to_char(s.backend_start, 'YYYY-MM-DD HH24:MI:SS') = $2::text)
