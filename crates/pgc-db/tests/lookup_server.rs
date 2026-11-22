//! pgtc's own queries inside the user's open transaction (D-019): a lookup that fails
//! does not fail the transaction. Runs only when `PGTC_TEST_PASSWORD` is set (local
//! server, user postgres).

use std::sync::Arc;
use std::time::Duration;

use pgc_core::settings::SavedConnection;
use pgc_db::{ConnectParams, DbEvent, EventSink, ExecuteRequest, QueryEvent, TxStatus};

#[tokio::test]
#[ignore]
async fn a_failed_lookup_does_not_fail_the_open_transaction() {
    let Ok(password) = std::env::var("PGTC_TEST_PASSWORD") else {
        return;
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let saved = SavedConnection {
        host: "localhost".into(),
        username: "postgres".into(),
        ..SavedConnection::default()
    };
    let params = ConnectParams::new(&saved, "postgres", Some(password), Duration::from_secs(10));
    let handle = pgc_db::spawn(1, params, sink);
    // the next event of the wanted kind
    async fn next<T>(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
        pick: impl Fn(DbEvent) -> Option<T>,
    ) -> T {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("an answer within 10 s")
                .expect("the connection task is there");
            if let Some(found) = pick(event) {
                return found;
            }
        }
    }
    let finished = |e| match e {
        DbEvent::Query {
            event: QueryEvent::Finished { status, .. },
            ..
        } => Some(status),
        _ => None,
    };
    let lookup = |e| match e {
        DbEvent::Lookup { result, .. } => Some(result),
        _ => None,
    };
    next(&mut rx, |e| {
        matches!(e, DbEvent::Connected { .. }).then_some(())
    })
    .await;

    let execute = |run, sql: &str| {
        handle.execute(ExecuteRequest {
            run,
            sql: sql.to_owned(),
            batch_rows: 1000,
            auto_begin: false,
            auto_rollback: false,
        });
    };
    execute(1, "begin; create temp table pgtc_lookup_test (x int)");
    assert_eq!(next(&mut rx, finished).await, TxStatus::InTransaction);

    // a lookup that fails, without and with bind parameters
    handle.lookup(1, "select 1 / 0".to_owned());
    assert!(next(&mut rx, lookup).await.is_err());
    handle.lookup_with(
        2,
        "select (1 / $1::int)::text".to_owned(),
        vec![Some("0".to_owned())],
    );
    assert!(next(&mut rx, lookup).await.is_err());
    // one that works sees what the transaction has made
    handle.lookup(
        3,
        "select count(*) from pg_temp.pgtc_lookup_test".to_owned(),
    );
    assert_eq!(
        next(&mut rx, lookup).await.unwrap(),
        vec![vec!["0".to_owned()]]
    );

    // the transaction is still usable, and nothing of it is lost
    execute(2, "insert into pgtc_lookup_test values (1)");
    assert_eq!(next(&mut rx, finished).await, TxStatus::InTransaction);
    execute(3, "rollback");
    assert_eq!(next(&mut rx, finished).await, TxStatus::Idle);
    handle.end(Duration::from_secs(1));
}
