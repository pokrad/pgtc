-- Find object (D-055): objects of the current database whose name matches the typed
-- text, with their place in the object tree.
-- $1  ILIKE pattern (contains / starts with / ends with; backslash escapes)
-- $2  scope: only objects whose path starts with it ('' = the whole database)
-- $3  'true' = columns too
-- Columns (text): type, schema ('' outside schemas), name (sub-objects as
-- `table.name`), path, sort (type order, to merge the results of several databases).
-- The path has one step per tree level below the database, `folder:oid` with the
-- folder of the object type under sql/ (`schemas:2200/tables:16384/columns:3`);
-- columns use their attnum.
-- Filters follow the tree's nodes.sql (system objects with show_system, D-036).
WITH RECURSIVE
nsp AS (
    SELECT n.oid, n.nspname,
        CASE WHEN (n.nspname = 'pg_catalog' AND EXISTS
                    (SELECT 1 FROM pg_catalog.pg_class WHERE relname = 'pg_class' AND relnamespace = n.oid))
                OR (n.nspname = 'information_schema' AND EXISTS
                    (SELECT 1 FROM pg_catalog.pg_class WHERE relname = 'tables' AND relnamespace = n.oid))
            THEN 'catalogs' ELSE 'schemas' END AS folder
    FROM pg_catalog.pg_namespace n
{% if not show_system %}
    WHERE n.nspname NOT LIKE 'pg\_%'
        AND n.nspname <> 'information_schema'
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_namespace'::regclass AND dep.objid = n.oid AND dep.deptype = 'e')
{% endif %}
),
-- relations with their path; partitions below their parent (Partitions group)
rel AS (
    SELECT c.oid, c.relname, c.relkind, nsp.nspname,
        nsp.folder || ':' || nsp.oid || '/' ||
        CASE c.relkind WHEN 'v' THEN 'views' WHEN 'm' THEN 'mviews'
            WHEN 'f' THEN 'foreign_tables' WHEN 'S' THEN 'sequences' ELSE 'tables' END
        || ':' || c.oid AS path,
        false AS partition
    FROM pg_catalog.pg_class c
        JOIN nsp ON nsp.oid = c.relnamespace
    WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f', 'S')
        AND NOT c.relispartition
{% if not show_system %}
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_class'::regclass AND dep.objid = c.oid AND dep.deptype = 'e')
{% endif %}
    UNION ALL
    SELECT c.oid, c.relname, c.relkind, n.nspname,
        rel.path || '/partitions:' || c.oid, true
    FROM pg_catalog.pg_inherits i
        JOIN rel ON rel.oid = i.inhparent
        JOIN pg_catalog.pg_class c ON c.oid = i.inhrelid AND c.relispartition
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
),
hits AS (
    SELECT CASE WHEN nsp.folder = 'catalogs' THEN 'catalog' ELSE 'schema' END AS type,
        ''::text AS schema, nsp.nspname::text AS name, nsp.folder || ':' || nsp.oid AS path, 1 AS sort
    FROM nsp
    WHERE nsp.nspname ILIKE $1::text
    UNION ALL
    SELECT CASE WHEN rel.partition THEN 'partition' ELSE CASE rel.relkind
            WHEN 'v' THEN 'view' WHEN 'm' THEN 'materialized view'
            WHEN 'f' THEN 'foreign table' WHEN 'S' THEN 'sequence' ELSE 'table' END END,
        rel.nspname::text, rel.relname::text, rel.path,
        CASE WHEN rel.partition THEN 3 ELSE CASE rel.relkind WHEN 'v' THEN 4 WHEN 'm' THEN 5
            WHEN 'f' THEN 6 WHEN 'S' THEN 7 ELSE 2 END END
    FROM rel
    WHERE rel.relname ILIKE $1::text
    UNION ALL
    SELECT CASE WHEN p.prokind = 'p' THEN 'procedure' WHEN p.prokind = 'a' THEN 'aggregate'
            WHEN p.prorettype IN ('pg_catalog.trigger'::pg_catalog.regtype, 'pg_catalog.event_trigger'::pg_catalog.regtype)
            THEN 'trigger function' ELSE 'function' END,
        nsp.nspname::text, p.proname || '(' || COALESCE(pg_catalog.pg_get_function_identity_arguments(p.oid), '') || ')',
        nsp.folder || ':' || nsp.oid || '/' ||
        CASE WHEN p.prokind = 'p' THEN 'procedures' WHEN p.prokind = 'a' THEN 'aggregates'
            WHEN p.prorettype IN ('pg_catalog.trigger'::pg_catalog.regtype, 'pg_catalog.event_trigger'::pg_catalog.regtype)
            THEN 'trigger_functions' ELSE 'functions' END || ':' || p.oid,
        CASE WHEN p.prokind = 'p' THEN 9 WHEN p.prokind = 'a' THEN 11
            WHEN p.prorettype IN ('pg_catalog.trigger'::pg_catalog.regtype, 'pg_catalog.event_trigger'::pg_catalog.regtype)
            THEN 10 ELSE 8 END
    FROM pg_catalog.pg_proc p
        JOIN nsp ON nsp.oid = p.pronamespace
    WHERE p.proname ILIKE $1::text
        AND p.prokind IN ('f', 'p', 'a')
{% if not show_system %}
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_proc'::regclass AND dep.objid = p.oid AND dep.deptype = 'e')
{% endif %}
    UNION ALL
    SELECT CASE WHEN t.typtype = 'd' THEN 'domain' ELSE 'type' END,
        nsp.nspname::text, t.typname::text,
        nsp.folder || ':' || nsp.oid || '/' || CASE WHEN t.typtype = 'd' THEN 'domains' ELSE 'types' END || ':' || t.oid,
        CASE WHEN t.typtype = 'd' THEN 13 ELSE 12 END
    FROM pg_catalog.pg_type t
        JOIN nsp ON nsp.oid = t.typnamespace
    WHERE t.typname ILIKE $1::text
        AND (t.typtype = 'd'
            OR (t.typtype IN ('b', 'c', 'e', 'r')
                AND (t.typrelid = 0
                     OR (SELECT c.relkind = 'c' FROM pg_catalog.pg_class c WHERE c.oid = t.typrelid))
                AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_type el WHERE el.typarray = t.oid)))
{% if not show_system %}
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_type'::regclass AND dep.objid = t.oid AND dep.deptype = 'e')
{% endif %}
    UNION ALL
    SELECT 'collation', nsp.nspname::text, co.collname::text,
        nsp.folder || ':' || nsp.oid || '/collations:' || co.oid, 14
    FROM pg_catalog.pg_collation co
        JOIN nsp ON nsp.oid = co.collnamespace
    WHERE co.collname ILIKE $1::text
{% if not show_system %}
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_collation'::regclass AND dep.objid = co.oid AND dep.deptype = 'e')
{% endif %}
    UNION ALL
    SELECT 'index', rel.nspname::text, rel.relname || '.' || ic.relname,
        rel.path || '/indexes:' || ic.oid, 15
    FROM pg_catalog.pg_index i
        JOIN rel ON rel.oid = i.indrelid AND rel.relkind IN ('r', 'p', 'm')
        JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
    WHERE ic.relname ILIKE $1::text
    UNION ALL
    SELECT 'constraint', rel.nspname::text, rel.relname || '.' || con.conname,
        rel.path || '/constraints:' || con.oid, 16
    FROM pg_catalog.pg_constraint con
        JOIN rel ON rel.oid = con.conrelid AND rel.relkind IN ('r', 'p')
    WHERE con.conname ILIKE $1::text
        AND con.contype IN ('p', 'u', 'f', 'c', 'x')
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint pc WHERE pc.oid = con.conparentid AND pc.conrelid = con.conrelid)
    UNION ALL
    SELECT 'trigger', rel.nspname::text, rel.relname || '.' || tg.tgname,
        rel.path || '/triggers:' || tg.oid, 17
    FROM pg_catalog.pg_trigger tg
        JOIN rel ON rel.oid = tg.tgrelid AND rel.relkind IN ('r', 'p', 'v')
    WHERE tg.tgname ILIKE $1::text
{% if not show_system %}
        AND NOT tg.tgisinternal
{% endif %}
    UNION ALL
    SELECT 'rule', rel.nspname::text, rel.relname || '.' || rw.rulename,
        rel.path || '/rules:' || rw.oid, 18
    FROM pg_catalog.pg_rewrite rw
        JOIN rel ON rel.oid = rw.ev_class AND rel.relkind IN ('r', 'p', 'v')
    WHERE rw.rulename ILIKE $1::text
{% if not show_system %}
        AND rw.rulename <> '_RETURN'
{% endif %}
    UNION ALL
    SELECT 'policy', rel.nspname::text, rel.relname || '.' || pol.polname,
        rel.path || '/policies:' || pol.oid, 19
    FROM pg_catalog.pg_policy pol
        JOIN rel ON rel.oid = pol.polrelid AND rel.relkind IN ('r', 'p')
    WHERE pol.polname ILIKE $1::text
    UNION ALL
    SELECT 'extension', ''::text, x.extname::text, 'extensions:' || x.oid, 20
    FROM pg_catalog.pg_extension x
    WHERE x.extname ILIKE $1::text
    UNION ALL
    SELECT 'event trigger', ''::text, e.evtname::text, 'event_triggers:' || e.oid, 21
    FROM pg_catalog.pg_event_trigger e
    WHERE e.evtname ILIKE $1::text
{% if not show_system %}
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep WHERE dep.classid = 'pg_catalog.pg_event_trigger'::regclass AND dep.objid = e.oid AND dep.deptype = 'e')
{% endif %}
    UNION ALL
    SELECT 'publication', ''::text, pb.pubname::text, 'publications:' || pb.oid, 22
    FROM pg_catalog.pg_publication pb
    WHERE pb.pubname ILIKE $1::text
    UNION ALL
    SELECT 'language', ''::text, l.lanname::text, 'languages:' || l.oid, 23
    FROM pg_catalog.pg_language l
    WHERE l.lanname ILIKE $1::text
{% if not show_system %}
        AND l.lanispl
{% endif %}
    UNION ALL
    SELECT 'foreign data wrapper', ''::text, w.fdwname::text, 'foreign_data_wrappers:' || w.oid, 24
    FROM pg_catalog.pg_foreign_data_wrapper w
    WHERE w.fdwname ILIKE $1::text
    UNION ALL
    SELECT 'column', rel.nspname::text, rel.relname || '.' || a.attname,
        rel.path || '/columns:' || a.attnum, 25
    FROM pg_catalog.pg_attribute a
        JOIN rel ON rel.oid = a.attrelid AND rel.relkind IN ('r', 'p', 'v', 'm', 'f')
    WHERE $3::text::boolean
        AND a.attname ILIKE $1::text
        AND a.attnum > 0
        AND NOT a.attisdropped
)
SELECT type, schema, name, path, sort::text
FROM hits
WHERE pg_catalog.left(path, pg_catalog.length($2::text)) = $2::text
ORDER BY sort, schema, name, path
LIMIT 201
