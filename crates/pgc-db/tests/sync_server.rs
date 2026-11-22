//! Sync scripts (D-048) for changes `ALTER` cannot make, run on a real server: the
//! script must run without an error and leave nothing that still differs. Runs only
//! when `PGTC_TEST_PASSWORD` is set (local server, user postgres); creates and drops
//! the databases `pgtc_test_sync_src` and `pgtc_test_sync_dst`.

use pgc_core::ServerVersion;
use pgc_db::model::{ModelQueries, read};
use pgc_sql::SqlCatalog;

const SRC: &str = "pgtc_test_sync_src";
const DST: &str = "pgtc_test_sync_dst";

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
    let db = read(client, &queries)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    // through the file, as the application does
    pgc_model::file::from_text(&pgc_model::file::to_text(&db)).unwrap()
}

/// Both databases start with `before` (the target also gets `only_target`), the
/// source then gets `change`. Returns the script that was run on the target, or why
/// it failed or what still differed after it.
async fn sync(before: &str, only_target: &str, change: &str) -> Result<String, String> {
    sync_then(before, only_target, change, "")
        .await
        .map(|(script, _)| script)
}

/// [`sync`], and then `after` on the target: a query with one text value, which is
/// returned with the script.
async fn sync_then(
    before: &str,
    only_target: &str,
    change: &str,
    after: &str,
) -> Result<(String, String), String> {
    let admin = connect("postgres").await;
    let recreate = |create: bool| {
        let admin = &admin;
        async move {
            for db in [SRC, DST] {
                admin
                    .batch_execute(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"))
                    .await
                    .unwrap();
                if create {
                    admin
                        .batch_execute(&format!("CREATE DATABASE {db}"))
                        .await
                        .unwrap();
                }
            }
        }
    };
    recreate(true).await;
    let result = async {
        let (src, dst) = (connect(SRC).await, connect(DST).await);
        let run = |c: &'static str, sql: String| {
            let (src, dst) = (&src, &dst);
            async move {
                let client = if c == "source" { src } else { dst };
                client
                    .batch_execute(&sql)
                    .await
                    .map_err(|e| format!("{c}: {}\n{sql}", pgc_db::describe(&e)))
            }
        };
        run("source", before.to_owned()).await?;
        run("target", before.to_owned()).await?;
        run("target", only_target.to_owned()).await?;
        run("source", change.to_owned()).await?;
        let want = model_of(&src).await;
        let script = pgc_model::sync::sync_script(&want, &model_of(&dst).await)
            .ok_or("the script is empty: nothing was found to differ")?;
        println!("{script}");
        run("target", script.clone()).await?;
        if let Some(rest) = pgc_model::sync::sync_script(&want, &model_of(&dst).await) {
            return Err(format!(
                "still differs after the script:\n{rest}\n--- after:\n{script}"
            ));
        }
        let value = if after.is_empty() {
            String::new()
        } else {
            dst.query_one(after, &[])
                .await
                .map_err(|e| format!("{after}: {}", pgc_db::describe(&e)))?
                .get::<_, String>(0)
        };
        Ok((script, value))
    }
    .await;
    recreate(false).await;
    result
}

/// One test, so that the cases do not use the two databases at the same time.
#[tokio::test]
#[ignore]
async fn sync_makes_the_changes_alter_cannot() {
    if std::env::var("PGTC_TEST_PASSWORD").is_err() {
        return;
    }

    // a parameter gets another name: CREATE OR REPLACE would be refused
    let script = sync(
        "CREATE FUNCTION public.f(x integer) RETURNS integer LANGUAGE sql RETURN x + 1;
         CREATE VIEW public.uses_f AS SELECT public.f(1) AS n;",
        "",
        "DROP VIEW public.uses_f;
         DROP FUNCTION public.f(integer);
         CREATE FUNCTION public.f(y integer) RETURNS integer LANGUAGE sql RETURN y + 2;
         CREATE VIEW public.uses_f AS SELECT public.f(1) AS n;",
    )
    .await
    .unwrap_or_else(|e| panic!("renamed parameter: {e}"));
    assert!(
        script.contains("DROP FUNCTION public.f(x integer);"),
        "{script}"
    );
    assert!(script.contains("DROP VIEW public.uses_f;"), "{script}");

    // a default is taken away
    let script = sync(
        "CREATE FUNCTION public.g(x integer DEFAULT 1) RETURNS integer LANGUAGE sql RETURN x;",
        "",
        "DROP FUNCTION public.g(integer);
         CREATE FUNCTION public.g(x integer) RETURNS integer LANGUAGE sql RETURN x;",
    )
    .await
    .unwrap_or_else(|e| panic!("removed default: {e}"));
    assert!(
        script.contains("DROP FUNCTION public.g(x integer);"),
        "{script}"
    );

    // only the body: replaced in place, nothing is dropped
    let script = sync(
        "CREATE FUNCTION public.h(x integer) RETURNS integer LANGUAGE sql RETURN x;
         CREATE VIEW public.uses_h AS SELECT public.h(1) AS n;",
        "",
        "CREATE OR REPLACE FUNCTION public.h(x integer) RETURNS integer LANGUAGE sql RETURN x * 2;",
    )
    .await
    .unwrap_or_else(|e| panic!("changed body: {e}"));
    assert!(!script.contains("DROP"), "{script}");

    // INHERITS: a new child with a column of its own and an index on an inherited
    // one; an existing table that gets a parent
    let script = sync(
        "CREATE TABLE public.parent (id integer NOT NULL, at date);
         CREATE TABLE public.later (id integer NOT NULL, at date, note text);",
        "",
        "CREATE TABLE public.child (extra text) INHERITS (public.parent);
         CREATE INDEX child_at ON public.child (at);
         ALTER TABLE public.later INHERIT public.parent;",
    )
    .await
    .unwrap_or_else(|e| panic!("inherits: {e}"));
    assert!(
        script.contains(
            "CREATE TABLE public.child (\n    extra text\n)\n    INHERITS (public.parent);"
        ),
        "{script}"
    );
    assert!(
        script.contains("ALTER TABLE public.later INHERIT public.parent;"),
        "{script}"
    );

    // identity options: of a new table, and changed on a table that is there
    let script = sync(
        "CREATE TABLE public.there (id bigint GENERATED BY DEFAULT AS IDENTITY, x text);",
        "",
        "CREATE TABLE public.fresh (
             id integer GENERATED ALWAYS AS IDENTITY (START WITH 1000 INCREMENT BY 10),
             plain integer GENERATED ALWAYS AS IDENTITY);
         ALTER TABLE public.there ALTER COLUMN id SET INCREMENT BY 5 SET START WITH 100;",
    )
    .await
    .unwrap_or_else(|e| panic!("identity options: {e}"));
    assert!(
        script
            .contains("id integer GENERATED ALWAYS AS IDENTITY (START WITH 1000 INCREMENT BY 10 "),
        "{script}"
    );
    assert!(
        script.contains("plain integer GENERATED ALWAYS AS IDENTITY NOT NULL"),
        "the usual options are not written: {script}"
    );
    assert!(script.contains("SET INCREMENT BY 5"), "{script}");

    // a column of a table with rows becomes an identity: the next value comes after
    // the rows
    let (script, next) = sync_then(
        "CREATE TABLE public.rows (id integer NOT NULL, x text);",
        "INSERT INTO public.rows VALUES (1, 'a'), (2, 'b'), (7, 'c');",
        "ALTER TABLE public.rows ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY;",
        "INSERT INTO public.rows (x) VALUES ('new') RETURNING id::text",
    )
    .await
    .unwrap_or_else(|e| panic!("identity on a table with rows: {e}"));
    assert_eq!(next, "8", "{script}");

    // a function that is dropped (a parameter gets another name) while tables use
    // it: in a default, a generated column, a check, an index and a policy — also
    // in a column and a table that only the target has. The rows stay.
    let parts = "ALTER TABLE public.t ALTER COLUMN b SET DEFAULT public.calc(1);
         ALTER TABLE public.t ADD COLUMN g integer GENERATED ALWAYS AS (public.calc(a)) STORED;
         ALTER TABLE public.t ADD CONSTRAINT ck CHECK (public.calc(a) > 0);
         CREATE INDEX t_calc ON public.t (public.calc(a));
         CREATE POLICY p ON public.t USING (public.calc(a) > 0);";
    let before = format!(
        "CREATE FUNCTION public.calc(x integer) RETURNS integer LANGUAGE sql IMMUTABLE
             RETURN x + 1;
         CREATE TABLE public.t (a integer, b integer);
         {parts}"
    );
    let change = format!(
        "ALTER TABLE public.t ALTER COLUMN b DROP DEFAULT;
         ALTER TABLE public.t DROP COLUMN g;
         ALTER TABLE public.t DROP CONSTRAINT ck;
         DROP INDEX public.t_calc;
         DROP POLICY p ON public.t;
         DROP FUNCTION public.calc(integer);
         CREATE FUNCTION public.calc(y integer) RETURNS integer LANGUAGE sql IMMUTABLE
             RETURN y + 1;
         {parts}"
    );
    let (script, kept) = sync_then(
        &before,
        "INSERT INTO public.t (a) VALUES (5);
         ALTER TABLE public.t ADD COLUMN only_db integer DEFAULT public.calc(3);
         CREATE TABLE public.only_db (x integer DEFAULT public.calc(2));",
        &change,
        "SELECT pg_catalog.concat_ws(',', a, b, g, only_db,
                pg_catalog.pg_get_expr(d.adbin, d.adrelid),
                (SELECT pg_catalog.pg_get_expr(o.adbin, o.adrelid)
                 FROM pg_catalog.pg_attrdef o WHERE o.adrelid = 'public.only_db'::regclass))
         FROM public.t
             JOIN pg_catalog.pg_attribute at ON at.attrelid = 'public.t'::regclass
                 AND at.attname = 'only_db'
             JOIN pg_catalog.pg_attrdef d ON d.adrelid = at.attrelid AND d.adnum = at.attnum",
    )
    .await
    .unwrap_or_else(|e| panic!("a dropped function that tables use: {e}"));
    assert!(
        script.contains("DROP FUNCTION public.calc(x integer);"),
        "{script}"
    );
    // the row with its values, the generated one computed again, and the defaults of
    // what only the target has
    assert_eq!(kept, "5,2,6,4,calc(3),calc(2)", "{script}");
}
