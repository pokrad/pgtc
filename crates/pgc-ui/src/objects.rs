//! Object types shown in the tree (D-036): group labels, SQL folders, children.

use crate::icons::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjType {
    // server level
    Database,
    Role,
    Tablespace,
    // database level
    Schema,
    /// pg_catalog / information_schema: a schema in its own group (D-036).
    Catalog,
    Extension,
    EventTrigger,
    Publication,
    Subscription,
    Cast,
    Language,
    ForeignDataWrapper,
    // schema level
    Table,
    View,
    MatView,
    Function,
    Procedure,
    TriggerFunction,
    Sequence,
    Type,
    Domain,
    Aggregate,
    Collation,
    Operator,
    FtsConfiguration,
    FtsDictionary,
    FtsParser,
    FtsTemplate,
    ForeignTable,
    // table level
    Column,
    Constraint,
    Index,
    Trigger,
    Rule,
    Policy,
    Partition,
}

use ObjType as T;

/// Groups under a connected server.
pub const SERVER_GROUPS: &[ObjType] = &[T::Database, T::Role, T::Tablespace];

/// Groups under a database, in alphabetical order as in pgAdmin (D-036).
const DATABASE_GROUPS: &[ObjType] = &[
    T::Cast,
    T::Catalog,
    T::EventTrigger,
    T::Extension,
    T::ForeignDataWrapper,
    T::Language,
    T::Publication,
    T::Schema,
    T::Subscription,
];

const SCHEMA_GROUPS: &[ObjType] = &[
    T::Table,
    T::View,
    T::MatView,
    T::Function,
    T::Procedure,
    T::TriggerFunction,
    T::Sequence,
    T::Type,
    T::Domain,
    T::Aggregate,
    T::Collation,
    T::Operator,
    T::FtsConfiguration,
    T::FtsDictionary,
    T::FtsParser,
    T::FtsTemplate,
    T::ForeignTable,
];

const TABLE_GROUPS: &[ObjType] = &[
    T::Column,
    T::Constraint,
    T::Index,
    T::Trigger,
    T::Rule,
    T::Policy,
    T::Partition,
];

impl ObjType {
    /// Folder under `sql/` (pgAdmin's names).
    pub fn folder(self) -> &'static str {
        match self {
            T::Database => "databases",
            T::Role => "roles",
            T::Tablespace => "tablespaces",
            T::Schema => "schemas",
            T::Catalog => "catalogs",
            T::Extension => "extensions",
            T::EventTrigger => "event_triggers",
            T::Publication => "publications",
            T::Subscription => "subscriptions",
            T::Cast => "casts",
            T::Language => "languages",
            T::ForeignDataWrapper => "foreign_data_wrappers",
            T::Table => "tables",
            T::View => "views",
            T::MatView => "mviews",
            T::Function => "functions",
            T::Procedure => "procedures",
            T::TriggerFunction => "trigger_functions",
            T::Sequence => "sequences",
            T::Type => "types",
            T::Domain => "domains",
            T::Aggregate => "aggregates",
            T::Collation => "collations",
            T::Operator => "operators",
            T::FtsConfiguration => "fts_configurations",
            T::FtsDictionary => "fts_dictionaries",
            T::FtsParser => "fts_parsers",
            T::FtsTemplate => "fts_templates",
            T::ForeignTable => "foreign_tables",
            T::Column => "columns",
            T::Constraint => "constraints",
            T::Index => "indexes",
            T::Trigger => "triggers",
            T::Rule => "rules",
            T::Policy => "policies",
            T::Partition => "partitions",
        }
    }

    /// Label of the group node.
    pub fn group_label(self) -> &'static str {
        match self {
            T::Database => "Databases",
            T::Role => "Login/Group Roles",
            T::Tablespace => "Tablespaces",
            T::Schema => "Schemas",
            T::Catalog => "Catalogs",
            T::Extension => "Extensions",
            T::EventTrigger => "Event Triggers",
            T::Publication => "Publications",
            T::Subscription => "Subscriptions",
            T::Cast => "Casts",
            T::Language => "Languages",
            T::ForeignDataWrapper => "Foreign Data Wrappers",
            T::Table => "Tables",
            T::View => "Views",
            T::MatView => "Materialized Views",
            T::Function => "Functions",
            T::Procedure => "Procedures",
            T::TriggerFunction => "Trigger Functions",
            T::Sequence => "Sequences",
            T::Type => "Types",
            T::Domain => "Domains",
            T::Aggregate => "Aggregates",
            T::Collation => "Collations",
            T::Operator => "Operators",
            T::FtsConfiguration => "FTS Configurations",
            T::FtsDictionary => "FTS Dictionaries",
            T::FtsParser => "FTS Parsers",
            T::FtsTemplate => "FTS Templates",
            T::ForeignTable => "Foreign Tables",
            T::Column => "Columns",
            T::Constraint => "Constraints",
            T::Index => "Indexes",
            T::Trigger => "Triggers",
            T::Rule => "Rules",
            T::Policy => "Policies",
            T::Partition => "Partitions",
        }
    }

    /// Groups below an object of this type. `partitioned` adds Partitions to tables.
    pub fn children(self, partitioned: bool) -> Vec<ObjType> {
        let groups: &[ObjType] = match self {
            T::Database => DATABASE_GROUPS,
            T::Schema | T::Catalog => SCHEMA_GROUPS,
            T::Table | T::Partition => TABLE_GROUPS,
            T::View => &[T::Column, T::Rule, T::Trigger],
            T::MatView => &[T::Column, T::Index],
            T::ForeignTable => &[T::Column],
            _ => &[],
        };
        groups
            .iter()
            .copied()
            .filter(|g| *g != T::Partition || partitioned)
            .collect()
    }

    /// Folder of the `definition.sql` query (partitions use the table query).
    pub fn definition_folder(self) -> &'static str {
        match self {
            T::Partition => "tables",
            T::Catalog => "schemas",
            other => other.folder(),
        }
    }

    /// The definition query also takes `$2` = OID of the parent table (columns have
    /// no OID of their own; their number comes as `$1`).
    pub fn definition_needs_parent(self) -> bool {
        self == T::Column
    }

    /// The nodes query takes `$1` = OID of the parent object (schema or table).
    pub fn takes_parent(self) -> bool {
        matches!(
            self,
            T::Table
                | T::View
                | T::MatView
                | T::Function
                | T::Procedure
                | T::TriggerFunction
                | T::Sequence
                | T::Type
                | T::Domain
                | T::Aggregate
                | T::Collation
                | T::Operator
                | T::FtsConfiguration
                | T::FtsDictionary
                | T::FtsParser
                | T::FtsTemplate
                | T::ForeignTable
                | T::Column
                | T::Constraint
                | T::Index
                | T::Trigger
                | T::Rule
                | T::Policy
                | T::Partition
        )
    }

    pub fn icon(self) -> Icon {
        match self {
            T::Database => Icon::Database,
            T::Table | T::Partition | T::ForeignTable => Icon::Table,
            T::View | T::MatView => Icon::View,
            T::Function | T::Procedure | T::TriggerFunction | T::Aggregate => Icon::Function,
            T::Column => Icon::Column,
            T::Role => Icon::Role,
            _ => Icon::Object,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[ObjType] = &[
        T::Database,
        T::Role,
        T::Tablespace,
        T::Schema,
        T::Catalog,
        T::Extension,
        T::EventTrigger,
        T::Publication,
        T::Subscription,
        T::Cast,
        T::Language,
        T::ForeignDataWrapper,
        T::Table,
        T::View,
        T::MatView,
        T::Function,
        T::Procedure,
        T::TriggerFunction,
        T::Sequence,
        T::Type,
        T::Domain,
        T::Aggregate,
        T::Collation,
        T::Operator,
        T::FtsConfiguration,
        T::FtsDictionary,
        T::FtsParser,
        T::FtsTemplate,
        T::ForeignTable,
        T::Column,
        T::Constraint,
        T::Index,
        T::Trigger,
        T::Rule,
        T::Policy,
        T::Partition,
    ];

    #[test]
    fn every_type_has_a_nodes_query_with_matching_parameters() {
        let catalog = pgc_sql::SqlCatalog::new();
        let ctx = pgc_sql::RenderContext {
            version: pgc_core::ServerVersion(170000),
            show_system: false,
        };
        for t in ALL {
            let sql = catalog
                .render(t.folder(), "nodes.sql", ctx)
                .unwrap_or_else(|e| panic!("{t:?}: {e}"));
            let body: String = sql.lines().filter(|l| !l.starts_with("--")).collect();
            assert_eq!(body.contains("$1"), t.takes_parent(), "{t:?}");
        }
    }

    #[test]
    fn database_groups_are_alphabetical_and_catalogs_are_schemas() {
        let labels: Vec<_> = T::Database
            .children(false)
            .iter()
            .map(|g| g.group_label())
            .collect();
        let mut sorted = labels.clone();
        sorted.sort();
        assert_eq!(labels, sorted);
        assert_eq!(&labels[..2], ["Casts", "Catalogs"]);
        assert_eq!(T::Catalog.children(false), T::Schema.children(false));
        assert_eq!(T::Catalog.definition_folder(), "schemas");
    }

    #[test]
    fn partitions_only_for_partitioned_tables() {
        assert!(!T::Table.children(false).contains(&T::Partition));
        assert!(T::Table.children(true).contains(&T::Partition));
        assert!(T::Column.children(true).is_empty());
    }
}
