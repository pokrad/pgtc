//! Find object (D-055) on a real server: `sql/find/` returns the objects with tree
//! paths that match the tree's nodes. Runs only when `PGTC_TEST_PASSWORD` is set
//! (local server, user postgres); everything runs in a transaction that is rolled back.

use pgc_core::ServerVersion;
use pgc_sql::{RenderContext, SqlCatalog};

const FIXTURE: &str = "BEGIN;
    CREATE SCHEMA pgtc_find;
    CREATE TABLE pgtc_find.orders (id int PRIMARY KEY, pgtc_customer int);
    CREATE INDEX pgtc_orders_customer ON pgtc_find.orders (pgtc_customer);
    CREATE TABLE pgtc_find.events (at date, pgtc_kind text) PARTITION BY RANGE (at);
    CREATE TABLE pgtc_find.events_2026 PARTITION OF pgtc_find.events
        FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
    CREATE VIEW pgtc_find.pgtc_open_orders AS SELECT * FROM pgtc_find.orders;
    CREATE FUNCTION pgtc_find.pgtc_count() RETURNS bigint LANGUAGE sql
        AS $$ SELECT count(*) FROM pgtc_find.orders $$;";

/// Runs `file` as the lookup does: every parameter as text. Returns (type, schema,
/// name, path).
async fn find(
    client: &tokio_postgres::Client,
    sql: &str,
    params: [&str; 3],
) -> Vec<(String, String, String, String)> {
    let params: Vec<Option<String>> = params.iter().map(|p| Some((*p).to_owned())).collect();
    let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
        .iter()
        .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect();
    client
        .query(sql, &refs)
        .await
        .expect("find query")
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect()
}

#[tokio::test]
#[ignore]
async fn find_returns_objects_with_tree_paths() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let (client, conn) = tokio_postgres::connect(
        &format!("host=localhost user=postgres dbname=postgres password={password}"),
        tokio_postgres::NoTls,
    )
    .await
    .expect("connect");
    tokio::spawn(conn);
    let version: i32 = client
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .await
        .unwrap()
        .get(0);
    let ctx = RenderContext {
        version: ServerVersion(version as u32),
        show_system: false,
    };
    let sql = SqlCatalog::new();
    let db_sql = sql.render("find", "database.sql", ctx).unwrap();
    let server_sql = sql.render("find", "server.sql", ctx).unwrap();
    client.batch_execute(FIXTURE).await.expect("fixture");
    let oid = |name: &str| {
        let q = format!("SELECT '{name}'::regclass::oid::text");
        let c = &client;
        async move { c.query_one(&q, &[]).await.unwrap().get::<_, String>(0) }
    };
    let nsp: String = client
        .query_one(
            "SELECT oid::text FROM pg_namespace WHERE nspname = 'pgtc_find'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let orders = oid("pgtc_find.orders").await;
    let events = oid("pgtc_find.events").await;
    let part = oid("pgtc_find.events_2026").await;

    // without columns: tables, views, functions, indexes — no columns
    let hits = find(&client, &db_sql, ["%pgtc%", "", "false"]).await;
    let has = |hits: &[(String, String, String, String)], kind: &str, path: &str| {
        hits.iter().any(|h| h.0 == kind && h.3 == path)
    };
    assert!(
        has(
            &hits,
            "view",
            &format!(
                "schemas:{nsp}/views:{}",
                oid("pgtc_find.pgtc_open_orders").await
            )
        ),
        "{hits:?}"
    );
    assert!(
        hits.iter()
            .any(|h| h.0 == "function" && h.1 == "pgtc_find" && h.2 == "pgtc_count()"),
        "{hits:?}"
    );
    assert!(
        has(
            &hits,
            "index",
            &format!(
                "schemas:{nsp}/tables:{orders}/indexes:{}",
                oid("pgtc_find.pgtc_orders_customer").await
            )
        ),
        "{hits:?}"
    );
    assert!(!hits.iter().any(|h| h.0 == "column"), "{hits:?}");

    // with columns: below their table, also in a partition (under Partitions)
    let hits = find(&client, &db_sql, ["%pgtc\\_%", "", "true"]).await;
    assert!(
        has(
            &hits,
            "column",
            &format!("schemas:{nsp}/tables:{orders}/columns:2")
        ),
        "{hits:?}"
    );
    assert!(
        has(
            &hits,
            "column",
            &format!("schemas:{nsp}/tables:{events}/partitions:{part}/columns:2")
        ),
        "{hits:?}"
    );
    // the partition itself is found below its parent
    let hits = find(&client, &db_sql, ["%events\\_2026%", "", "false"]).await;
    assert!(
        has(
            &hits,
            "partition",
            &format!("schemas:{nsp}/tables:{events}/partitions:{part}")
        ),
        "{hits:?}"
    );

    // scope: only below the orders table
    let hits = find(
        &client,
        &db_sql,
        ["%pgtc%", &format!("schemas:{nsp}/tables:{orders}/"), "true"],
    )
    .await;
    assert!(
        !hits.is_empty()
            && hits
                .iter()
                .all(|h| h.3.starts_with(&format!("schemas:{nsp}/tables:{orders}/"))),
        "{hits:?}"
    );
    // scope: only the Views group of the schema
    let hits = find(
        &client,
        &db_sql,
        ["%pgtc%", &format!("schemas:{nsp}/views:"), "false"],
    )
    .await;
    assert_eq!(hits.len(), 1, "{hits:?}");

    // server level: the database postgres
    let hits = find(&client, &server_sql, ["%postgres%", "databases:", "false"]).await;
    assert!(
        hits.iter()
            .any(|h| h.0 == "database" && h.1.is_empty() && h.2 == "postgres"),
        "{hits:?}"
    );

    client.batch_execute("ROLLBACK").await.unwrap();
}
