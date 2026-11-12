//! Every built-in developer template (D-013, D-046) renders SQL that a real server
//! accepts. Runs only when `PGTC_TEST_PASSWORD` is set (local server, user postgres);
//! each template runs inside a transaction that is rolled back.

use std::collections::BTreeMap;

use pgc_core::ServerVersion;
use pgc_sql::devtemplate::{
    self, Param, ParamType, ParamValue, QualName, Source, TemplateContext, TemplateLibrary,
};

/// Objects the templates work on, created before each template in the transaction.
/// `pgtc_tpl_base` has no dependents, so DROP … RESTRICT works on it.
const FIXTURE: &str = "BEGIN;
    CREATE TABLE public.pgtc_tpl_base (id int);
    CREATE TABLE public.pgtc_tpl_other (id int);
    CREATE VIEW public.pgtc_tpl_view AS SELECT id FROM public.pgtc_tpl_other;
    CREATE MATERIALIZED VIEW public.pgtc_tpl_mview AS SELECT id FROM public.pgtc_tpl_other;
    CREATE INDEX pgtc_tpl_index ON public.pgtc_tpl_other (id);
    CREATE SEQUENCE public.pgtc_tpl_seq;
    CREATE TYPE public.pgtc_tpl_enum AS ENUM ('a');
    CREATE DOMAIN public.pgtc_tpl_domain AS integer;
    CREATE FUNCTION public.pgtc_tpl_fn(n integer) RETURNS integer LANGUAGE sql AS 'SELECT n';
    CREATE PROCEDURE public.pgtc_tpl_proc() LANGUAGE sql AS 'SELECT 1';
    CREATE FUNCTION public.pgtc_tpl_trigger_fn() RETURNS trigger LANGUAGE plpgsql
        AS 'BEGIN RETURN NEW; END';
    CREATE TRIGGER pgtc_tpl_trigger BEFORE INSERT ON public.pgtc_tpl_other
        FOR EACH ROW EXECUTE FUNCTION public.pgtc_tpl_trigger_fn();
    CREATE SCHEMA pgtc_tpl_schema;
    CREATE ROLE pgtc_tpl_role;";

fn object(name: &str) -> ParamValue {
    ParamValue::Object(QualName::new("public", name))
}

fn sample(p: &Param) -> ParamValue {
    let text = |s: &str| ParamValue::Text(s.to_owned());
    match p.kind {
        ParamType::Bool => ParamValue::Bool(p.default_bool()),
        ParamType::Number => text("1"),
        ParamType::Identifier if p.name == "col_name" || p.name == "arg_name" => text("id"),
        ParamType::Identifier => text(&format!("pgtc_tpl_{}", p.name)),
        ParamType::Text if p.name == "col_name" => text("id"),
        ParamType::Text if p.name == "label" => text("a"),
        ParamType::Text if p.required => {
            let d = p.default_text();
            text(if d.is_empty() { "integer" } else { &d })
        }
        ParamType::Text => text(""),
        ParamType::Sql => text(&p.default_text()),
        // the objects of FIXTURE (D-050)
        ParamType::Choice => match p.source {
            Some(Source::Schemas) if p.required => text("pgtc_tpl_schema"),
            Some(Source::Schemas) => text("public"),
            Some(Source::Roles) if p.required => text("pgtc_tpl_role"),
            Some(Source::Roles) => text(""),
            Some(Source::Types) => ParamValue::Object(QualName::new("", "integer")),
            Some(Source::Tables) => object("pgtc_tpl_base"),
            Some(Source::Views) => object("pgtc_tpl_view"),
            Some(Source::MaterializedViews) => object("pgtc_tpl_mview"),
            Some(Source::Sequences) => object("pgtc_tpl_seq"),
            Some(Source::UserTypes) => object("pgtc_tpl_enum"),
            Some(Source::Domains) => object("pgtc_tpl_domain"),
            Some(Source::Indexes) => object("pgtc_tpl_index"),
            Some(Source::Functions) => ParamValue::Object(QualName {
                args: Some("n integer".into()),
                ..QualName::new("public", "pgtc_tpl_fn")
            }),
            Some(Source::Procedures) => ParamValue::Object(QualName {
                args: Some(String::new()),
                ..QualName::new("public", "pgtc_tpl_proc")
            }),
            Some(Source::Triggers) => ParamValue::Object(QualName {
                table: Some("pgtc_tpl_other".into()),
                ..QualName::new("public", "pgtc_tpl_trigger")
            }),
            None => {
                let d = p.default_text();
                text(if d.is_empty() { &p.options[0] } else { &d })
            }
        },
        ParamType::List => ParamValue::List(vec![
            p.fields
                .iter()
                .map(|f| (f.name.clone(), sample(f)))
                .collect(),
        ]),
    }
}

#[tokio::test]
#[ignore]
async fn built_in_templates_run_on_a_real_server() {
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
    let version = ServerVersion(version as u32);
    // the embedded templates, installed into a fresh folder (D-013)
    let dir = tempfile::tempdir().unwrap();
    devtemplate::install(dir.path()).unwrap();
    let lib = TemplateLibrary::scan(dir.path());
    assert!(lib.entries.len() >= 10);
    for e in &lib.entries {
        let (text, origin) = e.resolve(version).unwrap();
        let t = devtemplate::parse(text).unwrap();
        let values: BTreeMap<String, ParamValue> = t
            .params
            .iter()
            .map(|p| (p.name.clone(), sample(p)))
            .collect();
        let ctx = TemplateContext {
            version,
            database: "postgres".into(),
        };
        let sql =
            devtemplate::render(&t, &values, &ctx).unwrap_or_else(|err| panic!("{origin}: {err}"));
        client.batch_execute(FIXTURE).await.unwrap();
        let result = client.batch_execute(&sql).await;
        client.batch_execute("ROLLBACK").await.unwrap();
        if let Err(err) = result {
            panic!(
                "{origin} failed on the server:\n{sql}\n{}",
                pgc_db::describe(&err)
            );
        }
    }
}
