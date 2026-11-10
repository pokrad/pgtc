-- Macros: GRANT statements from an aclitem[] column.
{% macro grants(acl, kind, target) -%}
(SELECT pg_catalog.string_agg(pg_catalog.format('GRANT %s ON %s %s TO %s%s;',
        g.privs, {{ kind }}, {{ target }}, g.grantee,
        CASE WHEN g.grantable THEN ' WITH GRANT OPTION' ELSE '' END), E'\n' ORDER BY g.grantee, g.grantable)
    FROM (SELECT CASE WHEN a.grantee = 0 THEN 'PUBLIC'
                      ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END AS grantee,
                 a.is_grantable AS grantable,
                 pg_catalog.string_agg(a.privilege_type, ', ' ORDER BY a.privilege_type) AS privs
          FROM pg_catalog.aclexplode({{ acl }}) a
          GROUP BY 1, 2) g)
{%- endmacro %}
