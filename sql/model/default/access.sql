-- Structure model (D-048): the roles of the cluster (the file keeps those the database
-- needs), the privileges on the database itself, default privileges, and the role
-- that reads. Rows: schema (''), kind, data (JSON). Never a password.
{% import "macros/model.sql" as m %}
SELECT '' AS schema, 'role' AS kind,
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'name', r.rolname,
        'superuser', r.rolsuper,
        'login', r.rolcanlogin,
        'inherit', r.rolinherit,
        'createdb', r.rolcreatedb,
        'createrole', r.rolcreaterole,
        'replication', r.rolreplication,
        'bypassrls', r.rolbypassrls,
        'connection_limit', r.rolconnlimit,
        'valid_until', r.rolvaliduntil::text,
        'member_of', (SELECT pg_catalog.json_agg(DISTINCT g.rolname)
            FROM pg_catalog.pg_auth_members am
                JOIN pg_catalog.pg_roles g ON g.oid = am.roleid
            WHERE am.member = r.oid),
        'comment', pg_catalog.shobj_description(r.oid, 'pg_authid')))::text AS data
FROM pg_catalog.pg_roles r
WHERE r.rolname !~ '^pg_'
UNION ALL
SELECT '', 'database',
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'grants', {{ m.grants("COALESCE(d.datacl, pg_catalog.acldefault('d', d.datdba))", "d.datdba") }}))::text
FROM pg_catalog.pg_database d
WHERE d.datname = pg_catalog.current_database()
UNION ALL
SELECT '', 'default_privilege',
    pg_catalog.json_strip_nulls(pg_catalog.json_build_object(
        'role', pg_catalog.pg_get_userbyid(a.defaclrole),
        'schema', n.nspname,
        'object', CASE a.defaclobjtype WHEN 'r' THEN 'TABLES' WHEN 'S' THEN 'SEQUENCES'
                                       WHEN 'f' THEN 'FUNCTIONS' WHEN 'T' THEN 'TYPES'
                                       ELSE 'SCHEMAS' END,
        'grants', {{ m.grants("a.defaclacl", "a.defaclrole") }}))::text
FROM pg_catalog.pg_default_acl a
    LEFT JOIN pg_catalog.pg_namespace n ON n.oid = a.defaclnamespace
UNION ALL
SELECT '', 'reader', pg_catalog.json_build_object('name', CURRENT_USER)::text
