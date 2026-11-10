-- Object definition: table or partition, with its indexes, policies, triggers and rules.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/tables/templates/tables/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT CASE WHEN rel.relispartition THEN 'Partition' ELSE 'Table' END AS kind, x.q AS name,
    'DROP TABLE IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE ' || CASE WHEN rel.relpersistence = 'u' THEN 'UNLOGGED ' ELSE '' END
            || 'TABLE IF NOT EXISTS ' || x.q
            || CASE WHEN rel.relispartition
                    THEN E'\n    PARTITION OF ' || COALESCE(x.parent, '?')
                         || COALESCE(E'\n(\n' || x.items || E'\n)', '')
                         || E'\n    ' || pg_catalog.pg_get_expr(rel.relpartbound, rel.oid)
                    ELSE E'\n(\n' || COALESCE(x.items, '') || E'\n)' || COALESCE(x.inherits, '') END
            || CASE WHEN rel.relkind = 'p' THEN E'\nPARTITION BY ' || pg_catalog.pg_get_partkeydef(rel.oid) ELSE '' END
            || CASE WHEN rel.reloptions IS NOT NULL
                    THEN E'\nWITH (' || pg_catalog.array_to_string(rel.reloptions, ', ') || ')' ELSE '' END
            || E'\nTABLESPACE ' || COALESCE(pg_catalog.quote_ident(ts.spcname), 'pg_default') || ';',
        'ALTER TABLE IF EXISTS ' || x.q || E'\n    OWNER to ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(rel.relowner)) || ';',
        CASE WHEN rel.relrowsecurity THEN 'ALTER TABLE IF EXISTS ' || x.q || ' ENABLE ROW LEVEL SECURITY;' END,
        CASE WHEN rel.relforcerowsecurity THEN 'ALTER TABLE IF EXISTS ' || x.q || ' FORCE ROW LEVEL SECURITY;' END,
        'COMMENT ON TABLE ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(rel.oid, 'pg_class')) || ';',
        (SELECT pg_catalog.string_agg('COMMENT ON COLUMN ' || x.q || '.' || pg_catalog.quote_ident(a.attname)
                || E'\n    IS ' || pg_catalog.quote_literal(d.description) || ';', E'\n\n' ORDER BY a.attnum)
            FROM pg_catalog.pg_attribute a
                JOIN pg_catalog.pg_description d ON d.objoid = a.attrelid
                    AND d.classoid = 'pg_catalog.pg_class'::regclass AND d.objsubid = a.attnum
            WHERE a.attrelid = rel.oid AND a.attnum > 0 AND NOT a.attisdropped),
        {{ acl.grants("rel.relacl", "'TABLE'", "x.q") }},
        {{ parts.policies("rel.oid", "x.q") }},
        {{ parts.indexes("rel.oid", "nsp.nspname") }},
        {{ parts.triggers("rel.oid", "x.q") }},
        {{ parts.rules("rel.oid", "x.q") }}
    ) AS body
FROM pg_catalog.pg_class rel
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace
    LEFT JOIN pg_catalog.pg_tablespace ts ON ts.oid = rel.reltablespace
    CROSS JOIN LATERAL (SELECT
        pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(rel.relname) AS q,
        (SELECT pg_catalog.quote_ident(pn.nspname) || '.' || pg_catalog.quote_ident(pc.relname)
            FROM pg_catalog.pg_inherits i
                JOIN pg_catalog.pg_class pc ON pc.oid = i.inhparent
                JOIN pg_catalog.pg_namespace pn ON pn.oid = pc.relnamespace
            WHERE i.inhrelid = rel.oid ORDER BY i.inhseqno LIMIT 1) AS parent,
        (SELECT E'\nINHERITS (' || pg_catalog.string_agg(pg_catalog.quote_ident(pn.nspname) || '.'
                    || pg_catalog.quote_ident(pc.relname), ', ' ORDER BY i.inhseqno) || ')'
            FROM pg_catalog.pg_inherits i
                JOIN pg_catalog.pg_class pc ON pc.oid = i.inhparent
                JOIN pg_catalog.pg_namespace pn ON pn.oid = pc.relnamespace
            WHERE i.inhrelid = rel.oid) AS inherits,
        (SELECT pg_catalog.string_agg(it.line, E',\n' ORDER BY it.ord, it.sub) FROM (
            SELECT 1 AS ord, a.attnum::int AS sub, '    ' || {{ col.coldef(has("GeneratedColumns")) }} AS line
            FROM pg_catalog.pg_attribute a
                {{ col.coljoins() }}
            WHERE a.attrelid = rel.oid AND a.attnum > 0 AND NOT a.attisdropped AND a.attislocal
            UNION ALL
            SELECT 2, (pg_catalog.row_number() OVER (ORDER BY CASE c.contype WHEN 'p' THEN 1 WHEN 'u' THEN 2
                                                     WHEN 'x' THEN 3 WHEN 'f' THEN 4 ELSE 5 END, c.conname))::int,
                '    CONSTRAINT ' || pg_catalog.quote_ident(c.conname) || ' ' || pg_catalog.pg_get_constraintdef(c.oid, true)
            FROM pg_catalog.pg_constraint c
            WHERE c.conrelid = rel.oid AND c.contype IN ('p', 'u', 'f', 'c', 'x') AND c.conislocal
                AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint pc WHERE pc.oid = c.conparentid AND pc.conrelid = c.conrelid)
        ) it) AS items
    ) x
WHERE rel.oid = $1::oid
