//! The database statistics query (D-062) runs on a real server and returns five text
//! columns (as `ConnectionHandle::lookup` needs). Runs only when `PGTC_TEST_PASSWORD`
//! is set (local server, user postgres).

use pgc_core::ServerVersion;
use pgc_sql::{RenderContext, SqlCatalog};

#[tokio::test]
#[ignore]
async fn db_stats_query_runs_on_a_real_server() {
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
    let sql = SqlCatalog::new()
        .render("db_stats", "stats.sql", ctx)
        .unwrap();
    let rows = client
        .query(&sql, &[])
        .await
        .unwrap_or_else(|e| panic!("stats.sql: {}", pgc_db::describe(&e)));
    assert!(rows.len() > 30, "{} rows", rows.len());
    let mut keys = Vec::new();
    for r in &rows {
        assert_eq!(r.len(), 5);
        for i in 0..r.len() {
            r.try_get::<_, Option<String>>(i)
                .unwrap_or_else(|e| panic!("column {i}: {e}"));
        }
        keys.push(r.get::<_, String>(0));
    }
    for key in ["read_at", "size", "backends", "xid_age", "cache_hit"] {
        assert!(keys.iter().any(|k| k == key), "{key} missing");
    }
    let size: Option<String> = rows
        .iter()
        .find(|r| r.get::<_, String>(0) == "size")
        .unwrap()
        .get(4);
    assert!(size.unwrap().parse::<u64>().unwrap() > 0);
}
