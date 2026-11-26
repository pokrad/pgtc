//! The execution plan of a logged statement (D-075): plain EXPLAIN with the logged
//! parameter values, the generic plan without them, one statement only, nothing
//! executed. Runs only when `PGTC_TEST_PASSWORD` is set (local server, user postgres).

use std::sync::Arc;
use std::time::Duration;

use pgc_core::Feature;
use pgc_core::settings::SavedConnection;
use pgc_db::{ConnectParams, DbEvent, EventSink, ExplainRequest, Plan};

#[tokio::test]
#[ignore]
async fn plans_of_logged_statements() {
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
    let version = next(&mut rx, |e| match e {
        DbEvent::Connected { version, .. } => Some(version),
        _ => None,
    })
    .await;
    let generic = version.has(Feature::ExplainGenericPlan);
    let some = |s: &str| Some(s.to_owned());
    let mut key = 0;
    let mut explain = |statement: &str, params: Vec<Option<String>>| {
        key += 1;
        handle.explain(ExplainRequest {
            key,
            statement: statement.to_owned(),
            params,
            generic,
        });
    };
    let plan = |e| match e {
        DbEvent::Plan { result, .. } => Some(result),
        _ => None,
    };

    // no parameters
    explain("SELECT * FROM pg_class WHERE relname = 'pg_class';", vec![]);
    let p: Plan = next(&mut rx, plan).await.unwrap();
    assert!(!p.generic);
    assert!(p.lines[0].contains("pg_class"), "{:?}", p.lines);

    // the logged values, for parameters of any type (oid, name, bool, NULL)
    let with_params = "SELECT * FROM pg_class \
                       WHERE oid = $1 AND relname = $2 AND relisshared = $3 AND reltablespace = $4";
    explain(
        with_params,
        vec![some("1259"), some("pg_class"), some("f"), None],
    );
    let p = next(&mut rx, plan).await.unwrap();
    assert!(!p.generic);
    let text = p.lines.join("\n");
    assert!(text.contains("1259"), "the value is in the plan: {text}");
    assert!(!text.contains("$1"), "{text}");

    // a value that only looks cut (log_parameter_max_length is -1 by default)
    explain("SELECT $1::text", vec![some("abc...")]);
    assert!(!next(&mut rx, plan).await.unwrap().generic);

    // no values: the generic plan where the server has it, else a message
    explain(with_params, vec![]);
    match next(&mut rx, plan).await {
        Ok(p) => {
            assert!(generic);
            assert!(p.generic);
            assert!(p.lines.join("\n").contains("$1"), "{:?}", p.lines);
        }
        Err(e) => {
            assert!(!generic, "{e}");
            assert!(e.to_string().contains("PostgreSQL 16"), "{e}");
        }
    }

    // a value the type does not take: the server's error
    explain("SELECT * FROM pg_class WHERE oid = $1", vec![some("x")]);
    assert!(next(&mut rx, plan).await.is_err());

    // two statements, a statement that cannot be planned: refused by the server
    explain("SELECT 1; SELECT 2", vec![]);
    assert!(next(&mut rx, plan).await.is_err());
    explain("SET work_mem = '8MB'", vec![]);
    assert!(next(&mut rx, plan).await.is_err());

    // DML is planned, not executed; the session is usable after the errors
    explain("CREATE TABLE pgtc_explain_test AS SELECT 1 AS x", vec![]);
    next(&mut rx, plan).await.unwrap();
    explain("SELECT * FROM pgtc_explain_test", vec![]);
    let e = next(&mut rx, plan).await.unwrap_err();
    assert!(e.to_string().contains("pgtc_explain_test"), "{e}");
}
