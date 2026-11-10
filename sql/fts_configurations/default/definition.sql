-- Object definition: text search configuration with its token mappings.
-- Assembled by pgtc from the pg_get_* functions and the catalogs; structure follows
-- pgAdmin 4's SQL tab (databases/schemas/fts_configurations/templates/fts_configurations/sql/default/create.sql),
-- Copyright (C) 2013 - 2026, The pgAdmin Development Team, PostgreSQL Licence
-- (see THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt).
-- Returns kind, name, drop, body. $1 = object oid.
{% import "macros/acl.sql" as acl %}
{% import "macros/columns.sql" as col %}
{% import "macros/relation_parts.sql" as parts %}
SELECT 'FTS Configuration' AS kind, x.q AS name,
    'DROP TEXT SEARCH CONFIGURATION IF EXISTS ' || x.q || ';' AS drop,
    pg_catalog.concat_ws(E'\n\n',
        'CREATE TEXT SEARCH CONFIGURATION ' || x.q || E' (\n    PARSER = '
            || (SELECT pg_catalog.quote_ident(pn.nspname) || '.' || pg_catalog.quote_ident(p.prsname)
                FROM pg_catalog.pg_ts_parser p JOIN pg_catalog.pg_namespace pn ON pn.oid = p.prsnamespace
                WHERE p.oid = cfg.cfgparser) || E'\n);',
        (SELECT pg_catalog.string_agg('ALTER TEXT SEARCH CONFIGURATION ' || x.q || ' ADD MAPPING FOR '
                    || tt.alias || ' WITH ' || m.dicts || ';', E'\n' ORDER BY tt.tokid)
            FROM pg_catalog.ts_token_type(cfg.cfgparser) tt
                JOIN LATERAL (SELECT pg_catalog.string_agg(cm.mapdict::pg_catalog.regdictionary::text, ', '
                                                           ORDER BY cm.mapseqno) AS dicts
                              FROM pg_catalog.pg_ts_config_map cm
                              WHERE cm.mapcfg = cfg.oid AND cm.maptokentype = tt.tokid) m ON m.dicts IS NOT NULL),
        'ALTER TEXT SEARCH CONFIGURATION ' || x.q || E'\n    OWNER TO ' || pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(cfg.cfgowner)) || ';',
        'COMMENT ON TEXT SEARCH CONFIGURATION ' || x.q || E'\n    IS ' || pg_catalog.quote_literal(pg_catalog.obj_description(cfg.oid, 'pg_ts_config')) || ';'
    ) AS body
FROM pg_catalog.pg_ts_config cfg
    JOIN pg_catalog.pg_namespace nsp ON nsp.oid = cfg.cfgnamespace
    CROSS JOIN LATERAL (SELECT pg_catalog.quote_ident(nsp.nspname) || '.' || pg_catalog.quote_ident(cfg.cfgname) AS q) x
WHERE cfg.oid = $1::oid
