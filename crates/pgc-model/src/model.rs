//! The structure model (D-012, D-048): Rust structs describing a database's structure,
//! serialised as the `.pgdef` JSON file.
//!
//! - Only the structure: no owners, privileges, versions or timestamps (D-048).
//! - Names are stored as they are in the catalog (unquoted); SQL generation quotes them.
//! - Lists are sorted by name by [`Database::normalize`] (columns keep their order), so
//!   the same structure always gives the same file (D-012).
//! - Empty lists, `false` flags and missing values are left out of the file.

use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A whole database.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Database {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<Extension>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<Schema>,
    /// The roles the database needs: owners, grantees, policy roles and the roles
    /// they are members of (D-048). A model read from a database has every role of
    /// its cluster; the file keeps the needed ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<Role>,
    /// Privileges on the database itself (`CONNECT`, `TEMPORARY`, `CREATE`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub database_grants: Vec<Grant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_privileges: Vec<DefaultPrivilege>,
    /// The role that read a live database: what the script creates belongs to it.
    /// Not in the file.
    #[serde(skip)]
    pub reader: String,
}

/// A role (D-048). Never a password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub superuser: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub login: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub inherit: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub createdb: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub createrole: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub replication: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bypassrls: bool,
    /// `-1`: no limit.
    #[serde(default = "no_limit", skip_serializing_if = "is_no_limit")]
    pub connection_limit: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    /// The roles this one is a member of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_of: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn no_limit() -> i32 {
    -1
}

fn is_no_limit(n: &i32) -> bool {
    *n == -1
}

/// A privilege: to whom (`""` = `PUBLIC`), which (`SELECT`, `USAGE`, ...), and
/// whether it may be passed on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Grant {
    pub grantee: String,
    pub privilege: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub grantable: bool,
}

/// The owner of an object and its privileges — the owner's own not among them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    pub owner: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
}

/// `ALTER DEFAULT PRIVILEGES FOR ROLE role [IN SCHEMA schema] GRANT … ON object`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultPrivilege {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// `TABLES`, `SEQUENCES`, `FUNCTIONS`, `TYPES` or `SCHEMAS`.
    pub object: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extension {
    pub name: String,
    /// Schema the extension's objects are created in.
    pub schema: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Owner and privileges; missing in files written before they were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<Type>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequences: Vec<Sequence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<Table>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<View>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materialized_views: Vec<View>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<Function>,
}

/// Kind of a top-level object, used in dependency references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjKind {
    Type,
    Sequence,
    Table,
    View,
    MaterializedView,
    Function,
}

/// A reference to a top-level object of the model (dependencies, D-048).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjRef {
    pub kind: ObjKind,
    pub schema: String,
    pub name: String,
    /// Identity arguments of a function (`integer, text`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
}

/// A schema-qualified name.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QualName {
    pub schema: String,
    pub name: String,
}

/// A named piece of SQL (a check constraint of a domain).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedDef {
    pub name: String,
    pub definition: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Type {
    pub name: String,
    #[serde(flatten)]
    pub kind: TypeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Owner and privileges; missing in files written before they were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<ObjRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeKind {
    Enum {
        /// An enum may have no labels: the server's JSON then has no such key.
        #[serde(default)]
        labels: Vec<String>,
    },
    Composite {
        /// A composite type may have no attributes.
        #[serde(default)]
        attributes: Vec<Attribute>,
    },
    Domain {
        /// Base type as `format_type` shows it.
        base: String,
        #[serde(default, skip_serializing_if = "is_false")]
        not_null: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collation: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        checks: Vec<NamedDef>,
    },
    Range {
        subtype: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtype_opclass: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collation: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        canonical: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtype_diff: Option<String>,
    },
}

/// An attribute of a composite type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribute {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    pub start: i64,
    pub increment: i64,
    pub min_value: i64,
    pub max_value: i64,
    pub cache: i64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub cycle: bool,
    /// `OWNED BY` column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<ColumnRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Owner and privileges; missing in files written before they were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnRef {
    pub schema: String,
    pub table: String,
    pub column: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub unlogged: bool,
    /// Columns defined in this table (inherited partition columns are left out), in
    /// their order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    /// `PARTITION BY` clause of a partitioned table (`pg_get_partkeydef`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_by: Option<String>,
    /// Parent and bound of a partition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_of: Option<PartitionOf>,
    /// The tables this one inherits (`INHERITS`), in their order; their columns and
    /// constraints are not among this table's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inherits: Vec<QualName>,
    /// Storage parameters (`fillfactor=70`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub row_security: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub force_row_security: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
    /// Indexes that do not belong to a constraint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<Index>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<Policy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Owner and privileges; missing in files written before they were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<ObjRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionOf {
    pub parent: QualName,
    /// `FOR VALUES ...` / `DEFAULT` (`pg_get_expr(relpartbound)`).
    pub bound: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub not_null: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// `always` or `by default`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// The sequence of an identity column. Missing in files written before it
    /// existed: the options are then left as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_options: Option<IdentityOptions>,
    /// Expression of a generated column (stored unless `virtual`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<String>,
    /// A virtual generated column (PostgreSQL 18).
    #[serde(default, rename = "virtual", skip_serializing_if = "is_false")]
    pub virtual_: bool,
    /// Collation when it differs from the type's default (quoted, qualified).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Privileges on this column alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
}

/// The options of an identity column's sequence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityOptions {
    pub start: i64,
    pub increment: i64,
    pub min_value: i64,
    pub max_value: i64,
    pub cache: i64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub cycle: bool,
}

impl IdentityOptions {
    /// What a plain `GENERATED … AS IDENTITY` of a column of `data_type` gets: these
    /// need not be written.
    pub fn is_default(&self, data_type: &str) -> bool {
        let max = match data_type {
            "smallint" => i64::from(i16::MAX),
            "integer" => i64::from(i32::MAX),
            "bigint" => i64::MAX,
            _ => return false,
        };
        *self
            == Self {
                start: 1,
                increment: 1,
                min_value: 1,
                max_value: max,
                cache: 1,
                cycle: false,
            }
    }

    /// `START WITH 10 INCREMENT BY 5 …`, each option after `prefix` (`SET ` in an
    /// `ALTER COLUMN`).
    pub fn clause(&self, prefix: &str) -> String {
        format!(
            "{prefix}START WITH {} {prefix}INCREMENT BY {} {prefix}MINVALUE {} \
             {prefix}MAXVALUE {} {prefix}CACHE {} {prefix}{}",
            self.start,
            self.increment,
            self.min_value,
            self.max_value,
            self.cache,
            if self.cycle { "CYCLE" } else { "NO CYCLE" }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    PrimaryKey,
    Unique,
    ForeignKey,
    Check,
    Exclude,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraint {
    pub name: String,
    pub kind: ConstraintKind,
    /// `pg_get_constraintdef`, e.g. `PRIMARY KEY (id)`.
    pub definition: String,
    /// Referenced table of a foreign key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<QualName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub name: String,
    /// `pg_get_indexdef`: the whole `CREATE INDEX` statement.
    pub definition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    pub name: String,
    /// `pg_get_triggerdef`: the whole `CREATE TRIGGER` statement.
    pub definition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    /// `pg_get_ruledef`: the whole `CREATE RULE` statement.
    pub definition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub restrictive: bool,
    /// `ALL`, `SELECT`, `INSERT`, `UPDATE` or `DELETE`.
    pub command: String,
    /// Role names; empty = `PUBLIC`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub using: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_check: Option<String>,
}

/// A view or a materialized view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub name: String,
    /// The query (`pg_get_viewdef`).
    pub definition: String,
    /// `security_barrier=true`, `check_option=local`, ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Materialized views only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<Index>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Owner and privileges; missing in files written before they were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<ObjRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    /// Identity arguments (`pg_get_function_identity_arguments`), part of the identity.
    #[serde(default)]
    pub args: String,
    /// The types of the input arguments alone (`integer, text`): what makes two
    /// functions of one name the same function, whatever their parameters are
    /// called. Missing in files written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_types: Option<String>,
    /// How many arguments have a default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub defaults: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub procedure: bool,
    /// Result type (`pg_get_function_result`); functions only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The whole `CREATE OR REPLACE` statement (`pg_get_functiondef`).
    pub definition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Owner and privileges; missing in files written before they were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<ObjRef>,
}

impl Database {
    /// Sorts every list by name (columns keep their order) and dependency lists, so the
    /// same structure always serialises the same way (D-012).
    pub fn normalize(&mut self) {
        self.extensions.sort_by(|a, b| a.name.cmp(&b.name));
        self.schemas.sort_by(|a, b| a.name.cmp(&b.name));
        for s in &mut self.schemas {
            s.normalize();
        }
        by_name(&mut self.roles, |r| &r.name);
        for r in &mut self.roles {
            r.member_of.sort();
            r.member_of.dedup();
        }
        self.database_grants.sort();
        for d in &mut self.default_privileges {
            d.grants.sort();
        }
        self.default_privileges
            .sort_by(|a, b| (&a.role, &a.schema, &a.object).cmp(&(&b.role, &b.schema, &b.object)));
    }

    /// The names of the roles the database needs (D-048): owners, grantees, roles
    /// of policies and default privileges, and — the closure — the roles these are
    /// members of; no `PUBLIC`, no predefined `pg_*` role.
    pub fn needed_roles(&self) -> std::collections::BTreeSet<String> {
        let mut named: Vec<&str> = Vec::new();
        for s in &self.schemas {
            let accesses = std::iter::once(&s.access)
                .chain(s.types.iter().map(|t| &t.access))
                .chain(s.sequences.iter().map(|t| &t.access))
                .chain(s.tables.iter().map(|t| &t.access))
                .chain(s.views.iter().map(|t| &t.access))
                .chain(s.materialized_views.iter().map(|t| &t.access))
                .chain(s.functions.iter().map(|t| &t.access));
            for a in accesses.flatten() {
                named.push(&a.owner);
                named.extend(a.grants.iter().map(|g| g.grantee.as_str()));
            }
            for t in &s.tables {
                for c in &t.columns {
                    named.extend(c.grants.iter().map(|g| g.grantee.as_str()));
                }
                for p in &t.policies {
                    named.extend(p.roles.iter().map(String::as_str));
                }
            }
        }
        named.extend(self.database_grants.iter().map(|g| g.grantee.as_str()));
        for d in &self.default_privileges {
            named.push(&d.role);
            named.extend(d.grants.iter().map(|g| g.grantee.as_str()));
        }
        let listed = |n: &str| !n.is_empty() && !n.starts_with("pg_") && n != "public";
        let mut needed: std::collections::BTreeSet<String> = named
            .into_iter()
            .filter(|n| listed(n))
            .map(str::to_owned)
            .collect();
        // the groups of the needed roles, and theirs
        loop {
            let more: Vec<String> = self
                .roles
                .iter()
                .filter(|r| needed.contains(&r.name))
                .flat_map(|r| r.member_of.iter())
                .filter(|g| listed(g) && !needed.contains(*g))
                .cloned()
                .collect();
            if more.is_empty() {
                break;
            }
            needed.extend(more);
        }
        needed
    }

    /// Keeps the roles the database needs, drops the others of the cluster.
    pub fn keep_needed_roles(&mut self) {
        let needed = self.needed_roles();
        self.roles.retain(|r| needed.contains(&r.name));
    }

    pub fn role(&self, name: &str) -> Option<&Role> {
        self.roles.iter().find(|r| r.name == name)
    }

    pub fn schema(&self, name: &str) -> Option<&Schema> {
        self.schemas.iter().find(|s| s.name == name)
    }
}

fn by_name<T>(v: &mut [T], name: impl Fn(&T) -> &str) {
    v.sort_by(|a, b| name(a).cmp(name(b)));
}

fn deps(v: &mut Vec<ObjRef>) {
    v.sort();
    v.dedup();
}

impl Schema {
    fn normalize(&mut self) {
        by_name(&mut self.types, |t| &t.name);
        by_name(&mut self.sequences, |t| &t.name);
        by_name(&mut self.tables, |t| &t.name);
        by_name(&mut self.views, |t| &t.name);
        by_name(&mut self.materialized_views, |t| &t.name);
        self.functions
            .sort_by(|a, b| (&a.name, &a.args).cmp(&(&b.name, &b.args)));
        for t in &mut self.types {
            deps(&mut t.depends_on);
        }
        for t in &mut self.tables {
            by_name(&mut t.constraints, |c| &c.name);
            by_name(&mut t.indexes, |c| &c.name);
            by_name(&mut t.triggers, |c| &c.name);
            by_name(&mut t.rules, |c| &c.name);
            by_name(&mut t.policies, |c| &c.name);
            t.options.sort();
            deps(&mut t.depends_on);
        }
        for v in self.views.iter_mut().chain(&mut self.materialized_views) {
            by_name(&mut v.indexes, |c| &c.name);
            by_name(&mut v.triggers, |c| &c.name);
            by_name(&mut v.rules, |c| &c.name);
            v.options.sort();
            deps(&mut v.depends_on);
        }
        for f in &mut self.functions {
            deps(&mut f.depends_on);
        }
        let sort = |a: &mut Option<Access>| {
            if let Some(a) = a {
                a.grants.sort();
            }
        };
        sort(&mut self.access);
        self.types.iter_mut().for_each(|t| sort(&mut t.access));
        self.sequences.iter_mut().for_each(|t| sort(&mut t.access));
        self.views.iter_mut().for_each(|t| sort(&mut t.access));
        self.materialized_views
            .iter_mut()
            .for_each(|t| sort(&mut t.access));
        self.functions.iter_mut().for_each(|t| sort(&mut t.access));
        for t in &mut self.tables {
            sort(&mut t.access);
            for c in &mut t.columns {
                c.grants.sort();
            }
        }
    }
}
