-- Server dashboard (D-057): Cancel query of a session. $1 = PID. One text column,
-- 'true' when the signal was sent.
SELECT pg_catalog.pg_cancel_backend($1::text::int4)::text
