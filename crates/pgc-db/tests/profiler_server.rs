//! The SQL profiler's read-only queries (D-058) run on a real server and return text
//! columns only. Nothing is changed on the server. Runs only when `PGTC_TEST_PASSWORD`
//! is set (local server, user postgres).

use pgc_core::ServerVersion;
use pgc_sql::{RenderContext, SqlCatalog};

#[tokio::test]
#[ignore]
async fn profiler_queries_run_on_a_real_server() {
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
    let catalog = SqlCatalog::new();
    let sql = catalog.render("profiler", "check.sql", ctx).unwrap();
    let row = client.query_one(&sql, &[]).await.unwrap();
    assert_eq!(row.len(), 6);
    for i in 0..row.len() {
        row.try_get::<_, Option<String>>(i)
            .unwrap_or_else(|e| panic!("check.sql column {i}: {e}"));
    }
    let sql = catalog.render("profiler", "read.sql", ctx).unwrap();
    for format in ["csvlog", "jsonlog"] {
        // only find the current file
        let none: Option<String> = None;
        let fmt = Some(format.to_owned());
        let offset = Some("0".to_owned());
        let len = Some("100".to_owned());
        let row = client
            .query_one(&sql, &[&fmt, &none, &offset, &len])
            .await
            .unwrap_or_else(|e| panic!("read.sql {format}: {}", pgc_db::describe(&e)));
        assert_eq!(row.len(), 4);
        let current: Option<String> = row.get(0);
        // and read the start of it, when the server writes that format
        if let Some(file) = current {
            let file = Some(file);
            let row = client
                .query_one(&sql, &[&fmt, &file, &offset, &len])
                .await
                .unwrap();
            let data: Option<String> = row.get(3);
            assert!(data.is_some(), "{format}: data of the current file");
        }
    }
}
