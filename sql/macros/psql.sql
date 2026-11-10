{# The relation a psql-style \d names (D-054): $1 schema or NULL (search path), $2 name. #}
{% macro target() %}
WITH t AS (
    SELECT c.oid, c.relkind
    FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE c.relname = $2::text
        AND CASE WHEN $1::text IS NULL THEN pg_catalog.pg_table_is_visible(c.oid)
            ELSE n.nspname = $1::text END
        AND c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f')
)
{% endmacro %}
