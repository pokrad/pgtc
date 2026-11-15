//! The server dashboard's queries (D-057) run on a real server and return text
//! columns only (as `ConnectionHandle::lookup_with` needs). Runs only when
//! `PGTC_TEST_PASSWORD` is set (local server, user postgres).

use pgc_core::ServerVersion;
use pgc_sql::{RenderContext, SqlCatalog};

#[tokio::test]
#[ignore]
async fn dashboard_queries_run_on_a_real_server() {
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
    // whole server (NULL) and one database
    for filter in [None, Some("postgres".to_owned())] {
        for (file, columns) in [
            ("stats.sql", 41),
            ("size.sql", 2),
            ("sessions.sql", 20),
            ("blocking.sql", 13),
            ("progress.sql", 11),
            ("locks.sql", 15),
            ("prepared.sql", 5),
        ] {
            let sql = catalog.render("dashboard", file, ctx).unwrap();
            let rows = client
                .query(&sql, &[&filter])
                .await
                .unwrap_or_else(|e| panic!("{file}: {}", pgc_db::describe(&e)));
            if file == "stats.sql" || file == "size.sql" {
                assert_eq!(rows.len(), 1);
            }
            for r in &rows {
                assert_eq!(r.len(), columns, "{file}");
                for i in 0..r.len() {
                    r.try_get::<_, Option<String>>(i)
                        .unwrap_or_else(|e| panic!("{file} column {i}: {e}"));
                }
            }
        }
    }
    let sql = catalog
        .render("dashboard", "configuration.sql", ctx)
        .unwrap();
    let rows = client.query(&sql, &[]).await.unwrap();
    assert!(rows.len() > 100);
    assert_eq!(rows[0].len(), 5);
    // a PID that is no backend: no row, not an error; neither for a session whose
    // backend start is not the one that was shown (its PID was used again)
    let own: Option<String> = client
        .query_one("SELECT pg_catalog.pg_backend_pid()::text", &[])
        .await
        .unwrap()
        .get(0);
    for file in ["cancel.sql", "terminate.sql"] {
        let sql = catalog.render("dashboard", file, ctx).unwrap();
        let pid = Some("1".to_owned());
        let unknown: Option<String> = None;
        assert!(
            client
                .query(&sql, &[&pid, &unknown])
                .await
                .unwrap()
                .is_empty()
        );
        let other = Some("1999-01-01 00:00:00".to_owned());
        assert!(
            client
                .query(&sql, &[&own, &other])
                .await
                .unwrap()
                .is_empty()
        );
    }
}
