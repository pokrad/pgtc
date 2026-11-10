-- Structure model (D-048): dependencies between top-level model objects, for ordering
-- the sync script. From pg_depend (normal dependencies only):
-- - a view depends through its _RETURN rule, a column default through pg_attrdef, a
--   check constraint through pg_constraint (foreign keys are left out: they are added
--   after all tables), a domain constraint through its domain, a composite type
--   through its relation;
-- - a reference to a table's row type or to an array type counts as a reference to the
--   table / element type.
-- Rows: kind, schema, name, args, ref_kind, ref_schema, ref_name, ref_args.
-- Runs with search_path = ''.
{% import "macros/model.sql" as m %}
WITH objs AS (
    SELECT 'pg_catalog.pg_type'::pg_catalog.regclass AS cls, t.oid, 'type' AS kind,
        n.nspname AS schema, t.typname AS name, NULL::text AS args, t.typrelid AS rel,
        t.typarray AS arr
    FROM pg_catalog.pg_type t
        JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace
        LEFT JOIN pg_catalog.pg_class rel ON rel.oid = t.typrelid
    WHERE {{ m.user_schema("n") }}
        AND (t.typtype IN ('e', 'd', 'r') OR (t.typtype = 'c' AND rel.relkind = 'c'))
    UNION ALL
    SELECT 'pg_catalog.pg_class'::pg_catalog.regclass, c.oid,
        CASE c.relkind WHEN 'S' THEN 'sequence' WHEN 'v' THEN 'view'
                       WHEN 'm' THEN 'materialized_view' ELSE 'table' END,
        n.nspname, c.relname, NULL, 0, ty.typarray
    FROM pg_catalog.pg_class c
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
        LEFT JOIN pg_catalog.pg_type ty ON ty.oid = c.reltype
    WHERE c.relkind IN ('r', 'p', 'v', 'm', 'S') AND {{ m.user_schema("n") }}
    UNION ALL
    SELECT 'pg_catalog.pg_proc'::pg_catalog.regclass, p.oid, 'function',
        n.nspname, p.proname, pg_catalog.pg_get_function_identity_arguments(p.oid), 0, 0
    FROM pg_catalog.pg_proc p
        JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
    WHERE p.prokind IN ('f', 'p', 'w') AND {{ m.user_schema("n") }}
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend xi
            WHERE xi.classid = 'pg_catalog.pg_proc'::pg_catalog.regclass
                AND xi.objid = p.oid AND xi.deptype = 'i')
),
-- every catalog entry that stands for a model object
alias AS (
    SELECT cls, oid, kind, schema, name, args FROM objs
    -- composite type: its pg_class entry
    UNION ALL SELECT 'pg_catalog.pg_class'::pg_catalog.regclass, rel, kind, schema, name, args
        FROM objs WHERE kind = 'type' AND rel <> 0
    -- array types of types and relations
    UNION ALL SELECT 'pg_catalog.pg_type'::pg_catalog.regclass, arr, kind, schema, name, args
        FROM objs WHERE arr IS NOT NULL AND arr <> 0
    -- row type of a table / view
    UNION ALL SELECT 'pg_catalog.pg_type'::pg_catalog.regclass, c.reltype, o.kind, o.schema, o.name, o.args
        FROM objs o JOIN pg_catalog.pg_class c ON c.oid = o.oid
        WHERE o.cls = 'pg_catalog.pg_class'::pg_catalog.regclass AND c.reltype <> 0
),
-- the model object a dependent catalog entry belongs to (owner_cls, owner_oid)
dep AS (
    SELECT d.refclassid, d.refobjid,
        CASE WHEN x.rel_owner IS NOT NULL THEN 'pg_catalog.pg_class'::pg_catalog.regclass
             WHEN x.type_owner IS NOT NULL THEN 'pg_catalog.pg_type'::pg_catalog.regclass
             ELSE d.classid END AS owner_cls,
        COALESCE(x.rel_owner, x.type_owner, d.objid) AS owner_oid
    FROM pg_catalog.pg_depend d
        CROSS JOIN LATERAL (SELECT
            CASE d.classid
                WHEN 'pg_catalog.pg_rewrite'::pg_catalog.regclass THEN
                    (SELECT rw.ev_class FROM pg_catalog.pg_rewrite rw
                     WHERE rw.oid = d.objid AND rw.rulename = '_RETURN')
                WHEN 'pg_catalog.pg_attrdef'::pg_catalog.regclass THEN
                    (SELECT ad.adrelid FROM pg_catalog.pg_attrdef ad WHERE ad.oid = d.objid)
                WHEN 'pg_catalog.pg_constraint'::pg_catalog.regclass THEN
                    (SELECT k.conrelid FROM pg_catalog.pg_constraint k
                     WHERE k.oid = d.objid AND k.contype = 'c' AND k.conrelid <> 0)
                END AS rel_owner,
            CASE WHEN d.classid = 'pg_catalog.pg_constraint'::pg_catalog.regclass THEN
                    (SELECT k.contypid FROM pg_catalog.pg_constraint k
                     WHERE k.oid = d.objid AND k.contypid <> 0)
                END AS type_owner) x
    WHERE d.deptype = 'n'
        AND d.refclassid IN ('pg_catalog.pg_type'::pg_catalog.regclass,
                             'pg_catalog.pg_class'::pg_catalog.regclass,
                             'pg_catalog.pg_proc'::pg_catalog.regclass)
)
SELECT DISTINCT a.kind, a.schema, a.name, a.args,
    b.kind AS ref_kind, b.schema AS ref_schema, b.name AS ref_name, b.args AS ref_args
FROM dep
    JOIN alias a ON a.cls = dep.owner_cls AND a.oid = dep.owner_oid
    JOIN alias b ON b.cls = dep.refclassid AND b.oid = dep.refobjid
WHERE (a.kind, a.schema, a.name, a.args) IS DISTINCT FROM (b.kind, b.schema, b.name, b.args)
