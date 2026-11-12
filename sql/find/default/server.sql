-- Find object (D-055) on a server row: databases, roles and tablespaces whose name
-- contains the typed text. Parameters and columns as in database.sql ($3 unused);
-- paths start with the server-level group (`databases:16384`).
-- Filters follow the tree's nodes.sql (system objects with show_system, D-036).
WITH hits AS (
    SELECT 'database' AS type, ''::text AS schema, db.datname::text AS name, 'databases:' || db.oid AS path, -3 AS sort
    FROM pg_catalog.pg_database db
    WHERE db.datname ILIKE $1::text
{% if not show_system %}
        AND NOT db.datistemplate
{% endif %}
    UNION ALL
    SELECT 'role', ''::text, r.rolname::text, 'roles:' || r.oid, -2
    FROM pg_catalog.pg_roles r
    WHERE r.rolname ILIKE $1::text
{% if not show_system %}
        AND r.rolname NOT LIKE 'pg\_%'
{% endif %}
    UNION ALL
    SELECT 'tablespace', ''::text, ts.spcname::text, 'tablespaces:' || ts.oid, -1
    FROM pg_catalog.pg_tablespace ts
    WHERE ts.spcname ILIKE $1::text
)
SELECT type, schema, name, path, sort::text
FROM hits
WHERE $3::text IS NOT NULL
    AND pg_catalog.left(path, pg_catalog.length($2::text)) = $2::text
ORDER BY sort, name
LIMIT 201
