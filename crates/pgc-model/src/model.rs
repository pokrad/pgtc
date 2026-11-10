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

/// A whole database.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Database {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<Extension>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<Schema>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<ObjRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeKind {
    Enum {
        labels: Vec<String>,
    },
    Composite {
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<ObjRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    /// Identity arguments (`pg_get_function_identity_arguments`), part of the identity.
    #[serde(default)]
    pub args: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub procedure: bool,
    /// Result type (`pg_get_function_result`); functions only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The whole `CREATE OR REPLACE` statement (`pg_get_functiondef`).
    pub definition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
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
    }
}
