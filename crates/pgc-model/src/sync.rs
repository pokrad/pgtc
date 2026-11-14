//! Sync script (D-048): the statements that bring a live database to the structure of a
//! definition file — only what differs, never dropping objects that exist only in the
//! database, ordered by dependencies.
//!
//! Order:
//! 0. schemas, 1. extensions,
//! 2. types, sequences, functions, tables, views, materialized views — topologically
//!    sorted by `depends_on` (and partition parents),
//! 3. data of re-created tables, 4. foreign keys and `OWNED BY`, 5. indexes,
//! 6. triggers, rules, policies, row security, 7. the copies of re-created tables
//!    (commented-out `DROP TABLE`).
//!
//! A table is re-created only when its partitioning changed (it cannot be altered).
//! Its rows are first copied into `_TableCopy_<name>` and loaded back after the new
//! table exists; parts that exist only in the database are re-created too. Its
//! partitions go with it (`DROP TABLE` removes them): they are created again — also
//! those whose own partitioning changed — and get their rows back through the parent.
//!
//! Properties of an existing object follow the file (a default or comment missing in
//! the file is removed); parts that exist only in the database (columns, constraints,
//! indexes, ...) are kept. What cannot be changed with `ALTER` is reported as a
//! `-- NOTE:` comment in the script.

use std::collections::{BTreeSet, HashMap};

use pgc_core::quote::{quote_ident as qi, quote_literal as ql};

use crate::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Key {
    Schema(String),
    Extension(String),
    Obj(ObjRef),
    /// A later step of an object: phase, owner, what.
    Part(ObjRef, String),
}

struct Step {
    phase: u8,
    key: Key,
    deps: Vec<ObjRef>,
    sql: Vec<String>,
}

#[derive(Default)]
struct Script {
    steps: Vec<Step>,
    index: HashMap<(u8, Key), usize>,
}

impl Script {
    fn push(&mut self, phase: u8, key: Key, deps: &[ObjRef], sql: impl Into<String>) {
        let sql = sql.into();
        if let Some(&i) = self.index.get(&(phase, key.clone())) {
            let s = &mut self.steps[i];
            if !s.sql.contains(&sql) {
                s.sql.push(sql);
            }
            s.deps.extend(deps.iter().cloned());
            return;
        }
        self.index.insert((phase, key.clone()), self.steps.len());
        self.steps.push(Step {
            phase,
            key,
            deps: deps.to_vec(),
            sql: vec![sql],
        });
    }
}

fn qn(schema: &str, name: &str) -> String {
    format!("{}.{}", qi(schema), qi(name))
}

fn lit_or_null(v: Option<&str>) -> String {
    v.map_or_else(|| "NULL".to_owned(), ql)
}

/// `COMMENT ON` when the comment of a new object is set or of an existing one differs.
fn comment(
    out: &mut Script,
    phase: u8,
    key: &Key,
    what: &str,
    want: &Option<String>,
    live: Option<&Option<String>>,
) {
    let changed = match live {
        None => want.is_some(),
        Some(l) => l != want,
    };
    if changed {
        out.push(
            phase,
            key.clone(),
            &[],
            format!("COMMENT ON {what} IS {};", lit_or_null(want.as_deref())),
        );
    }
}

fn obj(kind: ObjKind, schema: &str, name: &str) -> ObjRef {
    ObjRef {
        kind,
        schema: schema.to_owned(),
        name: name.to_owned(),
        args: None,
    }
}

/// The script that brings `live` to `want`, or `None` if nothing differs.
pub fn sync_script(want: &Database, live: &Database) -> Option<String> {
    let mut out = Script::default();
    let (rebuilt, dropped) = rebuilt_tables(want, live);
    let mut reown: Vec<Disown> = Vec::new();
    for ws in &want.schemas {
        let ls = live.schema(&ws.name);
        let key = Key::Schema(ws.name.clone());
        if ls.is_none() {
            out.push(
                0,
                key.clone(),
                &[],
                format!("CREATE SCHEMA {};", qi(&ws.name)),
            );
        }
        comment(
            &mut out,
            0,
            &key,
            &format!("SCHEMA {}", qi(&ws.name)),
            &ws.comment,
            ls.map(|s| &s.comment),
        );
    }
    for e in &want.extensions {
        if !live.extensions.iter().any(|l| l.name == e.name) {
            out.push(
                1,
                Key::Extension(e.name.clone()),
                &[],
                format!(
                    "CREATE EXTENSION IF NOT EXISTS {} WITH SCHEMA {};",
                    qi(&e.name),
                    qi(&e.schema)
                ),
            );
        }
    }
    let empty = Schema::default();
    for ws in &want.schemas {
        let ls = live.schema(&ws.name).unwrap_or(&empty);
        for t in &ws.types {
            types(
                &mut out,
                &ws.name,
                t,
                ls.types.iter().find(|l| l.name == t.name),
            );
        }
        for s in &ws.sequences {
            sequence(
                &mut out,
                &ws.name,
                s,
                ls.sequences.iter().find(|l| l.name == s.name),
            );
        }
        for f in &ws.functions {
            function(
                &mut out,
                &ws.name,
                f,
                ls.functions
                    .iter()
                    .find(|l| l.name == f.name && l.args == f.args),
            );
        }
        for t in &ws.tables {
            let l = ls.tables.iter().find(|l| l.name == t.name);
            let key = (ws.name.clone(), t.name.clone());
            match l {
                Some(l) if rebuilt.contains(&key) => {
                    let merged = merge(t, l);
                    let plan = rebuild_plan(want, live, &ws.name, &merged, l, &dropped);
                    reown.extend(plan.disown.iter().cloned());
                    out_rebuild(&mut out, &ws.name, &merged, plan);
                }
                // dropped together with its re-created parent: created again, with
                // what exists only in the database; its rows come back through the
                // parent
                Some(l) if dropped.contains(&key) => table(&mut out, &ws.name, &merge(t, l), None),
                l => table(&mut out, &ws.name, t, l),
            }
        }
        // partitions only in the database, dropped with a re-created parent
        for l in &ls.tables {
            let key = (ws.name.clone(), l.name.clone());
            if dropped.contains(&key) && !ws.tables.iter().any(|t| t.name == l.name) {
                table(&mut out, &ws.name, l, None);
            }
        }
        for v in &ws.views {
            view(
                &mut out,
                &ws.name,
                v,
                ls.views.iter().find(|l| l.name == v.name),
                false,
            );
        }
        for v in &ws.materialized_views {
            view(
                &mut out,
                &ws.name,
                v,
                ls.materialized_views.iter().find(|l| l.name == v.name),
                true,
            );
        }
    }
    // sequences released before a DROP TABLE get their owner back
    for d in reown {
        let owner = want
            .schema(&d.schema)
            .and_then(|s| s.sequences.iter().find(|s| s.name == d.name))
            .map_or(d.owner.clone(), |s| s.owned_by.clone());
        if let Some(o) = owner {
            out.push(
                4,
                Key::Part(obj(ObjKind::Sequence, &d.schema, &d.name), "owned".into()),
                &[],
                format!(
                    "ALTER SEQUENCE {} OWNED BY {}.{};",
                    qn(&d.schema, &d.name),
                    qn(&o.schema, &o.table),
                    qi(&o.column)
                ),
            );
        }
    }
    render(out)
}

type TableKey = (String, String);

/// Tables that must be re-created (partitioning changed) and every table their
/// `DROP TABLE` removes (the partitions below them). A partition whose partitioning
/// changed together with its parent's is not re-created on its own: the parent's
/// `DROP TABLE` removes it and the parent's copy has its rows.
fn rebuilt_tables(want: &Database, live: &Database) -> (BTreeSet<TableKey>, BTreeSet<TableKey>) {
    let mut changed = BTreeSet::new();
    for ws in &want.schemas {
        let Some(ls) = live.schema(&ws.name) else {
            continue;
        };
        for t in &ws.tables {
            if let Some(l) = ls.tables.iter().find(|l| l.name == t.name)
                && (t.partition_by != l.partition_by || t.partition_of != l.partition_of)
            {
                changed.insert((ws.name.clone(), t.name.clone()));
            }
        }
    }
    let mut dropped = changed.clone();
    loop {
        let more: Vec<TableKey> = live
            .schemas
            .iter()
            .flat_map(|s| s.tables.iter().map(move |t| (s, t)))
            .filter(|(s, t)| !dropped.contains(&(s.name.clone(), t.name.clone())))
            .filter(|(_, t)| {
                t.partition_of.as_ref().is_some_and(|p| {
                    dropped.contains(&(p.parent.schema.clone(), p.parent.name.clone()))
                })
            })
            .map(|(s, t)| (s.name.clone(), t.name.clone()))
            .collect();
        if more.is_empty() {
            break;
        }
        dropped.extend(more);
    }
    // only the topmost: not the partitions of a table that is dropped anyway
    let live_parent = |(schema, name): &TableKey| {
        let t = live
            .schema(schema)?
            .tables
            .iter()
            .find(|t| t.name == *name)?;
        let p = &t.partition_of.as_ref()?.parent;
        Some((p.schema.clone(), p.name.clone()))
    };
    let rebuilt = changed
        .iter()
        .filter(|&key| live_parent(key).is_none_or(|p| !dropped.contains(&p)))
        .cloned()
        .collect();
    (rebuilt, dropped)
}

/// The file's table plus what exists only in the database (kept, D-048).
fn merge(t: &Table, l: &Table) -> Table {
    fn add<T: Clone>(to: &mut Vec<T>, from: &[T], name: impl Fn(&T) -> &str) {
        for x in from {
            if !to.iter().any(|y| name(y) == name(x)) {
                to.push(x.clone());
            }
        }
    }
    let mut m = t.clone();
    add(&mut m.columns, &l.columns, |c| &c.name);
    add(&mut m.constraints, &l.constraints, |c| &c.name);
    add(&mut m.indexes, &l.indexes, |c| &c.name);
    add(&mut m.triggers, &l.triggers, |c| &c.name);
    add(&mut m.rules, &l.rules, |c| &c.name);
    add(&mut m.policies, &l.policies, |c| &c.name);
    m
}

/// A sequence owned by a column of a dropped table: released before the drop.
#[derive(Clone)]
struct Disown {
    schema: String,
    name: String,
    owner: Option<ColumnRef>,
}

struct RebuildPlan {
    copy: String,
    /// Columns loaded back from the copy, and those of them that are identities.
    restore: Vec<String>,
    identity: Vec<String>,
    disown: Vec<Disown>,
}

/// Columns of a table; a partition has its parent's.
fn effective_columns<'a>(db: &'a Database, t: &'a Table) -> Vec<&'a Column> {
    let mut t = t;
    for _ in 0..32 {
        let Some(p) = &t.partition_of else {
            break;
        };
        let Some(parent) = db
            .schema(&p.parent.schema)
            .and_then(|s| s.tables.iter().find(|x| x.name == p.parent.name))
        else {
            return Vec::new();
        };
        t = parent;
    }
    t.columns.iter().collect()
}

fn copy_name(name: &str) -> String {
    let mut copy = format!("_TableCopy_{name}");
    while copy.len() > 63 {
        copy.pop();
    }
    copy
}

fn rebuild_plan(
    want: &Database,
    live: &Database,
    schema: &str,
    t: &Table,
    l: &Table,
    dropped: &BTreeSet<TableKey>,
) -> RebuildPlan {
    let old: Vec<String> = effective_columns(live, l)
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let mut wanted = effective_columns(want, t);
    if t.partition_of.is_none() {
        // the merged table (with the database-only columns)
        wanted = t.columns.iter().collect();
    }
    let restore: Vec<&Column> = wanted
        .into_iter()
        .filter(|c| c.generated.is_none() && old.contains(&c.name))
        .collect();
    let disown = live
        .schemas
        .iter()
        .flat_map(|s| s.sequences.iter().map(move |q| (s, q)))
        .filter_map(|(s, q)| {
            let o = q.owned_by.as_ref()?;
            dropped
                .contains(&(o.schema.clone(), o.table.clone()))
                .then(|| Disown {
                    schema: s.name.clone(),
                    name: q.name.clone(),
                    owner: Some(o.clone()),
                })
        })
        .collect();
    RebuildPlan {
        copy: qn(schema, &copy_name(&t.name)),
        identity: restore
            .iter()
            .filter(|c| c.identity.is_some())
            .map(|c| c.name.clone())
            .collect(),
        restore: restore.iter().map(|c| c.name.clone()).collect(),
        disown,
    }
}

/// Copy the rows, drop the table, create it as the file has it, load the rows back.
fn out_rebuild(out: &mut Script, schema: &str, t: &Table, plan: RebuildPlan) {
    let r = obj(ObjKind::Table, schema, &t.name);
    let key = Key::Obj(r.clone());
    let q = qn(schema, &t.name);
    let copy = &plan.copy;
    let mut deps = t.depends_on.clone();
    if let Some(p) = &t.partition_of {
        deps.push(obj(ObjKind::Table, &p.parent.schema, &p.parent.name));
    }
    out.push(
        2,
        key.clone(),
        &deps,
        format!(
            "-- NOTE: {q} is re-created because its partitioning changed. Its rows are\n\
             -- copied into {copy} first and loaded back below. Views and foreign keys\n\
             -- that depend on {q} must be dropped before (DROP TABLE fails otherwise)."
        ),
    );
    for d in &plan.disown {
        out.push(
            2,
            key.clone(),
            &[],
            format!("ALTER SEQUENCE {} OWNED BY NONE;", qn(&d.schema, &d.name)),
        );
    }
    out.push(
        2,
        key.clone(),
        &[],
        format!("CREATE TABLE {copy} AS TABLE {q};"),
    );
    out.push(2, key.clone(), &[], format!("DROP TABLE {q};"));
    table(out, schema, t, None);
    if !plan.restore.is_empty() {
        let cols: Vec<String> = plan.restore.iter().map(|c| qi(c)).collect();
        let cols = cols.join(", ");
        let restore = Key::Part(r.clone(), "restore".into());
        out.push(
            3,
            restore.clone(),
            &[],
            format!(
                "INSERT INTO {q} ({cols}) OVERRIDING SYSTEM VALUE\n    SELECT {cols} FROM {copy};"
            ),
        );
        for c in &plan.identity {
            out.push(
                3,
                restore.clone(),
                &[],
                format!(
                    "SELECT pg_catalog.setval(pg_catalog.pg_get_serial_sequence({}, {}), max({})) FROM {q} HAVING max({}) IS NOT NULL;",
                    ql(&q),
                    ql(c),
                    qi(c),
                    qi(c)
                ),
            );
        }
    }
    out.push(
        7,
        Key::Part(r, "copy".into()),
        &[],
        format!("-- DROP TABLE {copy};"),
    );
}

fn render(out: Script) -> Option<String> {
    if out.steps.is_empty() {
        return None;
    }
    let mut ordered: Vec<&Step> = Vec::new();
    for phase in 0..=7u8 {
        let steps: Vec<&Step> = out.steps.iter().filter(|s| s.phase == phase).collect();
        if phase == 2 {
            ordered.extend(topo_sort(steps));
        } else {
            ordered.extend(steps);
        }
    }
    let functions = ordered
        .iter()
        .any(|s| matches!(&s.key, Key::Obj(r) if r.kind == ObjKind::Function));
    let mut text = String::from(
        "-- Sync script generated by pgtc: brings this database to the structure of the\n\
         -- definition file. Objects that exist only in the database are kept.\n\
         -- Review it before running.\n\n",
    );
    if functions {
        text.push_str("SET check_function_bodies = false;\n\n");
    }
    let mut copies = false;
    for s in ordered {
        if s.phase == 7 && !copies {
            copies = true;
            text.push_str(
                "-- Copies of the re-created tables. Drop them once you are sure the\n\
                 -- migration went well:\n",
            );
        }
        text.push_str(&s.sql.join("\n"));
        text.push_str(if s.phase == 7 { "\n" } else { "\n\n" });
    }
    let len = text.trim_end().len();
    text.truncate(len);
    text.push('\n');
    Some(text)
}

/// Preferred order between independent objects.
fn rank(kind: ObjKind) -> u8 {
    match kind {
        ObjKind::Type => 0,
        ObjKind::Sequence => 1,
        ObjKind::Function => 2,
        ObjKind::Table => 3,
        ObjKind::View => 4,
        ObjKind::MaterializedView => 5,
    }
}

/// Kahn's algorithm with a deterministic choice (rank, then name) among ready steps; a
/// cycle is broken by taking the first remaining step.
fn topo_sort(steps: Vec<&Step>) -> Vec<&Step> {
    let refs: Vec<ObjRef> = steps
        .iter()
        .map(|s| match &s.key {
            Key::Obj(r) | Key::Part(r, _) => r.clone(),
            _ => unreachable!("phase 2 holds object steps only"),
        })
        .collect();
    let at: HashMap<&ObjRef, usize> = refs.iter().enumerate().map(|(i, r)| (r, i)).collect();
    let mut waiting: Vec<BTreeSet<usize>> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            s.deps
                .iter()
                .filter_map(|d| at.get(d).copied())
                .filter(|&d| d != i)
                .collect()
        })
        .collect();
    let order_key = |i: usize| (rank(refs[i].kind), refs[i].clone());
    let mut done = vec![false; steps.len()];
    let mut out = Vec::with_capacity(steps.len());
    while out.len() < steps.len() {
        let next = (0..steps.len())
            .filter(|&i| !done[i] && waiting[i].is_empty())
            .min_by_key(|&i| order_key(i))
            .or_else(|| {
                (0..steps.len())
                    .filter(|&i| !done[i])
                    .min_by_key(|&i| order_key(i))
            })
            .expect("a step is left");
        done[next] = true;
        out.push(steps[next]);
        for w in &mut waiting {
            w.remove(&next);
        }
    }
    out
}

// ---- types ----------------------------------------------------------------------

fn types(out: &mut Script, schema: &str, t: &Type, live: Option<&Type>) {
    let r = obj(ObjKind::Type, schema, &t.name);
    let key = Key::Obj(r.clone());
    let q = qn(schema, &t.name);
    let deps = &t.depends_on;
    match live {
        None => out.push(2, key.clone(), deps, create_type(&q, &t.kind)),
        Some(l) => alter_type(out, &key, deps, &q, &t.kind, &l.kind),
    }
    let what = match t.kind {
        TypeKind::Domain { .. } => "DOMAIN",
        _ => "TYPE",
    };
    comment(
        out,
        2,
        &key,
        &format!("{what} {q}"),
        &t.comment,
        live.map(|l| &l.comment),
    );
}

fn collate(c: &Option<String>) -> String {
    c.as_ref()
        .map(|c| format!(" COLLATE {c}"))
        .unwrap_or_default()
}

fn create_type(q: &str, kind: &TypeKind) -> String {
    match kind {
        TypeKind::Enum { labels } => {
            let labels: Vec<String> = labels.iter().map(|l| ql(l)).collect();
            format!("CREATE TYPE {q} AS ENUM ({});", labels.join(", "))
        }
        TypeKind::Composite { attributes } => {
            let attrs: Vec<String> = attributes
                .iter()
                .map(|a| {
                    format!(
                        "    {} {}{}",
                        qi(&a.name),
                        a.data_type,
                        collate(&a.collation)
                    )
                })
                .collect();
            format!("CREATE TYPE {q} AS (\n{}\n);", attrs.join(",\n"))
        }
        TypeKind::Domain {
            base,
            not_null,
            default,
            collation,
            checks,
        } => {
            let mut s = format!("CREATE DOMAIN {q} AS {base}{}", collate(collation));
            if let Some(d) = default {
                s.push_str(&format!(" DEFAULT {d}"));
            }
            if *not_null {
                s.push_str(" NOT NULL");
            }
            for c in checks {
                s.push_str(&format!(
                    "\n    CONSTRAINT {} {}",
                    qi(&c.name),
                    c.definition
                ));
            }
            s.push(';');
            s
        }
        TypeKind::Range {
            subtype,
            subtype_opclass,
            collation,
            canonical,
            subtype_diff,
        } => {
            let mut parts = vec![format!("SUBTYPE = {subtype}")];
            let mut opt = |name: &str, v: &Option<String>| {
                if let Some(v) = v {
                    parts.push(format!("{name} = {v}"));
                }
            };
            opt("SUBTYPE_OPCLASS", subtype_opclass);
            opt("COLLATION", collation);
            opt("CANONICAL", canonical);
            opt("SUBTYPE_DIFF", subtype_diff);
            format!("CREATE TYPE {q} AS RANGE ({});", parts.join(", "))
        }
    }
}

fn alter_type(
    out: &mut Script,
    key: &Key,
    deps: &[ObjRef],
    q: &str,
    want: &TypeKind,
    live: &TypeKind,
) {
    let mut push = |sql: String| out.push(2, key.clone(), deps, sql);
    match (want, live) {
        (TypeKind::Enum { labels }, TypeKind::Enum { labels: have }) => {
            let mut known: Vec<&String> = have.iter().collect();
            for (i, l) in labels.iter().enumerate() {
                if known.contains(&l) {
                    continue;
                }
                let place = labels[..i]
                    .iter()
                    .rev()
                    .find(|p| known.contains(p))
                    .map(|p| format!(" AFTER {}", ql(p)))
                    .or_else(|| {
                        labels[i + 1..]
                            .iter()
                            .find(|n| known.contains(n))
                            .map(|n| format!(" BEFORE {}", ql(n)))
                    })
                    .unwrap_or_default();
                push(format!(
                    "ALTER TYPE {q} ADD VALUE IF NOT EXISTS {}{place};",
                    ql(l)
                ));
                known.push(l);
            }
        }
        (TypeKind::Composite { attributes }, TypeKind::Composite { attributes: have }) => {
            for a in attributes {
                match have.iter().find(|h| h.name == a.name) {
                    None => push(format!(
                        "ALTER TYPE {q} ADD ATTRIBUTE {} {}{};",
                        qi(&a.name),
                        a.data_type,
                        collate(&a.collation)
                    )),
                    Some(h) if h.data_type != a.data_type || h.collation != a.collation => {
                        push(format!(
                            "ALTER TYPE {q} ALTER ATTRIBUTE {} TYPE {}{};",
                            qi(&a.name),
                            a.data_type,
                            collate(&a.collation)
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
        (
            TypeKind::Domain {
                base,
                not_null,
                default,
                collation,
                checks,
            },
            TypeKind::Domain {
                base: have_base,
                not_null: have_not_null,
                default: have_default,
                collation: have_collation,
                checks: have_checks,
            },
        ) => {
            if base != have_base || collation != have_collation {
                push(format!(
                    "-- NOTE: domain {q} has base type {have_base}{} in the database and \
                     {base}{} in the file; change it manually.",
                    collate(have_collation),
                    collate(collation)
                ));
            }
            if default != have_default {
                push(match default {
                    Some(d) => format!("ALTER DOMAIN {q} SET DEFAULT {d};"),
                    None => format!("ALTER DOMAIN {q} DROP DEFAULT;"),
                });
            }
            if not_null != have_not_null {
                let op = if *not_null { "SET" } else { "DROP" };
                push(format!("ALTER DOMAIN {q} {op} NOT NULL;"));
            }
            for c in checks {
                match have_checks.iter().find(|h| h.name == c.name) {
                    Some(h) if h.definition == c.definition => {}
                    found => {
                        if found.is_some() {
                            push(format!("ALTER DOMAIN {q} DROP CONSTRAINT {};", qi(&c.name)));
                        }
                        push(format!(
                            "ALTER DOMAIN {q} ADD CONSTRAINT {} {};",
                            qi(&c.name),
                            c.definition
                        ));
                    }
                }
            }
        }
        (want, live) if want == live => {}
        _ => push(format!(
            "-- NOTE: type {q} differs between the database and the file in a way that \
             cannot be changed with ALTER; change it manually."
        )),
    }
}

// ---- sequences and functions --------------------------------------------------------

fn sequence(out: &mut Script, schema: &str, s: &Sequence, live: Option<&Sequence>) {
    let r = obj(ObjKind::Sequence, schema, &s.name);
    let key = Key::Obj(r.clone());
    let q = qn(schema, &s.name);
    let cycle = |c: bool| if c { "CYCLE" } else { "NO CYCLE" };
    match live {
        None => out.push(
            2,
            key.clone(),
            &[],
            format!(
                "CREATE SEQUENCE {q} AS {} INCREMENT BY {} MINVALUE {} MAXVALUE {} START WITH {} CACHE {} {};",
                s.data_type,
                s.increment,
                s.min_value,
                s.max_value,
                s.start,
                s.cache,
                cycle(s.cycle)
            ),
        ),
        Some(l) => {
            let mut parts = Vec::new();
            if l.data_type != s.data_type {
                parts.push(format!("AS {}", s.data_type));
            }
            if l.increment != s.increment {
                parts.push(format!("INCREMENT BY {}", s.increment));
            }
            if l.min_value != s.min_value {
                parts.push(format!("MINVALUE {}", s.min_value));
            }
            if l.max_value != s.max_value {
                parts.push(format!("MAXVALUE {}", s.max_value));
            }
            if l.start != s.start {
                parts.push(format!("START WITH {}", s.start));
            }
            if l.cache != s.cache {
                parts.push(format!("CACHE {}", s.cache));
            }
            if l.cycle != s.cycle {
                parts.push(cycle(s.cycle).to_owned());
            }
            if !parts.is_empty() {
                out.push(2, key.clone(), &[], format!("ALTER SEQUENCE {q} {};", parts.join(" ")));
            }
        }
    }
    let have = live.and_then(|l| l.owned_by.as_ref());
    if s.owned_by.as_ref() != have && (s.owned_by.is_some() || live.is_some()) {
        let owner = match &s.owned_by {
            Some(o) => format!("{}.{}", qn(&o.schema, &o.table), qi(&o.column)),
            None => "NONE".to_owned(),
        };
        out.push(
            4,
            Key::Part(r.clone(), "owned".into()),
            &[],
            format!("ALTER SEQUENCE {q} OWNED BY {owner};"),
        );
    }
    comment(
        out,
        2,
        &key,
        &format!("SEQUENCE {q}"),
        &s.comment,
        live.map(|l| &l.comment),
    );
}

fn function(out: &mut Script, schema: &str, f: &Function, live: Option<&Function>) {
    let r = ObjRef {
        kind: ObjKind::Function,
        schema: schema.to_owned(),
        name: f.name.clone(),
        args: Some(f.args.clone()),
    };
    let key = Key::Obj(r);
    let what = format!(
        "{} {}({})",
        if f.procedure { "PROCEDURE" } else { "FUNCTION" },
        qn(schema, &f.name),
        f.args
    );
    let create = format!("{};", f.definition.trim_end());
    match live {
        None => out.push(2, key.clone(), &f.depends_on, create),
        Some(l) if l.definition == f.definition => {}
        Some(l) => {
            if l.result != f.result || l.procedure != f.procedure {
                let old = format!(
                    "{} {}({})",
                    if l.procedure { "PROCEDURE" } else { "FUNCTION" },
                    qn(schema, &l.name),
                    l.args
                );
                out.push(2, key.clone(), &f.depends_on, format!("DROP {old};"));
            }
            out.push(2, key.clone(), &f.depends_on, create);
        }
    }
    comment(out, 2, &key, &what, &f.comment, live.map(|l| &l.comment));
}

// ---- tables -------------------------------------------------------------------------

fn column_def(c: &Column) -> String {
    let mut s = format!("{} {}{}", qi(&c.name), c.data_type, collate(&c.collation));
    if let Some(g) = &c.generated {
        let how = if c.virtual_ { "VIRTUAL" } else { "STORED" };
        s.push_str(&format!(" GENERATED ALWAYS AS ({g}) {how}"));
    }
    if let Some(i) = &c.identity {
        s.push_str(&format!(" GENERATED {} AS IDENTITY", i.to_uppercase()));
    }
    if c.not_null {
        s.push_str(" NOT NULL");
    }
    if let Some(d) = &c.default {
        s.push_str(&format!(" DEFAULT {d}"));
    }
    s
}

fn option_name(o: &str) -> &str {
    o.split('=').next().unwrap_or(o)
}

fn table(out: &mut Script, schema: &str, t: &Table, live: Option<&Table>) {
    let r = obj(ObjKind::Table, schema, &t.name);
    let key = Key::Obj(r.clone());
    let q = qn(schema, &t.name);
    let mut deps = t.depends_on.clone();
    if let Some(p) = &t.partition_of {
        deps.push(obj(ObjKind::Table, &p.parent.schema, &p.parent.name));
    }
    let deps = &deps;
    let unlogged = if t.unlogged { "UNLOGGED " } else { "" };
    match live {
        None => {
            let mut sql = match &t.partition_of {
                Some(p) => format!(
                    "CREATE {unlogged}TABLE {q} PARTITION OF {}\n    {}",
                    qn(&p.parent.schema, &p.parent.name),
                    p.bound
                ),
                None => {
                    let mut items: Vec<String> = t
                        .columns
                        .iter()
                        .map(|c| format!("    {}", column_def(c)))
                        .collect();
                    items.extend(
                        t.constraints
                            .iter()
                            .filter(|c| c.kind != ConstraintKind::ForeignKey)
                            .map(|c| format!("    CONSTRAINT {} {}", qi(&c.name), c.definition)),
                    );
                    if items.is_empty() {
                        format!("CREATE {unlogged}TABLE {q} ()")
                    } else {
                        format!("CREATE {unlogged}TABLE {q} (\n{}\n)", items.join(",\n"))
                    }
                }
            };
            if let Some(p) = &t.partition_by {
                sql.push_str(&format!("\n    PARTITION BY {p}"));
            }
            if !t.options.is_empty() {
                sql.push_str(&format!("\n    WITH ({})", t.options.join(", ")));
            }
            sql.push(';');
            out.push(2, key.clone(), deps, sql);
            if t.partition_of.is_some() {
                for c in &t.constraints {
                    if c.kind != ConstraintKind::ForeignKey {
                        out.push(
                            2,
                            key.clone(),
                            deps,
                            format!(
                                "ALTER TABLE {q} ADD CONSTRAINT {} {};",
                                qi(&c.name),
                                c.definition
                            ),
                        );
                    }
                }
            }
        }
        Some(l) => alter_table(out, &key, deps, &q, t, l),
    }
    comment(
        out,
        2,
        &key,
        &format!("TABLE {q}"),
        &t.comment,
        live.map(|l| &l.comment),
    );
    for c in &t.columns {
        let have = live.and_then(|l| l.columns.iter().find(|h| h.name == c.name));
        comment(
            out,
            2,
            &key,
            &format!("COLUMN {q}.{}", qi(&c.name)),
            &c.comment,
            have.map(|h| &h.comment),
        );
    }
    constraints(out, &r, &q, t, live);
    indexes(out, &r, schema, &t.indexes, live.map(|l| &l.indexes[..]));
    triggers_rules(
        out,
        &r,
        &q,
        &t.triggers,
        &t.rules,
        live.map(|l| (&l.triggers[..], &l.rules[..])),
    );
    policies(out, &r, &q, t, live);
}

fn alter_table(out: &mut Script, key: &Key, deps: &[ObjRef], q: &str, t: &Table, l: &Table) {
    let mut push = |sql: String| out.push(2, key.clone(), deps, sql);
    if t.unlogged != l.unlogged {
        push(format!(
            "ALTER TABLE {q} SET {};",
            if t.unlogged { "UNLOGGED" } else { "LOGGED" }
        ));
    }
    for c in &t.columns {
        let Some(h) = l.columns.iter().find(|h| h.name == c.name) else {
            push(format!("ALTER TABLE {q} ADD COLUMN {};", column_def(c)));
            continue;
        };
        let col = format!("ALTER TABLE {q} ALTER COLUMN {}", qi(&c.name));
        if c.generated != h.generated || c.virtual_ != h.virtual_ {
            // a generation expression cannot be changed on every version: re-create
            push(format!("ALTER TABLE {q} DROP COLUMN {};", qi(&c.name)));
            push(format!("ALTER TABLE {q} ADD COLUMN {};", column_def(c)));
            continue;
        }
        if c.data_type != h.data_type || c.collation != h.collation {
            push(format!(
                "{col} TYPE {}{};",
                c.data_type,
                collate(&c.collation)
            ));
        }
        if c.default != h.default && c.generated.is_none() {
            push(match &c.default {
                Some(d) => format!("{col} SET DEFAULT {d};"),
                None => format!("{col} DROP DEFAULT;"),
            });
        }
        if c.not_null && !h.not_null {
            push(format!("{col} SET NOT NULL;"));
        }
        match (&c.identity, &h.identity) {
            (Some(i), None) => push(format!(
                "{col} ADD GENERATED {} AS IDENTITY;",
                i.to_uppercase()
            )),
            (None, Some(_)) => push(format!("{col} DROP IDENTITY;")),
            (Some(i), Some(j)) if i != j => {
                push(format!("{col} SET GENERATED {};", i.to_uppercase()))
            }
            _ => {}
        }
        if !c.not_null && h.not_null {
            push(format!("{col} DROP NOT NULL;"));
        }
    }
    let set: Vec<&String> = t
        .options
        .iter()
        .filter(|o| !l.options.contains(o))
        .collect();
    if !set.is_empty() {
        let set: Vec<&str> = set.iter().map(|s| s.as_str()).collect();
        push(format!("ALTER TABLE {q} SET ({});", set.join(", ")));
    }
    let reset: Vec<&str> = l
        .options
        .iter()
        .map(|o| option_name(o))
        .filter(|n| !t.options.iter().any(|o| option_name(o) == *n))
        .collect();
    if !reset.is_empty() {
        push(format!("ALTER TABLE {q} RESET ({});", reset.join(", ")));
    }
}

fn constraints(out: &mut Script, r: &ObjRef, q: &str, t: &Table, live: Option<&Table>) {
    for c in &t.constraints {
        let have = live.and_then(|l| l.constraints.iter().find(|h| h.name == c.name));
        let fk = c.kind == ConstraintKind::ForeignKey;
        let (phase, key) = if fk {
            (4, Key::Part(r.clone(), format!("fk {}", c.name)))
        } else {
            (2, Key::Obj(r.clone()))
        };
        let add = format!(
            "ALTER TABLE {q} ADD CONSTRAINT {} {};",
            qi(&c.name),
            c.definition
        );
        match (live, have) {
            // a new table has its other constraints in CREATE TABLE
            (None, _) => {
                if fk {
                    out.push(phase, key.clone(), &[], add);
                }
            }
            (Some(_), None) => out.push(phase, key.clone(), &[], add),
            (Some(_), Some(h)) if h.definition != c.definition => {
                out.push(
                    phase,
                    key.clone(),
                    &[],
                    format!("ALTER TABLE {q} DROP CONSTRAINT {};", qi(&c.name)),
                );
                out.push(phase, key.clone(), &[], add);
            }
            _ => {}
        }
        comment(
            out,
            phase,
            &key,
            &format!("CONSTRAINT {} ON {q}", qi(&c.name)),
            &c.comment,
            have.map(|h| &h.comment),
        );
    }
}

/// Definition and comment of a part that is one whole statement (index, trigger,
/// rule).
type Stmt<'a> = (&'a str, &'a Option<String>);

/// Creates the part when it is missing, drops and creates it again when its definition
/// differs; then its comment. `what` names it for `DROP` and `COMMENT ON`
/// (`INDEX s.i`, `TRIGGER t ON s.tbl`).
fn statement_part(
    out: &mut Script,
    phase: u8,
    key: &Key,
    what: &str,
    create: String,
    want: Stmt<'_>,
    have: Option<Stmt<'_>>,
) {
    match have {
        None => out.push(phase, key.clone(), &[], create),
        Some((definition, _)) if definition != want.0 => {
            out.push(phase, key.clone(), &[], format!("DROP {what};"));
            out.push(phase, key.clone(), &[], create);
        }
        Some(_) => {}
    }
    comment(out, phase, key, what, want.1, have.map(|h| h.1));
}

fn indexes(out: &mut Script, r: &ObjRef, schema: &str, want: &[Index], live: Option<&[Index]>) {
    for i in want {
        let have = live.and_then(|l| l.iter().find(|h| h.name == i.name));
        statement_part(
            out,
            5,
            &Key::Part(r.clone(), format!("index {}", i.name)),
            &format!("INDEX {}", qn(schema, &i.name)),
            format!("{};", i.definition),
            (&i.definition, &i.comment),
            have.map(|h| (h.definition.as_str(), &h.comment)),
        );
    }
}

type Parts<'a> = (&'a [Trigger], &'a [Rule]);

fn triggers_rules(
    out: &mut Script,
    r: &ObjRef,
    q: &str,
    triggers: &[Trigger],
    rules: &[Rule],
    live: Option<Parts<'_>>,
) {
    for t in triggers {
        let have = live.and_then(|l| l.0.iter().find(|h| h.name == t.name));
        statement_part(
            out,
            6,
            &Key::Part(r.clone(), format!("trigger {}", t.name)),
            &format!("TRIGGER {} ON {q}", qi(&t.name)),
            format!("{};", t.definition),
            (&t.definition, &t.comment),
            have.map(|h| (h.definition.as_str(), &h.comment)),
        );
    }
    for u in rules {
        let have = live.and_then(|l| l.1.iter().find(|h| h.name == u.name));
        statement_part(
            out,
            6,
            &Key::Part(r.clone(), format!("rule {}", u.name)),
            &format!("RULE {} ON {q}", qi(&u.name)),
            // pg_get_ruledef ends with its own semicolon
            u.definition.trim_end().to_owned(),
            (&u.definition, &u.comment),
            have.map(|h| (h.definition.as_str(), &h.comment)),
        );
    }
}

fn policies(out: &mut Script, r: &ObjRef, q: &str, t: &Table, live: Option<&Table>) {
    let key = Key::Part(r.clone(), "security".into());
    let (rls, force) = live.map_or((false, false), |l| (l.row_security, l.force_row_security));
    if t.row_security != rls {
        let op = if t.row_security { "ENABLE" } else { "DISABLE" };
        out.push(
            6,
            key.clone(),
            &[],
            format!("ALTER TABLE {q} {op} ROW LEVEL SECURITY;"),
        );
    }
    if t.force_row_security != force {
        let op = if t.force_row_security {
            "FORCE"
        } else {
            "NO FORCE"
        };
        out.push(
            6,
            key.clone(),
            &[],
            format!("ALTER TABLE {q} {op} ROW LEVEL SECURITY;"),
        );
    }
    for p in &t.policies {
        let have = live.and_then(|l| l.policies.iter().find(|h| h.name == p.name));
        let roles = if p.roles.is_empty() {
            "PUBLIC".to_owned()
        } else {
            p.roles.iter().map(|r| qi(r)).collect::<Vec<_>>().join(", ")
        };
        let mut create = format!(
            "CREATE POLICY {} ON {q} AS {} FOR {} TO {roles}",
            qi(&p.name),
            if p.restrictive {
                "RESTRICTIVE"
            } else {
                "PERMISSIVE"
            },
            p.command
        );
        if let Some(u) = &p.using {
            create.push_str(&format!(" USING ({u})"));
        }
        if let Some(w) = &p.with_check {
            create.push_str(&format!(" WITH CHECK ({w})"));
        }
        create.push(';');
        match have {
            None => out.push(6, key.clone(), &[], create),
            Some(h) if h != p => {
                out.push(
                    6,
                    key.clone(),
                    &[],
                    format!("DROP POLICY {} ON {q};", qi(&p.name)),
                );
                out.push(6, key.clone(), &[], create);
            }
            Some(_) => {}
        }
    }
}

// ---- views --------------------------------------------------------------------------

fn view(out: &mut Script, schema: &str, v: &View, live: Option<&View>, materialized: bool) {
    let kind = if materialized {
        ObjKind::MaterializedView
    } else {
        ObjKind::View
    };
    let r = obj(kind, schema, &v.name);
    let key = Key::Obj(r.clone());
    let q = qn(schema, &v.name);
    let with = if v.options.is_empty() {
        String::new()
    } else {
        format!(" WITH ({})", v.options.join(", "))
    };
    let body = v.definition.trim_end().trim_end_matches(';');
    let what = if materialized {
        "MATERIALIZED VIEW"
    } else {
        "VIEW"
    };
    let changed = live.is_some_and(|l| l.definition != v.definition || l.options != v.options);
    // a materialized view cannot be replaced: drop it (its indexes go with it)
    let recreated = materialized && changed;
    match live {
        None => {}
        Some(_) if recreated => {
            out.push(
                2,
                key.clone(),
                &v.depends_on,
                format!("DROP MATERIALIZED VIEW {q};"),
            );
        }
        Some(_) if !changed => {}
        Some(_) => {
            out.push(
                2,
                key.clone(),
                &v.depends_on,
                format!("CREATE OR REPLACE VIEW {q}{with} AS\n{body};"),
            );
        }
    }
    if live.is_none() || recreated {
        let tail = if materialized { "\n  WITH DATA" } else { "" };
        out.push(
            2,
            key.clone(),
            &v.depends_on,
            format!("CREATE {what} {q}{with} AS\n{body}{tail};"),
        );
    }
    let live = if recreated { None } else { live };
    comment(
        out,
        2,
        &key,
        &format!("{what} {q}"),
        &v.comment,
        live.map(|l| &l.comment),
    );
    indexes(out, &r, schema, &v.indexes, live.map(|l| &l.indexes[..]));
    triggers_rules(
        out,
        &r,
        &q,
        &v.triggers,
        &v.rules,
        live.map(|l| (&l.triggers[..], &l.rules[..])),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str) -> Column {
        Column {
            name: name.into(),
            data_type: ty.into(),
            ..Default::default()
        }
    }

    fn db(schemas: Vec<Schema>) -> Database {
        Database {
            extensions: vec![],
            schemas,
        }
    }

    fn schema(name: &str) -> Schema {
        Schema {
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn equal_structures_need_nothing() {
        let mut s = schema("public");
        s.tables.push(Table {
            name: "t".into(),
            columns: vec![col("id", "integer")],
            ..Default::default()
        });
        let d = db(vec![s]);
        assert_eq!(sync_script(&d, &d), None);
        assert_eq!(sync_script(&db(vec![]), &db(vec![])), None);
    }

    #[test]
    fn objects_only_in_the_database_are_kept() {
        let mut live = schema("public");
        live.tables.push(Table {
            name: "old".into(),
            columns: vec![col("id", "integer"), col("extra", "text")],
            ..Default::default()
        });
        let mut want = schema("public");
        want.tables.push(Table {
            name: "old".into(),
            columns: vec![col("id", "integer")],
            ..Default::default()
        });
        assert_eq!(sync_script(&db(vec![want]), &db(vec![live])), None);
    }

    #[test]
    fn dependencies_order_the_script() {
        let mut s = schema("app");
        let view_ref = |name: &str, kind| ObjRef {
            kind,
            schema: "app".into(),
            name: name.into(),
            args: None,
        };
        // names chosen so that alphabetical order would be wrong
        s.views.push(View {
            name: "a_view".into(),
            definition: " SELECT 1 FROM app.z_table;".into(),
            depends_on: vec![view_ref("z_table", ObjKind::Table)],
            ..Default::default()
        });
        s.tables.push(Table {
            name: "z_table".into(),
            columns: vec![col("m", "app.mood")],
            depends_on: vec![view_ref("mood", ObjKind::Type)],
            constraints: vec![Constraint {
                name: "fk".into(),
                kind: ConstraintKind::ForeignKey,
                definition: "FOREIGN KEY (m) REFERENCES app.y(m)".into(),
                references: None,
                comment: None,
            }],
            ..Default::default()
        });
        s.types.push(Type {
            name: "mood".into(),
            kind: TypeKind::Enum {
                labels: vec!["a".into()],
            },
            comment: None,
            depends_on: vec![],
        });
        let text = sync_script(&db(vec![s]), &db(vec![])).unwrap();
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}\n{text}"))
        };
        assert!(at("CREATE SCHEMA app;") < at("CREATE TYPE app.mood"));
        assert!(at("CREATE TYPE app.mood") < at("CREATE TABLE app.z_table"));
        assert!(at("CREATE TABLE app.z_table") < at("CREATE VIEW app.a_view"));
        assert!(at("CREATE VIEW app.a_view") < at("ADD CONSTRAINT fk FOREIGN KEY"));
        assert!(!text.contains("check_function_bodies"), "no functions");
    }

    #[test]
    fn changes_to_existing_objects() {
        let mut live = schema("public");
        live.types.push(Type {
            name: "mood".into(),
            kind: TypeKind::Enum {
                labels: vec!["sad".into(), "happy".into()],
            },
            comment: None,
            depends_on: vec![],
        });
        live.tables.push(Table {
            name: "t".into(),
            columns: vec![
                Column {
                    default: Some("1".into()),
                    ..col("a", "integer")
                },
                col("b", "text"),
            ],
            ..Default::default()
        });
        let mut want = live.clone();
        want.types[0].kind = TypeKind::Enum {
            labels: vec!["sad".into(), "ok".into(), "happy".into(), "joy".into()],
        };
        let t = &mut want.tables[0];
        t.columns[0].default = None;
        t.columns[0].not_null = true;
        t.columns[1].data_type = "varchar(10)".into();
        t.columns.push(col("c", "date"));
        t.comment = Some("it's new".into());
        let text = sync_script(&db(vec![want]), &db(vec![live])).unwrap();
        for line in [
            "ALTER TYPE public.mood ADD VALUE IF NOT EXISTS 'ok' AFTER 'sad';",
            "ALTER TYPE public.mood ADD VALUE IF NOT EXISTS 'joy' AFTER 'happy';",
            "ALTER TABLE public.t ALTER COLUMN a DROP DEFAULT;",
            "ALTER TABLE public.t ALTER COLUMN a SET NOT NULL;",
            "ALTER TABLE public.t ALTER COLUMN b TYPE varchar(10);",
            "ALTER TABLE public.t ADD COLUMN c date;",
            "COMMENT ON TABLE public.t IS 'it''s new';",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
        assert!(!text.contains("CREATE TABLE"));
    }

    #[test]
    fn changed_partitioning_recreates_the_table_with_a_copy() {
        let mut live = schema("app");
        live.tables.push(Table {
            name: "log".into(),
            columns: vec![col("id", "integer"), col("at", "date"), col("mine", "text")],
            ..Default::default()
        });
        let mut want = schema("app");
        want.tables.push(Table {
            name: "log".into(),
            columns: vec![col("id", "integer"), col("at", "date")],
            partition_by: Some("RANGE (at)".into()),
            ..Default::default()
        });
        want.tables.push(Table {
            name: "log_rest".into(),
            partition_of: Some(PartitionOf {
                parent: QualName {
                    schema: "app".into(),
                    name: "log".into(),
                },
                bound: "DEFAULT".into(),
            }),
            ..Default::default()
        });
        let text = sync_script(&db(vec![want]), &db(vec![live])).unwrap();
        let at = |needle: &str| {
            text.find(needle).unwrap_or_else(|| {
                panic!(
                    "{needle}
{text}"
                )
            })
        };
        let order = [
            "CREATE TABLE app.\"_TableCopy_log\" AS TABLE app.log;",
            "DROP TABLE app.log;",
            "CREATE TABLE app.log (",
            "    mine text
)
    PARTITION BY RANGE (at);",
            "CREATE TABLE app.log_rest PARTITION OF app.log",
            "INSERT INTO app.log (id, at, mine) OVERRIDING SYSTEM VALUE",
            "-- DROP TABLE app.\"_TableCopy_log\";",
        ];
        for w in order.windows(2) {
            assert!(
                at(w[0]) < at(w[1]),
                "{} before {}
{text}",
                w[0],
                w[1]
            );
        }
        assert!(
            text.trim_end()
                .ends_with("-- DROP TABLE app.\"_TableCopy_log\";")
        );
    }

    #[test]
    fn partitions_changed_with_their_parent_go_through_the_parent() {
        let tables = |by: &str, bound: &str| {
            let mut s = schema("app");
            s.tables.push(Table {
                name: "log".into(),
                columns: vec![col("id", "integer"), col("at", "date")],
                partition_by: Some(by.into()),
                ..Default::default()
            });
            s.tables.push(Table {
                name: "log_2024".into(),
                partition_of: Some(PartitionOf {
                    parent: QualName {
                        schema: "app".into(),
                        name: "log".into(),
                    },
                    bound: bound.into(),
                }),
                ..Default::default()
            });
            s
        };
        let mut live = tables(
            "RANGE (at)",
            "FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')",
        );
        // an index that exists only in the database is kept
        live.tables[1].indexes.push(Index {
            name: "log_2024_mine".into(),
            definition: "CREATE INDEX log_2024_mine ON app.log_2024 USING btree (id)".into(),
            comment: None,
        });
        let want = tables("RANGE (id)", "FOR VALUES FROM (1) TO (1000)");
        let text = sync_script(&db(vec![want]), &db(vec![live])).unwrap();
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}\n{text}"))
        };
        let order = [
            "CREATE TABLE app.\"_TableCopy_log\" AS TABLE app.log;",
            "DROP TABLE app.log;",
            "PARTITION BY RANGE (id);",
            "CREATE TABLE app.log_2024 PARTITION OF app.log\n    FOR VALUES FROM (1) TO (1000);",
            "INSERT INTO app.log (id, at) OVERRIDING SYSTEM VALUE",
            "CREATE INDEX log_2024_mine ON app.log_2024 USING btree (id);",
        ];
        for w in order.windows(2) {
            assert!(at(w[0]) < at(w[1]), "{} before {}\n{text}", w[0], w[1]);
        }
        // the partition is gone with its parent: no copy, no drop, no load of its own
        assert_eq!(text.matches(" AS TABLE ").count(), 1, "{text}");
        assert_eq!(text.matches("INSERT INTO").count(), 1, "{text}");
        assert!(!text.contains("_TableCopy_log_2024"), "{text}");
        assert!(!text.contains("DROP TABLE app.log_2024"), "{text}");
    }

    #[test]
    fn functions_are_replaced_or_recreated() {
        let f = |result: &str, body: &str| Function {
            name: "f".into(),
            args: "x integer".into(),
            result: Some(result.into()),
            definition: format!(
                "CREATE OR REPLACE FUNCTION public.f(x integer)\n RETURNS {result}\n{body}\n"
            ),
            ..Default::default()
        };
        let mut live = schema("public");
        live.functions.push(f("integer", "RETURN 1"));
        let mut want = schema("public");
        want.functions.push(f("integer", "RETURN 2"));
        let text = sync_script(&db(vec![want.clone()]), &db(vec![live.clone()])).unwrap();
        assert!(text.contains("SET check_function_bodies = false;"));
        assert!(text.contains("RETURN 2;"));
        assert!(!text.contains("DROP FUNCTION"));
        want.functions[0] = f("bigint", "RETURN 2");
        let text = sync_script(&db(vec![want]), &db(vec![live])).unwrap();
        assert!(
            text.contains("DROP FUNCTION public.f(x integer);"),
            "{text}"
        );
    }
}
