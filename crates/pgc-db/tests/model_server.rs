//! Reading the structure model (D-048) from a real server. Runs only when
//! `PGTC_TEST_PASSWORD` is set (local server, user postgres); creates and drops the
//! database `pgtc_test_model`.

use pgc_core::ServerVersion;
use pgc_db::model::{ModelQueries, read};
use pgc_model::{ConstraintKind, ObjKind, TypeKind};
use pgc_sql::SqlCatalog;

pub const FIXTURE: &str = r#"
CREATE SCHEMA app;
COMMENT ON SCHEMA app IS 'application';
CREATE TYPE app.mood AS ENUM ('sad', 'ok', 'happy');
CREATE DOMAIN app.positive AS integer NOT NULL DEFAULT 1 CHECK (VALUE > 0);
CREATE TYPE app.pair AS (a integer, b text COLLATE "C");
CREATE TYPE app.floatrange AS RANGE (subtype = float8, subtype_diff = float8mi);
CREATE FUNCTION app.double(x integer) RETURNS integer LANGUAGE sql IMMUTABLE
    RETURN x * 2;
CREATE SEQUENCE app.counter START 10 INCREMENT 5;
CREATE TABLE app.customer (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL,
    mood app.mood DEFAULT 'ok',
    score app.positive,
    twice integer GENERATED ALWAYS AS (app.double(score)) STORED,
    ticket integer DEFAULT nextval('app.counter'),
    best_order integer,
    CONSTRAINT name_not_empty CHECK (name <> '')
);
COMMENT ON TABLE app.customer IS 'customers';
COMMENT ON COLUMN app.customer.name IS 'full name';
ALTER SEQUENCE app.counter OWNED BY app.customer.ticket;
CREATE TABLE app."order" (
    id serial PRIMARY KEY,
    customer_id integer NOT NULL REFERENCES app.customer (id),
    total numeric(10, 2)
) WITH (fillfactor = 70);
ALTER TABLE app.customer ADD CONSTRAINT best_order_fk
    FOREIGN KEY (best_order) REFERENCES app."order" (id);
CREATE INDEX order_total_idx ON app."order" (total DESC);
CREATE FUNCTION app.touch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RETURN NEW;
END $$;
CREATE TRIGGER order_touch BEFORE UPDATE ON app."order"
    FOR EACH ROW EXECUTE FUNCTION app.touch();
CREATE RULE order_no_delete AS ON DELETE TO app."order" DO INSTEAD NOTHING;
ALTER TABLE app."order" ENABLE ROW LEVEL SECURITY;
CREATE POLICY order_mine ON app."order" FOR SELECT USING (total > 0);
CREATE TABLE app.event (id integer, at date NOT NULL) PARTITION BY RANGE (at);
CREATE TABLE app.event_2026 PARTITION OF app.event
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
CREATE VIEW app.big_orders AS
    SELECT o.id, app.double(o.customer_id) AS c2 FROM app."order" o WHERE o.total > 100;
CREATE VIEW app.big_orders_count AS SELECT count(*) AS n FROM app.big_orders;
CREATE MATERIALIZED VIEW app.totals AS
    SELECT customer_id, sum(total) AS total FROM app."order" GROUP BY customer_id;
CREATE INDEX totals_customer ON app.totals (customer_id);
CREATE PROCEDURE app.reset(n integer) LANGUAGE sql AS $$ SELECT n $$;
CREATE TABLE app.history (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at date NOT NULL,
    note text
);
"#;

#[tokio::test]
#[ignore]
async fn model_of_a_real_database() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let connect = |db: &str| {
        let conn = format!("host=localhost user=postgres dbname={db} password={password}");
        async move {
            let (client, conn) = tokio_postgres::connect(&conn, tokio_postgres::NoTls)
                .await
                .expect("connect");
            tokio::spawn(conn);
            client
        }
    };
    let admin = connect("postgres").await;
    admin
        .batch_execute("DROP DATABASE IF EXISTS pgtc_test_model")
        .await
        .unwrap();
    admin
        .batch_execute("CREATE DATABASE pgtc_test_model")
        .await
        .unwrap();
    let client = connect("pgtc_test_model").await;
    client.batch_execute(FIXTURE).await.unwrap();
    let version: i32 = client
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .await
        .unwrap()
        .get(0);
    let queries = ModelQueries::render(&SqlCatalog::new(), ServerVersion(version as u32)).unwrap();
    let result = read(&client, &queries).await;
    drop(client);
    admin
        .batch_execute("DROP DATABASE pgtc_test_model")
        .await
        .unwrap();
    let db = result.unwrap_or_else(|e| panic!("{}", e));
    println!("{}", pgc_model::file::to_text(&db));

    assert!(db.extensions.iter().any(|e| e.name == "plpgsql"));
    let names: Vec<&str> = db.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["app", "public"]);
    let app = db.schema("app").unwrap();
    assert_eq!(app.comment.as_deref(), Some("application"));

    let ty = |n: &str| app.types.iter().find(|t| t.name == n).unwrap();
    assert_eq!(
        ty("mood").kind,
        TypeKind::Enum {
            labels: vec!["sad".into(), "ok".into(), "happy".into()]
        }
    );
    assert!(
        matches!(&ty("positive").kind, TypeKind::Domain { not_null: true, checks, .. } if checks.len() == 1)
    );
    assert!(
        matches!(&ty("pair").kind, TypeKind::Composite { attributes } if attributes[1].collation.as_deref() == Some("pg_catalog.\"C\""))
    );
    assert!(
        matches!(&ty("floatrange").kind, TypeKind::Range { subtype, subtype_diff: Some(d), .. } if subtype == "double precision" && d.contains("float8mi"))
    );

    // identity and serial sequences belong to their column; app.counter is owned by a column
    let seqs: Vec<&str> = app.sequences.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(seqs, ["counter", "order_id_seq"]);
    let counter = &app.sequences[0];
    assert_eq!((counter.start, counter.increment), (10, 5));
    assert_eq!(counter.owned_by.as_ref().unwrap().column, "ticket");

    let table = |n: &str| app.tables.iter().find(|t| t.name == n).unwrap();
    let customer = table("customer");
    let cols: Vec<&str> = customer.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        cols,
        [
            "id",
            "name",
            "mood",
            "score",
            "twice",
            "ticket",
            "best_order"
        ]
    );
    assert_eq!(customer.columns[0].identity.as_deref(), Some("always"));
    assert!(customer.columns[1].not_null);
    assert_eq!(customer.columns[1].comment.as_deref(), Some("full name"));
    assert_eq!(customer.columns[2].data_type, "app.mood");
    assert!(
        customer.columns[4]
            .generated
            .as_deref()
            .unwrap()
            .contains("app.double")
    );
    assert!(customer.columns[4].default.is_none());
    assert!(
        customer.columns[5]
            .default
            .as_deref()
            .unwrap()
            .contains("app.counter")
    );
    let kinds: Vec<(&str, ConstraintKind)> = customer
        .constraints
        .iter()
        .map(|c| (c.name.as_str(), c.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("best_order_fk", ConstraintKind::ForeignKey),
            ("customer_pkey", ConstraintKind::PrimaryKey),
            ("name_not_empty", ConstraintKind::Check),
        ]
    );
    let order = table("order");
    assert_eq!(order.options, ["fillfactor=70"]);
    assert!(order.row_security);
    assert_eq!(
        order.indexes.len(),
        1,
        "the primary key index is not listed"
    );
    assert!(order.indexes[0].definition.contains("app.\"order\""));
    assert_eq!(order.triggers[0].name, "order_touch");
    assert_eq!(order.rules[0].name, "order_no_delete");
    assert_eq!(order.policies[0].command, "SELECT");
    let fk = order
        .constraints
        .iter()
        .find(|c| c.kind == ConstraintKind::ForeignKey);
    assert_eq!(fk.unwrap().references.as_ref().unwrap().name, "customer");
    let event = table("event");
    assert_eq!(event.partition_by.as_deref(), Some("RANGE (at)"));
    let part = table("event_2026");
    assert!(
        part.columns.is_empty(),
        "partition columns come from the parent"
    );
    assert_eq!(part.partition_of.as_ref().unwrap().parent.name, "event");

    let view = app.views.iter().find(|v| v.name == "big_orders").unwrap();
    let on = |deps: &[pgc_model::ObjRef], kind, name: &str| {
        deps.iter().any(|d| d.kind == kind && d.name == name)
    };
    assert!(
        on(&view.depends_on, ObjKind::Table, "order"),
        "{:?}",
        view.depends_on
    );
    assert!(on(&view.depends_on, ObjKind::Function, "double"));
    let count = app
        .views
        .iter()
        .find(|v| v.name == "big_orders_count")
        .unwrap();
    assert!(on(&count.depends_on, ObjKind::View, "big_orders"));
    assert_eq!(app.materialized_views[0].indexes[0].name, "totals_customer");

    // table dependencies: column types, defaults, generated expressions, no foreign keys
    assert!(on(&customer.depends_on, ObjKind::Type, "mood"));
    assert!(on(&customer.depends_on, ObjKind::Type, "positive"));
    assert!(on(&customer.depends_on, ObjKind::Function, "double"));
    assert!(on(&customer.depends_on, ObjKind::Sequence, "counter"));
    assert!(!on(&customer.depends_on, ObjKind::Table, "order"));

    let fnames: Vec<&str> = app.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        fnames,
        ["double", "reset", "touch"],
        "no range constructors"
    );
    let reset = app.functions.iter().find(|f| f.name == "reset").unwrap();
    assert!(reset.procedure);
    assert_eq!(reset.args, "IN n integer");
    assert!(
        reset
            .definition
            .contains("CREATE OR REPLACE PROCEDURE app.reset")
    );
}

async fn connect(db: &str) -> tokio_postgres::Client {
    let password = std::env::var("PGTC_TEST_PASSWORD").unwrap();
    let conn = format!("host=localhost user=postgres dbname={db} password={password}");
    let (client, conn) = tokio_postgres::connect(&conn, tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(conn);
    client
}

async fn model_of(client: &tokio_postgres::Client) -> pgc_model::Database {
    let version: i32 = client
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .await
        .unwrap()
        .get(0);
    let queries = ModelQueries::render(&SqlCatalog::new(), ServerVersion(version as u32)).unwrap();
    read(client, &queries)
        .await
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Clone = load into an empty database (D-048): the script runs without errors, and
/// afterwards the database matches the file, so a second load finds nothing to do.
#[tokio::test]
#[ignore]
async fn sync_clones_and_updates_a_real_database() {
    if std::env::var("PGTC_TEST_PASSWORD").is_err() {
        return;
    }
    let admin = connect("postgres").await;
    for db in ["pgtc_test_src", "pgtc_test_dst"] {
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"))
            .await
            .unwrap();
        admin
            .batch_execute(&format!("CREATE DATABASE {db}"))
            .await
            .unwrap();
    }
    let result = async {
        let src = connect("pgtc_test_src").await;
        src.batch_execute(FIXTURE).await.unwrap();
        let want = model_of(&src).await;
        // through the file, as the application does
        let want = pgc_model::file::from_text(&pgc_model::file::to_text(&want)).unwrap();

        let dst = connect("pgtc_test_dst").await;
        let script = pgc_model::sync::sync_script(&want, &model_of(&dst).await)
            .expect("an empty database differs");
        println!("{script}");
        if let Err(e) = dst.batch_execute(&script).await {
            return Err(format!("clone script failed: {}", pgc_db::describe(&e)));
        }
        let cloned = model_of(&dst).await;
        if let Some(rest) = pgc_model::sync::sync_script(&want, &cloned) {
            return Err(format!("still differs after the clone:\n{rest}"));
        }

        // change the source; the target gets only the changes
        src.batch_execute(
            "ALTER TYPE app.mood ADD VALUE 'great' AFTER 'happy';
             ALTER TABLE app.customer ADD COLUMN email text DEFAULT 'none';
             ALTER TABLE app.customer ALTER COLUMN name DROP NOT NULL;
             CREATE INDEX customer_email ON app.customer (email);
             CREATE OR REPLACE VIEW app.big_orders AS
                 SELECT o.id, app.double(o.customer_id) AS c2, o.total
                 FROM app.\"order\" o WHERE o.total > 200;
             CREATE TABLE app.note (id int PRIMARY KEY, customer int REFERENCES app.customer(id));",
        )
        .await
        .unwrap();
        // the partitioning of a table with data changes: it is re-created
        src.batch_execute(
            "DROP TABLE app.history;
             CREATE TABLE app.history (
                 id integer GENERATED ALWAYS AS IDENTITY,
                 at date NOT NULL,
                 note text,
                 PRIMARY KEY (id, at)
             ) PARTITION BY RANGE (at);
             CREATE TABLE app.history_rest PARTITION OF app.history DEFAULT;",
        )
        .await
        .unwrap();
        dst.batch_execute(
            "ALTER TABLE app.history ADD COLUMN extra text DEFAULT 'kept';
             INSERT INTO app.history (at, note) SELECT DATE '2026-01-01' + g, 'n' || g
                 FROM generate_series(1, 50) g;",
        )
        .await
        .unwrap();
        // something only in the target stays
        dst.batch_execute("CREATE TABLE app.local_only (x int)")
            .await
            .unwrap();
        let want = model_of(&src).await;
        let script =
            pgc_model::sync::sync_script(&want, &model_of(&dst).await).expect("the source changed");
        println!("{script}");
        assert!(
            !script.contains("CREATE TABLE app.customer"),
            "only changes"
        );
        assert!(!script.contains("local_only"));
        if let Err(e) = dst.batch_execute(&script).await {
            return Err(format!("update script failed: {}", pgc_db::describe(&e)));
        }
        let updated = model_of(&dst).await;
        if let Some(rest) = pgc_model::sync::sync_script(&want, &updated) {
            return Err(format!("still differs after the update:\n{rest}"));
        }
        assert!(
            updated
                .schema("app")
                .unwrap()
                .tables
                .iter()
                .any(|t| t.name == "local_only")
        );
        assert!(
            script.contains("CREATE TABLE app.\"_TableCopy_history\" AS TABLE app.history;"),
            "{script}"
        );
        assert!(script.contains("-- DROP TABLE app.\"_TableCopy_history\";"));
        let count = |sql: &'static str| {
            let dst = &dst;
            async move { dst.query_one(sql, &[]).await.unwrap().get::<_, i64>(0) }
        };
        assert_eq!(
            count("SELECT count(*) FROM app.history WHERE extra = 'kept'").await,
            50,
            "the rows and the database-only column survive"
        );
        assert_eq!(
            count("SELECT count(*) FROM app.\"_TableCopy_history\"").await,
            50,
            "the copy stays until the user drops it"
        );
        let next: i32 = dst
            .query_one(
                "INSERT INTO app.history (at) VALUES (DATE '2026-05-01') RETURNING id",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(next, 51, "the identity continues after the loaded rows");
        Ok(())
    }
    .await;
    for db in ["pgtc_test_src", "pgtc_test_dst"] {
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"))
            .await
            .unwrap();
    }
    if let Err(e) = result {
        panic!("{e}");
    }
}
