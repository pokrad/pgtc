-- \du, \dg (D-054): roles. $1 name regex or NULL. Switches: plus, hide_system.
SELECT r.rolname::text AS "Role name",
    pg_catalog.concat_ws(', ',
        CASE WHEN r.rolsuper THEN 'Superuser' END,
        CASE WHEN NOT r.rolinherit THEN 'No inheritance' END,
        CASE WHEN r.rolcreaterole THEN 'Create role' END,
        CASE WHEN r.rolcreatedb THEN 'Create DB' END,
        CASE WHEN NOT r.rolcanlogin THEN 'Cannot login' END,
        CASE WHEN r.rolreplication THEN 'Replication' END,
        CASE WHEN r.rolbypassrls THEN 'Bypass RLS' END,
        CASE WHEN r.rolconnlimit >= 0 THEN r.rolconnlimit || ' connections' END,
        CASE WHEN r.rolvaliduntil IS NOT NULL
            THEN 'Password valid until ' || r.rolvaliduntil END
    )::text AS "Attributes",
    pg_catalog.array_to_string(ARRAY(
        SELECT b.rolname FROM pg_catalog.pg_auth_members m
        JOIN pg_catalog.pg_roles b ON m.roleid = b.oid
        WHERE m.member = r.oid ORDER BY 1), ', ')::text AS "Member of"
{% if plus %}
    , pg_catalog.shobj_description(r.oid, 'pg_authid')::text AS "Description"
{% endif %}
FROM pg_catalog.pg_roles r
WHERE ($1::text IS NULL OR r.rolname ~ $1::text)
{% if hide_system %}
    AND r.rolname !~ '^pg_'
{% endif %}
ORDER BY 1
