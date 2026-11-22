//! Roles, owners and privileges of the sync script (D-048): the roles the definition
//! needs that the target's cluster lacks — the first section of the script —, and
//! the owners and missing privileges of the objects — the last one. Privileges are
//! never revoked: those only the database has get a note.

use std::collections::BTreeMap;

use pgc_core::quote::{quote_ident as qi, quote_literal as ql};

use super::{function_dropped, live_function, qn};
use crate::*;

/// The two sections, as statements and `-- NOTE:` lines.
#[derive(Default)]
pub(super) struct Sections {
    pub(super) roles: Vec<String>,
    pub(super) privileges: Vec<String>,
}

/// What the script makes anew although the database has it: tables re-created (their
/// partitioning changed), views replaced, functions dropped and created again. They
/// come back with the built-in privileges, owned by the role that runs the script.
pub(super) struct Recreated<'a> {
    pub(super) tables: &'a std::collections::BTreeSet<(String, String)>,
    pub(super) views: &'a std::collections::BTreeSet<ObjRef>,
}

pub(super) fn sections(want: &Database, live: &Database, recreated: &Recreated) -> Sections {
    let mut out = Sections {
        roles: roles(want, live),
        privileges: Vec::new(),
    };
    let p = &mut out.privileges;
    let empty = Schema::default();
    for ws in &want.schemas {
        let ls = live.schema(&ws.name);
        let schema_target = Target {
            owner_stmt: format!("ALTER SCHEMA {}", qi(&ws.name)),
            on: format!("SCHEMA {}", qi(&ws.name)),
            builtin: &[],
        };
        object(p, live, &schema_target, &ws.access, ls.map(|s| &s.access));
        let ls = ls.unwrap_or(&empty);
        for t in &ws.types {
            let q = qn(&ws.name, &t.name);
            let word = if matches!(t.kind, TypeKind::Domain { .. }) {
                "DOMAIN"
            } else {
                "TYPE"
            };
            let target = Target {
                owner_stmt: format!("ALTER {word} {q}"),
                on: format!("{word} {q}"),
                builtin: &[("", "USAGE")],
            };
            let have = ls
                .types
                .iter()
                .find(|l| l.name == t.name)
                .map(|l| &l.access);
            object(p, live, &target, &t.access, have);
        }
        for s in &ws.sequences {
            let q = qn(&ws.name, &s.name);
            // a sequence owned by a column belongs to the table's owner
            let owner_stmt = if s.owned_by.is_some() {
                String::new()
            } else {
                format!("ALTER SEQUENCE {q}")
            };
            let target = Target {
                owner_stmt,
                on: format!("SEQUENCE {q}"),
                builtin: &[],
            };
            let have = ls
                .sequences
                .iter()
                .find(|l| l.name == s.name)
                .map(|l| &l.access);
            object(p, live, &target, &s.access, have);
        }
        for t in &ws.tables {
            let q = qn(&ws.name, &t.name);
            let target = Target {
                owner_stmt: format!("ALTER TABLE {q}"),
                on: format!("TABLE {q}"),
                builtin: &[],
            };
            let again = recreated
                .tables
                .contains(&(ws.name.clone(), t.name.clone()));
            let have = ls
                .tables
                .iter()
                .find(|l| l.name == t.name)
                .filter(|_| !again);
            object(p, live, &target, &t.access, have.map(|l| &l.access));
            for c in &t.columns {
                let had = have.and_then(|l| l.columns.iter().find(|h| h.name == c.name));
                let on = format!("TABLE {q}");
                let column = format!("({})", qi(&c.name));
                grants(
                    p,
                    &on,
                    Some(&column),
                    &c.grants,
                    had.map(|h| &h.grants[..]),
                    &[],
                );
            }
        }
        for (views, have, word, kind) in [
            (&ws.views, &ls.views, "VIEW", ObjKind::View),
            (
                &ws.materialized_views,
                &ls.materialized_views,
                "MATERIALIZED VIEW",
                ObjKind::MaterializedView,
            ),
        ] {
            for v in views {
                let q = qn(&ws.name, &v.name);
                let target = Target {
                    owner_stmt: format!("ALTER {word} {q}"),
                    on: format!("TABLE {q}"),
                    builtin: &[],
                };
                let again = recreated
                    .views
                    .contains(&super::obj(kind, &ws.name, &v.name));
                let had = have
                    .iter()
                    .find(|l| l.name == v.name)
                    .filter(|_| !again)
                    .map(|l| &l.access);
                object(p, live, &target, &v.access, had);
            }
        }
        for f in &ws.functions {
            let word = if f.procedure { "PROCEDURE" } else { "FUNCTION" };
            let q = format!("{}({})", qn(&ws.name, &f.name), f.args);
            let target = Target {
                owner_stmt: format!("ALTER {word} {q}"),
                on: format!("{word} {q}"),
                builtin: &[("", "EXECUTE")],
            };
            let had = live_function(ls, f)
                .filter(|l| !function_dropped(f, l))
                .map(|l| &l.access);
            object(p, live, &target, &f.access, had);
        }
    }
    default_privileges(p, want, live);
    database(p, want, live);
    out
}

/// How the statements name an object.
struct Target<'a> {
    /// `ALTER TABLE s.t` — followed by ` OWNER TO role`; empty: its owner is not set
    /// on its own.
    owner_stmt: String,
    /// `TABLE s.t` — what `GRANT … ON` names.
    on: String,
    /// The privileges a new object has besides its owner's: (grantee, privilege).
    builtin: &'a [(&'a str, &'a str)],
}

/// Owner and privileges of one object. `have`: the object as the database has it —
/// `None` when the script creates it (`Some(None)`: the database has it from a time
/// before owners were kept).
fn object(
    out: &mut Vec<String>,
    live: &Database,
    target: &Target,
    want: &Option<Access>,
    have: Option<&Option<Access>>,
) {
    let Some(want) = want else {
        return; // a file from before owners and privileges were kept
    };
    let (owner, had) = match have {
        Some(Some(a)) => (Some(a.owner.as_str()), Some(&a.grants[..])),
        Some(None) => return,
        // new: the role that runs the script owns it
        None => (
            (!live.reader.is_empty()).then_some(live.reader.as_str()),
            None,
        ),
    };
    if !target.owner_stmt.is_empty() && owner != Some(want.owner.as_str()) {
        out.push(format!(
            "{} OWNER TO {};",
            target.owner_stmt,
            qi(&want.owner)
        ));
    }
    grants(out, &target.on, None, &want.grants, had, target.builtin);
}

/// `GRANT`s of the privileges `want` names that `have` lacks, and notes for those
/// only `have` has. `have` `None`: a new object, with only the `builtin` privileges.
/// `column`: `(c)` for the privileges on a column.
fn grants(
    out: &mut Vec<String>,
    on: &str,
    column: Option<&str>,
    want: &[Grant],
    have: Option<&[Grant]>,
    builtin: &[(&str, &str)],
) {
    let builtin: Vec<Grant> = builtin
        .iter()
        .map(|(grantee, privilege)| Grant {
            grantee: (*grantee).to_owned(),
            privilege: (*privilege).to_owned(),
            grantable: false,
        })
        .collect();
    let existing = have.unwrap_or(&builtin);
    let missing: Vec<&Grant> = want
        .iter()
        .filter(|g| {
            !existing.iter().any(|h| {
                h.grantee == g.grantee
                    && h.privilege == g.privilege
                    && (h.grantable || !g.grantable)
            })
        })
        .collect();
    let column = column.unwrap_or("");
    for ((grantee, grantable), privileges) in by_grantee(&missing) {
        let option = if grantable { " WITH GRANT OPTION" } else { "" };
        out.push(format!(
            "GRANT {}{column} ON {on} TO {}{option};",
            privileges.join(", "),
            role_name(grantee)
        ));
    }
    if have.is_some() {
        let extra: Vec<&Grant> = existing
            .iter()
            .filter(|h| {
                !want
                    .iter()
                    .any(|g| g.grantee == h.grantee && g.privilege == h.privilege)
            })
            .collect();
        for ((grantee, _), privileges) in by_grantee(&extra) {
            out.push(format!(
                "-- NOTE: {} has {}{column} on {on} in the database only; REVOKE it by hand if it is not wanted.",
                role_name(grantee),
                privileges.join(", ")
            ));
        }
    }
}

/// Privileges grouped by (grantee, grantable), in a stable order.
fn by_grantee<'a>(grants: &[&'a Grant]) -> BTreeMap<(&'a str, bool), Vec<&'a str>> {
    let mut out: BTreeMap<(&str, bool), Vec<&str>> = BTreeMap::new();
    for g in grants {
        out.entry((g.grantee.as_str(), g.grantable))
            .or_default()
            .push(&g.privilege);
    }
    out
}

/// A grantee as SQL names it: `PUBLIC` for `""`.
fn role_name(grantee: &str) -> String {
    if grantee.is_empty() {
        "PUBLIC".to_owned()
    } else {
        qi(grantee)
    }
}

/// `ALTER DEFAULT PRIVILEGES … GRANT` for what the target's default privileges lack.
fn default_privileges(out: &mut Vec<String>, want: &Database, live: &Database) {
    for d in &want.default_privileges {
        let had = live
            .default_privileges
            .iter()
            .find(|l| l.role == d.role && l.schema == d.schema && l.object == d.object);
        let schema = d
            .schema
            .as_ref()
            .map(|s| format!(" IN SCHEMA {}", qi(s)))
            .unwrap_or_default();
        let mut lines = Vec::new();
        let have = had.map_or(&[][..], |l| &l.grants[..]);
        grants(&mut lines, &d.object, None, &d.grants, Some(have), &[]);
        for line in lines {
            match line.strip_prefix("GRANT ") {
                // `GRANT p ON TABLES TO r;` is the end of the statement
                Some(rest) => out.push(format!(
                    "ALTER DEFAULT PRIVILEGES FOR ROLE {}{schema} GRANT {rest}",
                    qi(&d.role)
                )),
                None => out.push(line.replacen(
                    &format!(" on {} ", d.object),
                    &format!(" on future {} of role {}{schema} ", d.object, qi(&d.role)),
                    1,
                )),
            }
        }
    }
}

/// The privileges on the database itself; its name is the target's, so they are
/// granted with `current_database()`.
fn database(out: &mut Vec<String>, want: &Database, live: &Database) {
    let mut lines = Vec::new();
    let (want_grants, have) = (&want.database_grants, &live.database_grants[..]);
    grants(&mut lines, "DATABASE", None, want_grants, Some(have), &[]);
    for line in lines {
        match line.strip_suffix(';') {
            Some(grant) if grant.starts_with("GRANT ") => {
                // `GRANT p ON DATABASE TO r` → `… ON DATABASE <the current one> TO r`
                let format =
                    grant
                        .replace('%', "%%")
                        .replacen(" ON DATABASE ", " ON DATABASE %I ", 1);
                let body = format!(
                    "BEGIN EXECUTE pg_catalog.format({}, pg_catalog.current_database()); END",
                    ql(&format)
                );
                out.push(format!("DO {};", ql(&body)));
            }
            _ => out.push(line.replacen(
                " on DATABASE in the database only",
                " on this database only",
                1,
            )),
        }
    }
}

/// `CREATE ROLE` for the roles of the file the target's cluster lacks, notes for those
/// it has with other attributes, and the memberships it lacks.
fn roles(want: &Database, live: &Database) -> Vec<String> {
    let mut out = Vec::new();
    for r in &want.roles {
        match live.role(&r.name) {
            None => {
                let mut options = Vec::new();
                let mut flag = |on: bool, word: &str| {
                    if on {
                        options.push(word.to_owned());
                    }
                };
                flag(r.login, "LOGIN");
                flag(!r.inherit, "NOINHERIT");
                flag(r.createdb, "CREATEDB");
                flag(r.createrole, "CREATEROLE");
                flag(r.replication, "REPLICATION");
                flag(r.bypassrls, "BYPASSRLS");
                if r.connection_limit != -1 {
                    options.push(format!("CONNECTION LIMIT {}", r.connection_limit));
                }
                if let Some(v) = &r.valid_until {
                    options.push(format!("VALID UNTIL {}", ql(v)));
                }
                let with = if options.is_empty() {
                    String::new()
                } else {
                    format!(" WITH {}", options.join(" "))
                };
                out.push(format!("CREATE ROLE {}{with};", qi(&r.name)));
                if r.superuser {
                    out.push(format!(
                        "-- NOTE: {} is a superuser in the definition; it is created without SUPERUSER.",
                        qi(&r.name)
                    ));
                }
                if r.login {
                    out.push(format!(
                        "-- NOTE: {} can log in but has no password: ALTER ROLE {} PASSWORD '…';",
                        qi(&r.name),
                        qi(&r.name)
                    ));
                }
                if let Some(c) = &r.comment {
                    out.push(format!("COMMENT ON ROLE {} IS {};", qi(&r.name), ql(c)));
                }
            }
            Some(l) => {
                let differ: Vec<&str> = [
                    (r.superuser != l.superuser, "SUPERUSER"),
                    (r.login != l.login, "LOGIN"),
                    (r.inherit != l.inherit, "INHERIT"),
                    (r.createdb != l.createdb, "CREATEDB"),
                    (r.createrole != l.createrole, "CREATEROLE"),
                    (r.replication != l.replication, "REPLICATION"),
                    (r.bypassrls != l.bypassrls, "BYPASSRLS"),
                    (r.connection_limit != l.connection_limit, "CONNECTION LIMIT"),
                    (r.valid_until != l.valid_until, "VALID UNTIL"),
                ]
                .into_iter()
                .filter(|(d, _)| *d)
                .map(|(_, name)| name)
                .collect();
                if !differ.is_empty() {
                    out.push(format!(
                        "-- NOTE: role {} exists with another {}; it is left as it is.",
                        qi(&r.name),
                        differ.join(", ")
                    ));
                }
            }
        }
    }
    for r in &want.roles {
        let had = live.role(&r.name).map_or(&[][..], |l| &l.member_of[..]);
        for g in &r.member_of {
            let known = g.starts_with("pg_") || want.role(g).is_some() || live.role(g).is_some();
            if known && !had.contains(g) {
                out.push(format!("GRANT {} TO {};", qi(g), qi(&r.name)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::sync::sync_script;
    use crate::*;

    fn role(name: &str) -> Role {
        Role {
            name: name.into(),
            superuser: false,
            login: false,
            inherit: true,
            createdb: false,
            createrole: false,
            replication: false,
            bypassrls: false,
            connection_limit: -1,
            valid_until: None,
            member_of: vec![],
            comment: None,
        }
    }

    fn grant(grantee: &str, privilege: &str) -> Grant {
        Grant {
            grantee: grantee.into(),
            privilege: privilege.into(),
            grantable: false,
        }
    }

    fn access(owner: &str, grants: Vec<Grant>) -> Option<Access> {
        Some(Access {
            owner: owner.into(),
            grants,
        })
    }

    /// A database with schema `app` (owned by `owner`) and table `app.t`.
    fn db(owner: &str, table_grants: Vec<Grant>, roles: Vec<Role>) -> Database {
        Database {
            schemas: vec![Schema {
                name: "app".into(),
                access: access(owner, vec![]),
                tables: vec![Table {
                    name: "t".into(),
                    columns: vec![Column {
                        name: "id".into(),
                        data_type: "integer".into(),
                        ..Default::default()
                    }],
                    access: access(owner, table_grants),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            roles,
            reader: "postgres".into(),
            ..Default::default()
        }
    }

    fn at(text: &str, what: &str) -> usize {
        text.find(what)
            .unwrap_or_else(|| panic!("{what} is missing in\n{text}"))
    }

    /// Missing roles first, with notes for what is never written; owners and missing
    /// privileges last; nothing is revoked.
    #[test]
    fn roles_first_owners_and_privileges_last() {
        let mut app = role("app");
        app.login = true;
        app.connection_limit = 5;
        app.member_of = vec!["readers".into()];
        let mut boss = role("boss");
        boss.superuser = true;
        let want = db(
            "app",
            vec![grant("readers", "SELECT"), grant("", "SELECT")],
            vec![app, boss, role("readers")],
        );
        // the target: owned by postgres, one privilege of its own
        let live = db(
            "postgres",
            vec![grant("auditor", "SELECT")],
            vec![role("postgres"), role("auditor")],
        );
        let text = sync_script(&want, &live).unwrap();
        let roles = at(&text, "-- Roles the definition needs");
        assert!(roles < at(&text, "CREATE ROLE app WITH LOGIN CONNECTION LIMIT 5;"));
        at(&text, "-- NOTE: app can log in but has no password");
        at(&text, "CREATE ROLE boss;");
        at(
            &text,
            "-- NOTE: boss is a superuser in the definition; it is created without SUPERUSER.",
        );
        assert!(!text.contains("SUPERUSER;"), "{text}");
        // memberships after every role is there
        assert!(at(&text, "CREATE ROLE readers;") < at(&text, "GRANT readers TO app;"));
        let privileges = at(&text, "-- Owners and privileges (none is revoked):");
        assert!(at(&text, "GRANT readers TO app;") < privileges);
        assert!(privileges < at(&text, "ALTER SCHEMA app OWNER TO app;"));
        at(&text, "ALTER TABLE app.t OWNER TO app;");
        at(&text, "GRANT SELECT ON TABLE app.t TO PUBLIC;");
        at(&text, "GRANT SELECT ON TABLE app.t TO readers;");
        at(
            &text,
            "-- NOTE: auditor has SELECT on TABLE app.t in the database only;",
        );
        // a note says so, but no statement revokes anything
        assert!(!text.lines().any(|l| l.starts_with("REVOKE")), "{text}");
    }

    /// The same owners and privileges: nothing to do. A role that exists with other
    /// attributes is not changed.
    #[test]
    fn what_is_there_is_left_alone() {
        let want = db("app", vec![grant("app", "SELECT")], vec![role("app")]);
        let mut live = want.clone();
        assert_eq!(sync_script(&want, &live), None);
        live.roles[0].createdb = true;
        let text = sync_script(&want, &live).unwrap();
        at(
            &text,
            "-- NOTE: role app exists with another CREATEDB; it is left as it is.",
        );
        assert!(!text.contains("CREATE ROLE"), "{text}");
    }

    /// What the script creates belongs to the role that runs it: its owner is set
    /// only when the file names another one; a new function keeps PUBLIC EXECUTE.
    #[test]
    fn new_objects_owned_by_the_reader() {
        let f = |owner: &str| Function {
            name: "f".into(),
            definition: "CREATE OR REPLACE FUNCTION app.f()\n RETURNS integer\nRETURN 1\n".into(),
            result: Some("integer".into()),
            access: access(owner, vec![grant("", "EXECUTE")]),
            ..Default::default()
        };
        let mut want = db("postgres", vec![], vec![]);
        want.schemas[0].functions.push(f("postgres"));
        let live = db("postgres", vec![], vec![]);
        let text = sync_script(&want, &live).unwrap();
        assert!(!text.contains("OWNER TO"), "{text}");
        assert!(!text.contains("GRANT"), "{text}");
        want.schemas[0].functions[0] = f("app");
        let text = sync_script(&want, &live).unwrap();
        at(&text, "ALTER FUNCTION app.f() OWNER TO app;");
    }

    /// Default privileges and the privileges on the database itself.
    #[test]
    fn default_privileges_and_the_database() {
        let mut want = db("postgres", vec![], vec![]);
        want.default_privileges.push(DefaultPrivilege {
            role: "app".into(),
            schema: Some("app".into()),
            object: "TABLES".into(),
            grants: vec![grant("readers", "SELECT")],
        });
        want.database_grants = vec![grant("app", "CONNECT"), grant("", "TEMPORARY")];
        let mut live = db("postgres", vec![], vec![]);
        live.database_grants = vec![grant("", "TEMPORARY"), grant("old", "CREATE")];
        let text = sync_script(&want, &live).unwrap();
        at(
            &text,
            "ALTER DEFAULT PRIVILEGES FOR ROLE app IN SCHEMA app GRANT SELECT ON TABLES TO readers;",
        );
        at(
            &text,
            "DO 'BEGIN EXECUTE pg_catalog.format(''GRANT CONNECT ON DATABASE %I TO app'', pg_catalog.current_database()); END';",
        );
        at(&text, "-- NOTE: old has CREATE on this database only;");
    }

    /// The file keeps the roles the database needs, and the groups they are in.
    #[test]
    fn the_file_keeps_the_needed_roles() {
        let mut app = role("app");
        app.member_of = vec!["readers".into(), "pg_read_all_data".into()];
        let mut db = db(
            "app",
            vec![],
            vec![app, role("readers"), role("unrelated"), role("postgres")],
        );
        db.keep_needed_roles();
        let names: Vec<&str> = db.roles.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["app", "readers"]);
    }
}
