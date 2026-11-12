-- Server dashboard (D-057): Terminate session. $1 = PID. One text column, 'true' when
-- the signal was sent.
SELECT pg_catalog.pg_terminate_backend($1::text::int4)::text
