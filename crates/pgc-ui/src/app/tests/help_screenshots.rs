//! Screenshots of the HTML help (`docs/help/img/*.svg`): real screens drawn by pgtc's
//! renderer, staged with made-up data in the shape of the AdventureWorks sample
//! database. Not part of the normal test run:
//!
//! `$env:PGTC_HELP_DIR="$PWD\docs\help\img"; cargo test -p pgc-ui help_screenshots -- --ignored`
//!
//! `PGTC_HELP_TXT` (optional) names a folder for the same screens as plain text.

use super::*;

use pgc_core::settings::SavedConnection;
use pgc_db::TxStatus;
use ratatui::layout::Rect;

use crate::keymap::Command;
use crate::objects::ObjType;
use crate::query_tab::{LineMap, Sent};
use crate::tree::{ConnState, DbConn, NodeKind, RowKind, ServerSettings, Setting};
use crate::view::Hits;

const W: u16 = 120;
const H: u16 = 34;
const DB: &str = "adventureworks";

/// What part of the screen a screenshot shows.
#[derive(Clone, Copy)]
enum Crop {
    Full,
    /// The open dialog with a margin (its shadow included).
    Dialog,
    /// The top-left part: `width` × `height` cells.
    TopLeft(u16, u16),
    Area(u16, u16, u16, u16),
}

/// Draws `a` and returns the SVG of the `crop` part and the same part as text.
fn draw(a: &App, crop: Crop) -> (String, String) {
    let palette = &crate::theme_check::PALETTES[0];
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
    let mut hits = Hits::default();
    terminal
        .draw(|f| crate::view::render(f, a, &mut hits))
        .unwrap();
    let buf = terminal.backend().buffer();
    let area = match crop {
        Crop::Full => Rect::new(0, 0, W, H),
        Crop::TopLeft(w, h) => Rect::new(0, 0, w.min(W), h.min(H)),
        Crop::Area(x, y, w, h) => Rect::new(x, y, w.min(W - x), h.min(H - y)),
        Crop::Dialog => match &hits.dialog {
            Some((r, _)) => {
                // the dialog and its shadow
                let right = (r.right() + 2).min(W);
                let bottom = (r.bottom() + 1).min(H);
                Rect::new(r.x, r.y, right - r.x, bottom - r.y)
            }
            None => Rect::new(0, 0, W, H),
        },
    };
    let (cw, ch) = (9.0_f32, 19.0_f32);
    let hex = |c: (u8, u8, u8)| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
    let (pw, ph) = (area.width as f32 * cw, area.height as f32 * ch);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pw}\" height=\"{ph}\" \
         viewBox=\"0 0 {pw} {ph}\" font-family=\"Cascadia Mono, Consolas, 'DejaVu Sans Mono', Menlo, monospace\" \
         font-size=\"15\" shape-rendering=\"crispEdges\" xml:space=\"preserve\">\n"
    );
    let (_, window) =
        crate::theme_check::shown(a.theme.get(crate::theme::Role::CoreNormal), palette);
    out.push_str(&format!(
        "<rect width=\"{pw}\" height=\"{ph}\" fill=\"{}\"/>\n",
        hex(window)
    ));
    let mut text = String::new();
    for y in area.y..area.bottom() {
        let py = (y - area.y) as f32 * ch;
        // backgrounds as runs of one colour (the window colour is already there)
        let mut x = area.x;
        while x < area.right() {
            let (_, bg) = crate::theme_check::shown(buf[(x, y)].style(), palette);
            let start = x;
            while x < area.right()
                && crate::theme_check::shown(buf[(x, y)].style(), palette).1 == bg
            {
                x += 1;
            }
            if bg != window {
                out.push_str(&format!(
                    "<rect x=\"{}\" y=\"{py}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
                    (start - area.x) as f32 * cw,
                    (x - start) as f32 * cw + 0.5,
                    ch + 0.5,
                    hex(bg)
                ));
            }
        }
        // text as runs of one style; every character keeps its own column
        let style_of = |x: u16| {
            let cell = &buf[(x, y)];
            let (fg, _) = crate::theme_check::shown(cell.style(), palette);
            let m = cell.modifier;
            (
                fg,
                m.contains(ratatui::style::Modifier::BOLD),
                m.contains(ratatui::style::Modifier::UNDERLINED),
                m.contains(ratatui::style::Modifier::ITALIC),
            )
        };
        let mut x = area.x;
        while x < area.right() {
            let sym = buf[(x, y)].symbol();
            text.push_str(if sym.is_empty() { " " } else { sym });
            if sym.trim().is_empty() {
                x += 1;
                continue;
            }
            let style = style_of(x);
            let (mut xs, mut chars) = (String::new(), String::new());
            let mut end = x;
            while end < area.right()
                && !buf[(end, y)].symbol().trim().is_empty()
                && style_of(end) == style
            {
                if end > x {
                    xs.push(' ');
                    text.push_str(buf[(end, y)].symbol());
                }
                xs.push_str(&format!("{}", (end - area.x) as f32 * cw));
                chars.push_str(
                    &buf[(end, y)]
                        .symbol()
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;"),
                );
                end += 1;
            }
            let (fg, bold, under, italic) = style;
            out.push_str(&format!(
                "<text x=\"{xs}\" y=\"{}\" fill=\"{}\"{}{}{}>{chars}</text>\n",
                py + ch * 0.75,
                hex(fg),
                if bold { " font-weight=\"700\"" } else { "" },
                if under {
                    " text-decoration=\"underline\""
                } else {
                    ""
                },
                if italic { " font-style=\"italic\"" } else { "" },
            ));
            x = end;
        }
        text.push('\n');
    }
    out.push_str("</svg>\n");
    (out, text)
}

/// Where the screenshots go.
struct Out {
    dir: std::path::PathBuf,
    txt: Option<std::path::PathBuf>,
    names: std::cell::RefCell<Vec<String>>,
}

impl Out {
    fn shot(&self, name: &str, a: &App, crop: Crop) {
        let (svg, text) = draw(a, crop);
        std::fs::write(self.dir.join(format!("{name}.svg")), svg).unwrap();
        if let Some(t) = &self.txt {
            std::fs::write(t.join(format!("{name}.txt")), text).unwrap();
        }
        let mut names = self.names.borrow_mut();
        assert!(!names.iter().any(|n| n == name), "{name} twice");
        names.push(name.to_owned());
    }
}

/// Node ids of the scene's tree.
#[derive(Clone, Copy)]
struct Ids {
    uid: u64,
    db: u64,
    sales: u64,
    tables: u64,
    customer: u64,
    views: u64,
}

fn node_row(oid: u32, name: &str) -> pgc_db::NodeRow {
    pgc_db::NodeRow {
        oid,
        name: name.into(),
        label: None,
        partitioned: false,
    }
}

/// The group of type `t` right below node `parent`.
fn group_of(a: &App, parent: u64, t: ObjType) -> u64 {
    a.tree
        .node(parent)
        .and_then(|n| n.children.as_ref())
        .and_then(|c| c.iter().find(|n| n.kind == NodeKind::Group(t)))
        .map(|n| n.nid)
        .unwrap_or_else(|| panic!("no group {t:?}"))
}

/// Fills group `group` with objects named `names` (OIDs from `oid`); their node ids.
fn fill(a: &mut App, group: u64, oid: u32, names: &[&str]) -> Vec<u64> {
    let rows = names
        .iter()
        .enumerate()
        .map(|(i, n)| node_row(oid + i as u32, n))
        .collect();
    a.tree.set_children(group, rows);
    a.tree
        .node(group)
        .unwrap()
        .children
        .as_ref()
        .unwrap()
        .iter()
        .map(|n| n.nid)
        .collect()
}

fn expand(a: &mut App, nid: u64) {
    a.tree.node_mut(nid).unwrap().expanded = true;
}

fn collapse(a: &mut App, nid: u64) {
    a.tree.node_mut(nid).unwrap().expanded = false;
}

/// Puts the tree cursor on `kind`, scrolled into view; the default tab follows.
fn select(a: &mut App, kind: RowKind) {
    a.tree.select_kind(kind);
    a.tree.ensure_visible(H as usize - 6);
    a.follow_selection();
}

/// The definition shown in the default tab for the selected object.
fn definition(a: &mut App, text: &str) {
    let view = &mut a.tabs[0].view;
    let title = view.title.clone();
    view.set_text(title, text.to_owned());
    view.loading = false;
}

const SALES_TABLES: &[&str] = &[
    "countryregioncurrency",
    "creditcard",
    "currency",
    "currencyrate",
    "customer",
    "personcreditcard",
    "salesorderdetail",
    "salesorderheader",
    "salesorderheadersalesreason",
    "salesperson",
    "salespersonquotahistory",
    "salesreason",
    "salestaxrate",
    "salesterritory",
    "salesterritoryhistory",
    "shoppingcartitem",
    "specialoffer",
    "specialofferproduct",
    "store",
];

const CUSTOMER_DDL: &str = "-- Table: sales.customer

-- DROP TABLE IF EXISTS sales.customer;

CREATE TABLE IF NOT EXISTS sales.customer
(
    customerid integer NOT NULL DEFAULT nextval('sales.customer_customerid_seq'::regclass),
    personid integer,
    storeid integer,
    territoryid integer,
    rowguid uuid NOT NULL DEFAULT uuid_generate_v1(),
    modifieddate timestamp without time zone NOT NULL DEFAULT now(),
    CONSTRAINT \"PK_Customer_CustomerID\" PRIMARY KEY (customerid),
    CONSTRAINT \"FK_Customer_Person_PersonID\" FOREIGN KEY (personid) REFERENCES person.person(businessentityid),
    CONSTRAINT \"FK_Customer_SalesTerritory_TerritoryID\" FOREIGN KEY (territoryid) REFERENCES sales.salesterritory(territoryid),
    CONSTRAINT \"FK_Customer_Store_StoreID\" FOREIGN KEY (storeid) REFERENCES sales.store(businessentityid)
)
TABLESPACE pg_default;

ALTER TABLE IF EXISTS sales.customer
    OWNER to postgres;

COMMENT ON TABLE sales.customer
    IS 'Current customer information. Also see the Person and Store tables.';

COMMENT ON COLUMN sales.customer.customerid
    IS 'Primary key.';

COMMENT ON COLUMN sales.customer.personid
    IS 'Foreign key to Person.BusinessEntityID';

COMMENT ON COLUMN sales.customer.storeid
    IS 'Foreign key to Store.BusinessEntityID';

COMMENT ON COLUMN sales.customer.territoryid
    IS 'ID of the territory in which the customer is located. Foreign key to SalesTerritory.SalesTerritoryID.';

GRANT SELECT ON TABLE sales.customer TO reporting;

GRANT ALL ON TABLE sales.customer TO postgres;

-- Index: ix_customer_territoryid

-- DROP INDEX IF EXISTS sales.ix_customer_territoryid;

CREATE INDEX IF NOT EXISTS ix_customer_territoryid
    ON sales.customer USING btree (territoryid);
";

const VIEW_DDL: &str = "-- View: sales.vindividualcustomer

-- DROP VIEW sales.vindividualcustomer;

CREATE OR REPLACE VIEW sales.vindividualcustomer
 AS
 SELECT p.businessentityid,
    p.title,
    p.firstname,
    p.middlename,
    p.lastname,
    p.suffix,
    pp.phonenumber,
    pnt.name AS phonenumbertype,
    ea.emailaddress,
    p.emailpromotion,
    at.name AS addresstype,
    a.addressline1,
    a.addressline2,
    a.city,
    sp.name AS stateprovincename,
    a.postalcode,
    cr.name AS countryregionname,
    p.demographics
   FROM person.person p
     JOIN person.businessentityaddress bea ON bea.businessentityid = p.businessentityid
     JOIN person.address a ON a.addressid = bea.addressid
     JOIN person.stateprovince sp ON sp.stateprovinceid = a.stateprovinceid
     JOIN person.countryregion cr ON cr.countryregioncode::text = sp.countryregioncode::text
     JOIN person.addresstype at ON at.addresstypeid = bea.addresstypeid
     JOIN sales.customer c ON c.personid = p.businessentityid
     LEFT JOIN person.emailaddress ea ON ea.businessentityid = p.businessentityid
     LEFT JOIN person.personphone pp ON pp.businessentityid = p.businessentityid
     LEFT JOIN person.phonenumbertype pnt ON pnt.phonenumbertypeid = pp.phonenumbertypeid
  WHERE c.storeid IS NULL;

ALTER TABLE sales.vindividualcustomer
    OWNER TO postgres;
";

/// Configuration settings of the sample server (D-037).
fn sample_settings() -> Vec<Setting> {
    let set = |name: &str, value: &str, source: &str, file: Option<(&str, &str)>, restart: bool| {
        Setting {
            name: name.into(),
            value: value.into(),
            source: source.into(),
            file: file.map(|(f, l)| (f.to_owned(), l.to_owned())),
            pending_restart: restart,
        }
    };
    let conf = "/etc/postgresql/17/main/postgresql.conf";
    vec![
        set("allow_alter_system", "on", "default", None, false),
        set("application_name", "pgtc", "client", None, false),
        set("archive_mode", "off", "default", None, false),
        set("autovacuum", "on", "default", None, false),
        set("autovacuum_max_workers", "3", "default", None, false),
        set("autovacuum_naptime", "1min", "default", None, false),
        set(
            "checkpoint_completion_target",
            "0.9",
            "default",
            None,
            false,
        ),
        set("checkpoint_timeout", "5min", "default", None, false),
        set("client_encoding", "UTF8", "client", None, false),
        set("data_checksums", "off", "default", None, false),
        set(
            "data_directory",
            "/var/lib/postgresql/17/main",
            "override",
            None,
            false,
        ),
        set(
            "DateStyle",
            "ISO, DMY",
            "configuration file",
            Some((conf, "714")),
            false,
        ),
        set("default_statistics_target", "100", "default", None, false),
        set(
            "effective_cache_size",
            "6GB",
            "configuration file",
            Some((conf, "417")),
            false,
        ),
        set("fsync", "on", "default", None, false),
        set(
            "listen_addresses",
            "*",
            "configuration file",
            Some((conf, "60")),
            false,
        ),
        set("log_destination", "stderr", "default", None, false),
        set(
            "logging_collector",
            "on",
            "configuration file",
            Some((conf, "469")),
            false,
        ),
        set(
            "max_connections",
            "200",
            "configuration file",
            Some((conf, "65")),
            false,
        ),
        set(
            "shared_buffers",
            "2GB",
            "configuration file",
            Some((conf, "130")),
            true,
        ),
        set(
            "work_mem",
            "16MB",
            "configuration file",
            Some((conf, "143")),
            false,
        ),
    ]
}

/// The scene: a connected server `local` with the AdventureWorks database open down
/// to the tables of schema `sales`, and a second, disconnected server.
fn scene_with(scheme: &str) -> (App, tempfile::TempDir, Ids) {
    let (mut a, d) = app();
    a.settings.ui.color_scheme = scheme.into();
    a.theme = a.themes.get(scheme).clone();
    a.frame.screen = Rect::new(0, 0, W, H);
    a.frame.tree_height = H as usize - 6;
    a.files.dir = std::path::PathBuf::from(if cfg!(windows) {
        "D:\\work\\adventureworks"
    } else {
        "/home/dev/adventureworks"
    });
    let uid = a.tree.add(SavedConnection {
        name: "local".into(),
        username: "postgres".into(),
        ..SavedConnection::default()
    });
    a.tree.add(SavedConnection {
        name: "production".into(),
        host: "db.example.com".into(),
        username: "app".into(),
        database: "adventureworks".into(),
        ssl_mode: pgc_core::settings::SslMode::VerifyFull,
        ..SavedConnection::default()
    });
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(170002));
    a.tree.servers[0].handle = Some(pgc_db::ConnectionHandle::detached(1));
    a.tree.servers[0].settings = ServerSettings::Loaded(sample_settings());
    let dbs = a.tree.servers[0].children[0].nid;
    let db = fill(&mut a, dbs, 16384, &[DB, "postgres"])[0];
    let roles = a.tree.servers[0].children[1].nid;
    fill(
        &mut a,
        roles,
        10,
        &["app", "postgres", "reporting", "webshop"],
    );
    collapse(&mut a, roles);
    let groups = a.tree.groups(&ObjType::Database.children(false));
    let n = a.tree.node_mut(db).unwrap();
    n.conn = Some(DbConn {
        state: ConnState::Connected,
        handle: Some(pgc_db::ConnectionHandle::detached(51)),
    });
    n.children = Some(groups);
    n.expanded = true;
    let schemas = group_of(&a, db, ObjType::Schema);
    let s = fill(
        &mut a,
        schemas,
        16400,
        &[
            "humanresources",
            "person",
            "production",
            "public",
            "purchasing",
            "sales",
        ],
    );
    expand(&mut a, schemas);
    let sales = s[5];
    expand(&mut a, sales);
    let tables = group_of(&a, sales, ObjType::Table);
    let t = fill(&mut a, tables, 17000, SALES_TABLES);
    expand(&mut a, tables);
    let views = group_of(&a, sales, ObjType::View);
    fill(
        &mut a,
        views,
        17500,
        &[
            "vindividualcustomer",
            "vpersondemographics",
            "vsalesperson",
            "vsalespersonsalesbyfiscalyears",
            "vstorewithaddresses",
            "vstorewithcontacts",
            "vstorewithdemographics",
        ],
    );
    let ids = Ids {
        uid,
        db,
        sales,
        tables,
        customer: t[4],
        views,
    };
    select(&mut a, RowKind::Node(ids.customer));
    definition(&mut a, CUSTOMER_DDL);
    a.status.clear();
    (a, d, ids)
}

fn scene() -> (App, tempfile::TempDir, Ids) {
    scene_with("Classic")
}

/// A shorter tree: the tables group closed, the cursor on the database.
fn short_tree(a: &mut App, ids: Ids) {
    collapse(a, ids.tables);
    collapse(a, ids.sales);
    // a narrower tree (dragged, D-045): more room for the tab
    a.settings.ui.tree_width = Some(32);
    select(a, RowKind::Node(ids.db));
    a.tabs[0].view.loading = false;
}

/// A query tab connected to AdventureWorks (no real connection behind it) with `text`.
fn query_tab(a: &mut App, ids: Ids, text: &str) {
    a.new_query_tab(None);
    let q = a.query_mut().unwrap();
    q.binding = Some((ids.uid, DB.into()));
    q.conn = Some(crate::query_tab::QueryConn {
        id: 99,
        database: DB.into(),
        handle: pgc_db::ConnectionHandle::detached(99),
        cancel: None,
        state: crate::query_tab::QueryConnState::Connected,
        ssl_mode: pgc_core::settings::SslMode::default(),
        ssl_root_cert: None,
    });
    q.info(format!("Connected to {DB} on local (PostgreSQL 17.2)"));
    let active = a.active_tab;
    a.tabs[active].server = Some(ids.uid);
    a.tabs[active].database = Some(DB.into());
    a.editor().set_text(text);
    a.focus = Focus::Tabs;
    a.status.clear();
}

fn start(a: &mut App) {
    let text = a.editor().text();
    a.query_mut().unwrap().start_run(1, Sent::new(0, &text));
}

/// Applies a query event with line numbers of the editor text.
fn apply(a: &mut App, e: pgc_db::QueryEvent) {
    let text = a.editor().text();
    let line_of = |c: usize| text.chars().take(c).filter(|ch| *ch == '\n').count();
    let lines = LineMap { line_of: &line_of };
    a.query_mut().unwrap().apply(e, &lines);
}

fn notice(a: &mut App, severity: &str, message: &str) {
    let text = a.editor().text();
    let line_of = |c: usize| text.chars().take(c).filter(|ch| *ch == '\n').count();
    let lines = LineMap { line_of: &line_of };
    a.query_mut().unwrap().notice(
        &pgc_db::ServerMessage {
            severity: severity.into(),
            message: message.into(),
            ..Default::default()
        },
        &lines,
    );
}

/// A result set: columns as (name, type OID), rows of texts (`\N` = NULL).
fn result(a: &mut App, title: Option<&str>, cols: &[(&str, u32)], rows: &[&[&str]], more: bool) {
    apply(
        a,
        pgc_db::QueryEvent::ResultStart {
            columns: cols
                .iter()
                .map(|(n, t)| pgc_db::Column {
                    name: (*n).into(),
                    type_oid: *t,
                })
                .collect(),
            title: title.map(str::to_owned),
        },
    );
    apply(
        a,
        pgc_db::QueryEvent::Rows {
            rows: rows
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|v| (*v != "\\N").then(|| (*v).into()))
                        .collect()
                })
                .collect(),
            more,
        },
    );
}

fn tag(a: &mut App, tag: &str) {
    apply(a, pgc_db::QueryEvent::Command { tag: tag.into() });
}

fn finished(a: &mut App, ms: u64, status: TxStatus) {
    apply(
        a,
        pgc_db::QueryEvent::Finished {
            elapsed: std::time::Duration::from_millis(ms),
            status,
        },
    );
}

const TOP_QUERY: &str = "-- best customers of 2014
SELECT p.firstname, p.lastname, count(*) AS orders, sum(h.totaldue) AS total
FROM sales.salesorderheader h
JOIN sales.customer c ON c.customerid = h.customerid
JOIN person.person p ON p.businessentityid = c.personid
WHERE h.orderdate >= '2014-01-01'
GROUP BY p.firstname, p.lastname
ORDER BY total DESC
LIMIT 20;";

const TOP_COLS: &[(&str, u32)] = &[
    ("firstname", 1043),
    ("lastname", 1043),
    ("orders", 20),
    ("total", 1700),
];

const TOP_ROWS: &[&[&str]] = &[
    &["Roger", "Harui", "12", "877107.1923"],
    &["Andrew", "Dixon", "12", "853849.1795"],
    &["Reuben", "D'sa", "12", "841908.7708"],
    &["Robert", "Vessa", "8", "816755.5763"],
    &["Ryan", "Calafato", "12", "799277.8953"],
    &["Joseph", "Castellucio", "12", "787773.0436"],
    &["Kirk", "DeGrasse", "12", "746317.5293"],
    &["Lindsey", "Camacho", "8", "740985.8338"],
    &["Robin", "McGuigan", "12", "730798.7139"],
    &["Stacey", "Cereghino", "8", "727272.6494"],
    &["Marcia", "Sultan", "12", "693502.9881"],
    &["Richard", "Byham", "12", "689141.2241"],
];

/// A query tab with the "best customers" query run and its result shown.
fn query_scene() -> (App, tempfile::TempDir, Ids) {
    let (mut a, d, ids) = scene();
    short_tree(&mut a, ids);
    query_tab(&mut a, ids, TOP_QUERY);
    start(&mut a);
    result(&mut a, None, TOP_COLS, TOP_ROWS, false);
    tag(&mut a, "SELECT 12");
    finished(&mut a, 184, TxStatus::Idle);
    (a, d, ids)
}

fn customer_rows() -> Vec<Vec<&'static str>> {
    vec![
        vec![
            "1",
            "\\N",
            "934",
            "1",
            "3f5ae95e-b87d-4aed-95b4-c3797afcb74f",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "2",
            "\\N",
            "1028",
            "1",
            "e552f657-a9af-4a7d-a645-c429d6e02491",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "3",
            "\\N",
            "642",
            "4",
            "130774b1-db21-4ef3-98c8-c104bcd6ed6d",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "4",
            "\\N",
            "932",
            "4",
            "ff862851-1daa-4044-be7c-3e85583c054d",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "5",
            "\\N",
            "1026",
            "4",
            "83905bdc-6f5e-4f71-b162-c98da069f38a",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "6",
            "\\N",
            "644",
            "4",
            "1a92df88-bfa2-467d-bd54-fcb9e647fdd7",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "11000",
            "13531",
            "\\N",
            "9",
            "477586b3-2977-4e54-b1a8-569ab2c7c4d4",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "11001",
            "5454",
            "\\N",
            "9",
            "d7b1b6e3-5c0f-4c8a-8d4c-2b1f0c0c4f43",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "11002",
            "11269",
            "\\N",
            "9",
            "a1f9b3de-7d3c-4e0b-9a55-0c7a1d7c2b10",
            "2014-09-12 11:15:07.263",
        ],
        vec![
            "11003",
            "11358",
            "\\N",
            "9",
            "f6a4c5be-91d4-4d9e-8b1a-6a0d4e1f7c52",
            "2014-09-12 11:15:07.263",
        ],
    ]
}

const CUSTOMER_COLS: &[(&str, u32)] = &[
    ("customerid", 23),
    ("personid", 23),
    ("storeid", 23),
    ("territoryid", 23),
    ("rowguid", 2950),
    ("modifieddate", 1114),
];

fn strs<'a, 'b>(rows: &'a [Vec<&'b str>]) -> Vec<&'a [&'b str]> {
    rows.iter().map(Vec::as_slice).collect()
}

/// A fake folder listing for the open file dialog.
fn listing(a: &mut App, entries: &[(&str, bool)], ext: &str) {
    let sep = std::path::MAIN_SEPARATOR;
    let mut rows = vec![(format!("..{sep}"), true, false)];
    for (name, dir) in entries {
        if *dir {
            rows.push((format!("{name}{sep}"), true, false));
        } else {
            rows.push(((*name).to_owned(), false, name.ends_with(ext)));
        }
    }
    a.files.seq += 1;
    a.files.listing = Some(a.files.seq);
    let seq = a.files.seq;
    a.on_file_event(crate::app::file_jobs::FileEvent::Listed {
        seq,
        listing: crate::app::file_jobs::Listing::Folder(rows),
    });
}

const WORK_FILES: &[(&str, bool)] = &[
    ("migrations", true),
    ("reports", true),
    ("best_customers.sql", false),
    ("inventory_check.sql", false),
    ("monthly_sales.sql", false),
    ("notes.txt", false),
    ("territory_quota.sql", false),
];

/// The example template of `docs/help/templates.html` (keep the two the same).
const OWN_TEMPLATE: &str = r#"+++
name        = "Create lookup table"
description = "A code / name table with our standard columns"

[[param]]
name     = "schema"
label    = "Schema"
type     = "choice"
source   = "schemas"
default  = "public"
required = true

[[param]]
name     = "name"
label    = "Table name"
type     = "identifier"
required = true
help     = "Singular, lower case: country, orderstatus"

[[param]]
name    = "code_length"
label   = "Length of the code"
type    = "number"
default = "10"

[[param]]
name  = "with_audit"
label = "Audit columns (modifiedby, modifieddate)"
type  = "bool"
+++
CREATE TABLE {{ schema|ident }}.{{ name|ident }} (
    code varchar({{ code_length }}) NOT NULL,
    name text NOT NULL,
{% if with_audit %}
    modifiedby text NOT NULL DEFAULT CURRENT_USER,
    modifieddate timestamp NOT NULL DEFAULT now(),
{% endif %}
    CONSTRAINT {{ (name ~ "_pkey")|ident }} PRIMARY KEY (code)
);

COMMENT ON TABLE {{ schema|ident }}.{{ name|ident }} IS 'Lookup table';
"#;

/// Moves the template picker to the template whose description starts with `desc`.
fn pick_template(a: &mut App, desc: &str) {
    a.run(Command::InstallTemplates);
    a.dialogs.clear();
    a.status.clear();
    a.run(Command::NewFromTemplate);
    for _ in 0..80 {
        let d = a
            .dialogs
            .last()
            .and_then(|d| d.widgets.iter().find(|w| w.id == "desc"))
            .map(|w| w.label.text.clone())
            .unwrap_or_default();
        if d.starts_with(desc) {
            return;
        }
        key(a, "Down");
    }
    panic!("no template {desc}");
}

fn text_choices(names: &[&str]) -> Vec<crate::template_dialog::Choice> {
    names
        .iter()
        .map(|s| crate::template_dialog::Choice {
            label: (*s).into(),
            value: pgc_sql::devtemplate::ParamValue::Text((*s).into()),
        })
        .collect()
}

const SCHEMAS: &[&str] = &[
    "humanresources",
    "person",
    "production",
    "public",
    "purchasing",
    "sales",
];

fn field_index(a: &App, name: &str) -> usize {
    a.templates
        .form
        .as_ref()
        .unwrap()
        .fields
        .iter()
        .position(|f| f.param.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
}

fn set_text_field(a: &mut App, name: &str, text: &str) {
    let i = field_index(a, name);
    a.templates.form.as_mut().unwrap().fields[i].value = crate::template_dialog::FieldValue::Text {
        text: text.into(),
        cursor: text.chars().count(),
    };
}

/// Opens the dashboard of the selected row with a detached connection.
fn dashboard(a: &mut App) {
    a.settings.ui.tree_width = Some(26);
    a.run(Command::Dashboard);
    let d = a.tabs[a.active_tab].dashboard_mut().unwrap();
    let c = d.conn.as_mut().unwrap();
    c.handle.abort();
    c.handle = pgc_db::ConnectionHandle::detached(c.id);
    c.connected = true;
    d.version = Some(pgc_core::ServerVersion(170002));
    let (mut commits, mut tuples, mut blocks) = (9_000_000u64, 40_000_000u64, 7_000_000u64);
    for i in 0..60u64 {
        let wave = (i as f64 / 6.0).sin().abs();
        commits += 300 + (wave * 500.0) as u64 + i % 7 * 20;
        tuples += 2_000 + (wave * 6_000.0) as u64;
        blocks += 4_000 + ((1.0 - wave) * 3_000.0) as u64;
        let sessions = 38 + (wave * 9.0) as u64;
        let active = 3 + (wave * 6.0) as u64;
        let mut row: Vec<String> = vec![
            sessions.to_string(),
            active.to_string(),
            (sessions - active - 2).to_string(),
            commits.to_string(),
            (commits / 180).to_string(),
            (tuples / 3).to_string(),
            (tuples / 5).to_string(),
            (tuples / 40).to_string(),
            (tuples * 2).to_string(),
            (tuples * 9).to_string(),
            (blocks / 90).to_string(),
            blocks.to_string(),
            (1_000_000.0 + i as f64 * 2.0).to_string(),
        ];
        let deadlocks = if i >= 58 { "4" } else { "3" };
        row.extend(
            [
                "2", "0", "1", "200", "1", "3", "2", "8", "0", "4", "2", "10", "0", "2", "10",
                "1209600", "1", "8388608", "false", "754", "48.6", "1843", "41377", deadlocks, "0",
                "0", "16384", "50331648",
            ]
            .map(String::from),
        );
        d.apply_stats(crate::dashboard::Sample::from_row(&row).unwrap());
    }
    d.apply_size(1_310_000_000, 1_000_000.0);
    d.apply_size(1_310_368_640, 1_000_090.0);
    let s = |pid: &str,
             app: &str,
             state: &str,
             wait: &str,
             by: &str,
             secs: &str,
             xmin: &str,
             kind: &str,
             query: &str| {
        [
            pid,
            DB,
            if kind == "client backend" {
                "webshop"
            } else {
                ""
            },
            app,
            if kind == "client backend" {
                "10.0.4.17"
            } else {
                ""
            },
            state,
            wait,
            by,
            secs,
            secs,
            "3600",
            xmin,
            "",
            "",
            "",
            "2026-11-16 08:00:02",
            "",
            "",
            kind,
            query,
        ]
        .map(String::from)
        .to_vec()
    };
    d.apply_rows(
        crate::dashboard::List::Sessions,
        vec![
            s(
                "41377",
                "order-import",
                "idle in transaction",
                "Client: ClientRead",
                "",
                "754",
                "1843",
                "client backend",
                "UPDATE sales.salesorderheader SET status = 5 WHERE salesorderid = $1",
            ),
            s(
                "41502",
                "webshop",
                "active",
                "Lock: transactionid",
                "41377",
                "48.6",
                "12",
                "client backend",
                "UPDATE sales.salesorderheader SET shipdate = now() WHERE salesorderid = $1",
            ),
            s(
                "41620",
                "reporting",
                "active",
                "IO: DataFileRead",
                "",
                "3.2",
                "8",
                "client backend",
                "SELECT t.name, sum(h.totaldue) FROM sales.salesorderheader h JOIN sales.salesterritory t ON t.territoryid = h.territoryid GROUP BY 1",
            ),
            s(
                "41733",
                "webshop",
                "active",
                "",
                "",
                "0.004",
                "",
                "client backend",
                "SELECT * FROM production.product WHERE productid = $1",
            ),
            s(
                "40112",
                "",
                "active",
                "",
                "",
                "21",
                "3",
                "autovacuum worker",
                "autovacuum: VACUUM sales.salesorderdetail",
            ),
            s(
                "41801",
                "webshop",
                "idle",
                "Client: ClientRead",
                "",
                "",
                "",
                "client backend",
                "COMMIT",
            ),
            s(
                "41802",
                "webshop",
                "idle",
                "Client: ClientRead",
                "",
                "",
                "",
                "client backend",
                "SELECT 1",
            ),
        ],
    );
    a.focus = Focus::Results;
    a.status.clear();
    // the statement pane follows the cursor in `update`
    key(a, "Down");
    key(a, "Up");
}

fn to_rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| r.iter().map(|s| (*s).to_owned()).collect())
        .collect()
}

/// Opens the profiler of the selected server with a detached connection.
fn profiler(a: &mut App) -> (u64, pgc_db::ConnId) {
    a.settings.ui.tree_width = Some(26);
    a.run(Command::SqlProfiler);
    let tab = a.tabs[a.active_tab].id;
    let p = a.tabs[a.active_tab].profiler_mut().unwrap();
    let c = p.conn.as_mut().unwrap();
    c.handle.abort();
    c.handle = pgc_db::ConnectionHandle::detached(c.id);
    c.connected = true;
    p.version = Some(pgc_core::ServerVersion(170002));
    let id = c.id;
    a.status.clear();
    (tab, id)
}

fn profiler_records(a: &mut App) {
    let p = a.tabs[a.active_tab].profiler_mut().unwrap();
    p.phase = crate::profiler::Phase::Running;
    let r = |time: &str, pid: &str, app: &str, ms: &str, kind: &str, sql: &str, par: &str| {
        crate::profiler::Record {
            time: format!("2026-11-16 10:42:{time}"),
            user: if app == "reporting" {
                "reporting".into()
            } else {
                "webshop".into()
            },
            database: DB.into(),
            pid: pid.into(),
            client: "10.0.4.17".into(),
            severity: "LOG".into(),
            sqlstate: "00000".into(),
            message: format!("duration: {ms} ms  {kind}: {sql}"),
            detail: if par.is_empty() {
                String::new()
            } else {
                format!("Parameters: {par}")
            },
            statement: String::new(),
            application: app.into(),
        }
    };
    let mut records = vec![
        r(
            "03.114",
            "41733",
            "webshop",
            "0.412",
            "execute <unnamed>",
            "SELECT * FROM production.product WHERE productid = $1",
            "$1 = '771'",
        ),
        r(
            "03.180",
            "41733",
            "webshop",
            "1.873",
            "statement",
            "SELECT count(*) FROM sales.shoppingcartitem WHERE shoppingcartid = '20621'",
            "",
        ),
        r(
            "03.902",
            "41620",
            "reporting",
            "3204.551",
            "statement",
            "SELECT t.name, sum(h.totaldue) FROM sales.salesorderheader h JOIN sales.salesterritory t ON t.territoryid = h.territoryid GROUP BY 1",
            "",
        ),
        r(
            "04.015",
            "41502",
            "webshop",
            "0.388",
            "execute <unnamed>",
            "SELECT * FROM production.product WHERE productid = $1",
            "$1 = '712'",
        ),
        r(
            "04.230",
            "41502",
            "webshop",
            "12.906",
            "execute <unnamed>",
            "INSERT INTO sales.salesorderdetail (salesorderid, orderqty, productid, specialofferid, unitprice) VALUES ($1, $2, $3, $4, $5)",
            "$1 = '75123', $2 = '2', $3 = '712', $4 = '1', $5 = '8.99'",
        ),
        r(
            "04.231",
            "41502",
            "webshop",
            "0.051",
            "statement",
            "COMMIT",
            "",
        ),
    ];
    records.push(crate::profiler::Record {
        severity: "ERROR".into(),
        message: "duplicate key value violates unique constraint \"PK_Currency_CurrencyCode\""
            .into(),
        statement: "INSERT INTO sales.currency (currencycode, name) VALUES ('EUR', 'Euro')".into(),
        ..r("04.502", "41801", "admin-tool", "0", "statement", "", "")
    });
    records.push(r(
        "05.077",
        "41733",
        "webshop",
        "0.297",
        "execute <unnamed>",
        "SELECT * FROM production.product WHERE productid = $1",
        "$1 = '870'",
    ));
    records.push(r(
        "05.640",
        "41620",
        "reporting",
        "845.020",
        "statement",
        "SELECT date_trunc('month', orderdate), count(*) FROM sales.salesorderheader GROUP BY 1 ORDER BY 1",
        "",
    ));
    p.add_records(records);
    p.grid.row = 4;
    a.focus = Focus::Results;
    key(a, "Down");
    key(a, "Up");
}

fn popup_for(a: &mut App, items_from_names: bool) {
    let analysis = pgc_editor::complete::analyze(&a.tabs[a.active_tab].view.editor).unwrap();
    let built = crate::completion::build(
        &analysis,
        &a.query().unwrap().names,
        pgc_editor::lexer::keywords(),
    );
    let mut popup = crate::completion::Popup {
        in_form: false,
        tab: a.tabs[a.active_tab].id,
        analysis,
        items: Vec::new(),
        loading: false,
        selected: 0,
        offset: 0,
        auto: !items_from_names,
    };
    popup.set_items(built.items, false);
    a.completion = Some(popup);
}

#[tokio::test]
#[ignore]
async fn help_screenshots() {
    let Ok(dir) = std::env::var("PGTC_HELP_DIR") else {
        return;
    };
    let out = Out {
        dir: dir.into(),
        txt: std::env::var("PGTC_HELP_TXT").ok().map(Into::into),
        names: Default::default(),
    };
    std::fs::create_dir_all(&out.dir).unwrap();
    if let Some(t) = &out.txt {
        std::fs::create_dir_all(t).unwrap();
    }

    // ---- the window -------------------------------------------------------------
    let (mut a, _d, _) = scene();
    a.focus = Focus::Tree;
    out.shot("window", &a, Crop::Full);

    // the menus
    for (i, name) in ["file", "view", "query", "tools", "settings", "help"]
        .iter()
        .enumerate()
    {
        let (mut a, _d, _) = query_scene();
        key(&mut a, "F9");
        for _ in 0..i {
            key(&mut a, "Right");
        }
        out.shot(&format!("menu-{name}"), &a, Crop::TopLeft(96, 20));
    }

    // F1 in the query editor; Help → Keyboard shortcuts; About
    let (mut a, _d, _) = query_scene();
    key(&mut a, "F1");
    out.shot("f1", &a, Crop::Full);
    let (mut a, _d, _) = query_scene();
    a.run(Command::KeyboardShortcuts);
    out.shot("shortcuts", &a, Crop::Full);
    let (mut a, _d, _) = query_scene();
    a.run(Command::About);
    out.shot("about", &a, Crop::Dialog);
    let (mut a, _d, _) = query_scene();
    a.run(Command::ThirdPartyLicences);
    out.shot("licences", &a, Crop::Full);

    // ---- connections ------------------------------------------------------------
    let (mut a, _d, _) = scene();
    a.focus = Focus::Tree;
    a.run(Command::NewConnection);
    out.shot("conn-empty", &a, Crop::Dialog);
    {
        let d = a.dialogs.last_mut().unwrap();
        let ids: Vec<&'static str> = d.widgets.iter().map(|w| w.id).collect();
        // the fields in their order: name, host, port, database, user, ...
        let values = [
            "reporting replica",
            "replica.example.com",
            "5432",
            "adventureworks",
            "reporting",
        ];
        let mut v = values.iter();
        for id in ids {
            if d.text(id).is_some()
                && let Some(value) = v.next()
            {
                d.set_text(id, value);
            }
        }
    }
    out.shot("conn-new", &a, Crop::Dialog);

    // the login dialog
    let (mut a, _d, _) = scene();
    let uid2 = a.tree.servers[1].uid;
    a.tree.select_server(uid2);
    a.follow_selection();
    let id = fake_connecting(&mut a, uid2);
    a.update(
        Msg::Db(DbEvent::ConnectFailed {
            id,
            error: DbError::PasswordRequired,
        }),
        &Hits::default(),
    );
    typing(&mut a, "secret42");
    out.shot("conn-login", &a, Crop::Dialog);

    // a disconnected server's info text and its context menu
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    let uid2 = a.tree.servers[1].uid;
    a.tree.select_server(uid2);
    a.follow_selection();
    a.focus = Focus::Tree;
    out.shot("conn-disconnected", &a, Crop::Full);
    key(&mut a, "F2");
    out.shot("conn-menu", &a, Crop::Full);

    // the connected server: its context menu, delete
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.tree.select_server(ids2.uid);
    a.follow_selection();
    a.focus = Focus::Tree;
    key(&mut a, "F2");
    out.shot("server-menu", &a, Crop::TopLeft(84, 20));
    a.popup = None;
    key(&mut a, "F8");
    out.shot("conn-delete", &a, Crop::Dialog);

    // the server row: version and configuration
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.tree.servers[0].settings = ServerSettings::Loaded(sample_settings());
    a.followed = None;
    a.tree.select_server(ids2.uid);
    a.follow_selection();
    a.focus = Focus::Tree;
    out.shot("server-info", &a, Crop::Full);

    // ---- the object tree --------------------------------------------------------
    // a group row lists its objects
    let (mut a, _d, ids2) = scene();
    select(&mut a, RowKind::Node(ids2.tables));
    a.focus = Focus::Tree;
    out.shot("tree-group", &a, Crop::Full);

    // context menus: a table, the database
    let (mut a, _d, _) = scene();
    a.focus = Focus::Tree;
    key(&mut a, "F2");
    out.shot("tree-menu-table", &a, Crop::Full);
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.focus = Focus::Tree;
    key(&mut a, "F2");
    out.shot("tree-menu-db", &a, Crop::TopLeft(84, 24));

    // quick search
    let (mut a, _d, _) = scene();
    a.focus = Focus::Tree;
    key(&mut a, "Alt+S");
    typing(&mut a, "salesp");
    a.tabs[0].view.loading = false;
    let title = a.tabs[0].view.title.clone();
    a.tabs[0].view.set_text(
        title,
        "-- Table: sales.salesperson\n\n-- DROP TABLE IF EXISTS sales.salesperson;\n\nCREATE TABLE IF NOT EXISTS sales.salesperson\n(\n    businessentityid integer NOT NULL,\n    territoryid integer,\n    salesquota numeric,\n    bonus numeric NOT NULL DEFAULT 0.00,\n    commissionpct numeric NOT NULL DEFAULT 0.00,\n    salesytd numeric NOT NULL DEFAULT 0.00,\n    saleslastyear numeric NOT NULL DEFAULT 0.00,\n    rowguid uuid NOT NULL DEFAULT uuid_generate_v1(),\n    modifieddate timestamp without time zone NOT NULL DEFAULT now(),\n    CONSTRAINT \"PK_SalesPerson_BusinessEntityID\" PRIMARY KEY (businessentityid)\n)\nTABLESPACE pg_default;\n".into(),
    );
    out.shot("tree-search", &a, Crop::Full);

    // Find object below the database
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.focus = Focus::Tree;
    key(&mut a, "Ctrl+F");
    out.shot("find-empty", &a, Crop::Dialog);
    typing(&mut a, "territory");
    key(&mut a, "Alt+C");
    let seq = a.find_seq;
    a.update(Msg::FindTimer(seq), &Hits::default());
    let sent = *a.find.as_ref().unwrap().pending.keys().next().unwrap();
    let r = |v: [&str; 5]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
    a.find_loaded(
        sent,
        Ok(vec![
            r([
                "table",
                "sales",
                "salesterritory",
                "schemas:16405/tables:17013",
                "2",
            ]),
            r([
                "table",
                "sales",
                "salesterritoryhistory",
                "schemas:16405/tables:17014",
                "2",
            ]),
            r([
                "view",
                "humanresources",
                "vemployeedepartment",
                "schemas:16400/views:17601",
                "6",
            ]),
            r([
                "sequence",
                "sales",
                "salesterritory_territoryid_seq",
                "schemas:16405/sequences:17102",
                "12",
            ]),
            r([
                "index",
                "sales",
                "ix_customer_territoryid",
                "schemas:16405/tables:17004/indexes:17103",
                "20",
            ]),
            r([
                "constraint",
                "sales",
                "FK_Customer_SalesTerritory_TerritoryID",
                "schemas:16405/tables:17004/constraints:17104",
                "21",
            ]),
            r([
                "constraint",
                "sales",
                "PK_SalesTerritory_TerritoryID",
                "schemas:16405/tables:17013/constraints:17105",
                "21",
            ]),
            r([
                "column",
                "person",
                "stateprovince.territoryid",
                "schemas:16401/tables:16800/columns:7",
                "90",
            ]),
            r([
                "column",
                "sales",
                "customer.territoryid",
                "schemas:16405/tables:17004/columns:4",
                "90",
            ]),
            r([
                "column",
                "sales",
                "salesorderheader.territoryid",
                "schemas:16405/tables:17007/columns:12",
                "90",
            ]),
            r([
                "column",
                "sales",
                "salesperson.territoryid",
                "schemas:16405/tables:17009/columns:2",
                "90",
            ]),
        ]),
    );
    a.dialogs[0].list_select("list", 1);
    out.shot("find-object", &a, Crop::Full);

    // Select top 100 rows
    let (mut a, _d, ids2) = scene();
    query_tab(&mut a, ids2, "SELECT * FROM sales.customer LIMIT 100;");
    start(&mut a);
    let mut rows = customer_rows();
    // 100 rows: the sample rows, then more individual customers
    let more: Vec<[String; 2]> = (0..90)
        .map(|i| [(11004 + i).to_string(), (11300 + i * 37 % 9000).to_string()])
        .collect();
    let more: &'static [[String; 2]] = Vec::leak(more);
    for m in more {
        rows.push(vec![
            m[0].as_str(),
            m[1].as_str(),
            "\\N",
            "9",
            "5b3c1e2a-0d4f-4c7b-9e1a-7f2d3c4b5a69",
            "2014-09-12 11:15:07.263",
        ]);
    }
    result(&mut a, None, CUSTOMER_COLS, &strs(&rows), false);
    tag(&mut a, "SELECT 100");
    finished(&mut a, 6, TxStatus::Idle);
    a.focus = Focus::Results;
    out.shot("top100", &a, Crop::Full);

    // ---- the object view --------------------------------------------------------
    // F3: a view in its own tab, with Find open
    let (mut a, _d, ids2) = scene();
    expand(&mut a, ids2.views);
    collapse(&mut a, ids2.tables);
    let view = a.tree.node(ids2.views).unwrap().children.as_ref().unwrap()[0].nid;
    select(&mut a, RowKind::Node(view));
    definition(&mut a, VIEW_DDL);
    a.focus = Focus::Tree;
    key(&mut a, "F3");
    {
        let t = a.active_tab;
        let v = &mut a.tabs[t].view;
        let title = v.title.clone();
        v.set_text(title, VIEW_DDL.into());
        v.loading = false;
    }
    a.status.clear();
    out.shot("object-tab", &a, Crop::Full);
    // Ctrl+E: the definition in a query tab
    a.active_tab = 0;
    a.focus = Focus::Tree;
    query_tab(&mut a, ids2, VIEW_DDL);
    out.shot("edit-in-tab", &a, Crop::Full);

    // ---- query tabs -------------------------------------------------------------
    let (a, _d, _) = query_scene();
    out.shot("query", &a, Crop::Full);

    // running
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(&mut a, ids2, TOP_QUERY);
    start(&mut a);
    out.shot("query-running", &a, Crop::Full);

    // a selection is executed
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(
        &mut a,
        ids2,
        "SELECT count(*) FROM sales.customer;\n\nSELECT name, \"group\" FROM sales.salesterritory ORDER BY name;\n\nSELECT count(*) FROM sales.store;",
    );
    a.editor().set_cursor(38, false);
    key(&mut a, "Shift+End");
    start(&mut a);
    result(
        &mut a,
        None,
        &[("name", 1043), ("group", 1043)],
        &[
            &["Australia", "Pacific"],
            &["Canada", "North America"],
            &["Central", "North America"],
            &["France", "Europe"],
            &["Germany", "Europe"],
            &["Northeast", "North America"],
            &["Northwest", "North America"],
            &["Southeast", "North America"],
            &["Southwest", "North America"],
            &["United Kingdom", "Europe"],
        ],
        false,
    );
    tag(&mut a, "SELECT 10");
    finished(&mut a, 3, TxStatus::Idle);
    out.shot("query-selection", &a, Crop::Full);

    // several result sets, the second one shown
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(
        &mut a,
        ids2,
        "SELECT territoryid, name, countryregioncode, salesytd FROM sales.salesterritory ORDER BY 1;\n\nSELECT name, rate FROM sales.currencyrate r JOIN sales.currency c ON c.currencycode = r.tocurrencycode\nWHERE r.currencyratedate = '2014-05-31' ORDER BY 1;\n\nSELECT count(*) AS stores FROM sales.store;",
    );
    start(&mut a);
    result(
        &mut a,
        None,
        &[
            ("territoryid", 23),
            ("name", 1043),
            ("countryregioncode", 1043),
            ("salesytd", 1700),
        ],
        &[
            &["1", "Northwest", "US", "7887186.7882"],
            &["2", "Northeast", "US", "2402176.8476"],
            &["3", "Central", "US", "3072175.118"],
            &["4", "Southwest", "US", "10510853.8739"],
            &["5", "Southeast", "US", "2538667.2515"],
            &["6", "Canada", "CA", "6771829.1376"],
            &["7", "France", "FR", "4772398.3078"],
            &["8", "Germany", "DE", "3805202.3478"],
            &["9", "Australia", "AU", "5977814.9154"],
            &["10", "United Kingdom", "GB", "5012905.3656"],
        ],
        false,
    );
    tag(&mut a, "SELECT 10");
    result(
        &mut a,
        None,
        &[("name", 1043), ("rate", 1700)],
        &[
            &["Argentine Peso", "1.0002"],
            &["Australian Dollar", "1.5491"],
            &["Bolivar", "652.3015"],
            &["Brazilian Real", "1.9379"],
            &["Canadian Dollar", "1.4641"],
            &["Deutsche Mark", "1.8967"],
            &["EURO", "0.9695"],
            &["French Franc", "6.3611"],
            &["Mexican Peso", "9.5078"],
            &["Saudi Riyal", "3.7534"],
            &["United Kingdom Pound", "0.6346"],
            &["Yen", "120.065"],
            &["Yuan Renminbi", "8.2781"],
        ],
        false,
    );
    tag(&mut a, "SELECT 13");
    result(&mut a, None, &[("stores", 20)], &[&["701"]], false);
    tag(&mut a, "SELECT 1");
    finished(&mut a, 21, TxStatus::Idle);
    a.focus = Focus::Results;
    key(&mut a, "Ctrl+PageDown");
    out.shot("query-multi", &a, Crop::Full);
    // its Messages
    key(&mut a, "Alt+M");
    out.shot("query-multi-messages", &a, Crop::Full);

    // a paused result: more rows available
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(
        &mut a,
        ids2,
        "SELECT salesorderid, salesorderdetailid, orderqty, productid, unitprice\nFROM sales.salesorderdetail\nORDER BY salesorderid, salesorderdetailid;",
    );
    start(&mut a);
    let many: Vec<Vec<String>> = (0..1000)
        .map(|i| {
            vec![
                (43659 + i / 9).to_string(),
                (i + 1).to_string(),
                (1 + i % 4).to_string(),
                (707 + (i * 7) % 290).to_string(),
                format!("{}.{:04}", 5 + (i * 37) % 2020, (i * 7919) % 10000),
            ]
        })
        .collect();
    let many_refs: Vec<Vec<&str>> = many
        .iter()
        .map(|r| r.iter().map(String::as_str).collect())
        .collect();
    let many_slices: Vec<&[&str]> = many_refs.iter().map(Vec::as_slice).collect();
    result(
        &mut a,
        None,
        &[
            ("salesorderid", 23),
            ("salesorderdetailid", 23),
            ("orderqty", 21),
            ("productid", 23),
            ("unitprice", 1700),
        ],
        &many_slices,
        true,
    );
    a.focus = Focus::Results;
    out.shot("query-more", &a, Crop::Full);

    // an error: Messages with LINE n and the caret
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    let text = "UPDATE sales.salesperson SET bonus = bonus + 500 WHERE territoryid = 4;\n\nSELECT p.firstname, p.lastname, s.salesqouta\nFROM sales.salesperson s\nJOIN person.person p ON p.businessentityid = s.businessentityid;";
    query_tab(&mut a, ids2, text);
    start(&mut a);
    tag(&mut a, "UPDATE 2");
    let pos = text.find("s.salesqouta").unwrap() as u32 + 1;
    apply(
        &mut a,
        pgc_db::QueryEvent::Error(pgc_db::ServerMessage {
            severity: "ERROR".into(),
            code: "42703".into(),
            message: "column s.salesqouta does not exist".into(),
            hint: Some("Perhaps you meant to reference the column \"s.salesquota\".".into()),
            position: Some(pos),
            ..Default::default()
        }),
    );
    finished(&mut a, 9, TxStatus::Idle);
    a.status = "Execution failed after 9 ms — see Messages".into();
    a.focus = Focus::Results;
    {
        let q = a.query_mut().unwrap();
        q.message_cursor = q.messages.len().saturating_sub(2);
    }
    out.shot("query-error", &a, Crop::Full);
    key(&mut a, "F2");
    out.shot("messages-menu", &a, Crop::Full);
    a.popup = None;
    key(&mut a, "Enter");
    a.status.clear();
    out.shot("query-error-jump", &a, Crop::Full);

    // results context menu
    let (mut a, _d, _) = query_scene();
    a.focus = Focus::Results;
    key(&mut a, "F2");
    out.shot("results-menu", &a, Crop::Full);

    // an open transaction: auto commit off
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(
        &mut a,
        ids2,
        "UPDATE sales.salesperson\nSET commissionpct = 0.02\nWHERE territoryid = 6;",
    );
    a.query_mut().unwrap().auto_commit = false;
    start(&mut a);
    apply(&mut a, pgc_db::QueryEvent::Info("BEGIN".into()));
    tag(&mut a, "UPDATE 2");
    finished(&mut a, 4, TxStatus::InTransaction);
    out.shot("tx-open", &a, Crop::Full);
    key(&mut a, "Ctrl+W");
    out.shot("tx-close", &a, Crop::Dialog);

    // closing an unsaved tab
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(&mut a, ids2, "");
    typing(&mut a, "SELECT * FROM sales.store;");
    key(&mut a, "Ctrl+W");
    out.shot("save-changes", &a, Crop::Dialog);

    // ---- the editor -------------------------------------------------------------
    let (mut a, _d, _) = query_scene();
    key(&mut a, "Ctrl+F");
    typing(&mut a, "total");
    out.shot("find", &a, Crop::Full);
    key(&mut a, "Tab");
    typing(&mut a, "revenue");
    key(&mut a, "Alt+R");
    out.shot("replace-confirm", &a, Crop::Full);

    // ---- autocomplete -----------------------------------------------------------
    let names = |a: &mut App| {
        let q = a.query_mut().unwrap();
        let obj =
            |schema: Option<&str>, name: &str, kind: &str, visible: bool| crate::completion::Obj {
                schema: schema.map(str::to_owned),
                name: name.into(),
                kind: kind.into(),
                visible,
            };
        let mut objects: Vec<crate::completion::Obj> = SCHEMAS
            .iter()
            .map(|s| obj(None, s, "schema", true))
            .collect();
        objects.extend(
            SALES_TABLES
                .iter()
                .map(|t| obj(Some("sales"), t, "table", false)),
        );
        for v in ["vindividualcustomer", "vsalesperson", "vstorewithaddresses"] {
            objects.push(obj(Some("sales"), v, "view", false));
        }
        for f in [
            "count",
            "coalesce",
            "concat",
            "concat_ws",
            "convert_from",
            "cos",
        ] {
            objects.push(obj(Some("pg_catalog"), f, "function", true));
        }
        q.names.objects = Some(objects);
        q.names.columns.insert(
            (Some("sales".into()), "customer".into()),
            [
                "customerid",
                "personid",
                "storeid",
                "territoryid",
                "rowguid",
                "modifieddate",
            ]
            .iter()
            .map(|s| (*s).into())
            .collect(),
        );
    };
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(
        &mut a,
        ids2,
        "SELECT c.customerid, c.\nFROM sales.customer c\nWHERE c.storeid IS NULL;",
    );
    names(&mut a);
    a.editor().set_cursor(23, false);
    popup_for(&mut a, true);
    out.shot("ac-columns", &a, Crop::Full);

    // keywords and functions by prefix
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(&mut a, ids2, "SELECT territoryid, co\nFROM sales.customer");
    names(&mut a);
    a.editor().set_cursor(22, false);
    popup_for(&mut a, false);
    out.shot("ac-prefix", &a, Crop::Full);

    // tables of a schema after FROM
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(&mut a, ids2, "SELECT *\nFROM sales.c");
    names(&mut a);
    a.editor().set_cursor(21, false);
    popup_for(&mut a, false);
    out.shot("ac-tables", &a, Crop::Full);

    // ---- PL/pgSQL mode ----------------------------------------------------------
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(
        &mut a,
        ids2,
        "DECLARE\n    t record;\n    n bigint;\nBEGIN\n    FOR t IN SELECT territoryid, name FROM sales.salesterritory ORDER BY name LOOP\n        SELECT count(*) INTO n FROM sales.customer c WHERE c.territoryid = t.territoryid;\n        RAISE NOTICE '% has % customers', t.name, n;\n    END LOOP;\nEND;",
    );
    a.query_mut().unwrap().plpgsql = true;
    start(&mut a);
    for (t, n) in [
        ("Australia", 3665),
        ("Canada", 1791),
        ("Central", 69),
        ("France", 1884),
        ("Germany", 1852),
        ("Northeast", 57),
        ("Northwest", 3520),
        ("Southeast", 91),
        ("Southwest", 4696),
        ("United Kingdom", 1991),
    ] {
        notice(&mut a, "NOTICE", &format!("{t} has {n} customers"));
    }
    tag(&mut a, "DO");
    finished(&mut a, 27, TxStatus::Idle);
    {
        let q = a.query_mut().unwrap();
        q.show_messages = true;
    }
    out.shot("plpgsql", &a, Crop::Full);

    // ---- files ------------------------------------------------------------------
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.focus = Focus::Tree;
    a.open_file_dialog(false);
    listing(&mut a, WORK_FILES, ".sql");
    a.dialogs.last_mut().unwrap().focus_on("files");
    a.dialogs.last_mut().unwrap().list_select("files", 3);
    out.shot("file-open", &a, Crop::Dialog);

    // a tab with a file: the file bar; Save as
    let (mut a, _d, _) = query_scene();
    {
        let dir = a.files.dir.clone();
        a.query_mut().unwrap().file = Some(dir.join("best_customers.sql"));
    }
    typing(&mut a, " ");
    key(&mut a, "Backspace");
    out.shot("file-bar", &a, Crop::Full);
    a.open_file_dialog(true);
    listing(&mut a, WORK_FILES, ".sql");
    out.shot("file-save", &a, Crop::Dialog);

    // the places
    let (mut a, _d, _) = query_scene();
    a.open_file_dialog(false);
    listing(&mut a, WORK_FILES, ".sql");
    key(&mut a, "Alt+P");
    {
        let place = |path: &str, note: &str| pgc_core::places::Place {
            path: path.into(),
            name: path.into(),
            note: note.into(),
        };
        let places = if cfg!(windows) {
            vec![
                place("~", ""),
                place("C:\\", "System"),
                place("D:\\", "Work"),
                place("E:\\", "CD/DVD"),
                place("N:\\", "Network"),
            ]
        } else {
            vec![
                place("~", ""),
                place("/", ""),
                place("/media/dev/BACKUP", ""),
                place("/mnt/nas", ""),
            ]
        };
        a.files.seq += 1;
        a.files.listing = Some(a.files.seq);
        let seq = a.files.seq;
        a.on_file_event(crate::app::file_jobs::FileEvent::Listed {
            seq,
            listing: crate::app::file_jobs::Listing::Places(places),
        });
    }
    out.shot("file-places", &a, Crop::Dialog);

    // ---- export -----------------------------------------------------------------
    let (mut a, _d, _) = query_scene();
    a.focus = Focus::Results;
    a.run(Command::ExportResult);
    listing(
        &mut a,
        &[
            ("migrations", true),
            ("reports", true),
            ("territories.csv", false),
        ],
        ".csv",
    );
    out.shot("export", &a, Crop::Dialog);

    // ---- psql commands ----------------------------------------------------------
    let (mut a, _d, _) = query_scene();
    let d = a.psql_dialog("");
    a.dialogs.push(d);
    typing(&mut a, "dt sales.*");
    out.shot("psql", &a, Crop::Full);

    // dt sales.*
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(&mut a, ids2, "");
    start(&mut a);
    let dt: Vec<Vec<&str>> = SALES_TABLES
        .iter()
        .map(|t| vec!["sales", *t, "table", "postgres"])
        .collect();
    result(
        &mut a,
        Some("dt sales.*"),
        &[("Schema", 19), ("Name", 19), ("Type", 25), ("Owner", 19)],
        &strs(&dt),
        false,
    );
    finished(&mut a, 5, TxStatus::Idle);
    a.focus = Focus::Results;
    out.shot("psql-dt", &a, Crop::Full);

    // d sales.customer
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(&mut a, ids2, "");
    start(&mut a);
    result(
        &mut a,
        Some("Relation"),
        &[
            ("Schema", 19),
            ("Name", 19),
            ("Type", 25),
            ("Owner", 19),
            ("Description", 25),
        ],
        &[&[
            "sales",
            "customer",
            "table",
            "postgres",
            "Current customer information. Also see the Person and Store tables.",
        ]],
        false,
    );
    result(
        &mut a,
        Some("Columns"),
        &[
            ("Column", 19),
            ("Type", 25),
            ("Collation", 25),
            ("Nullable", 25),
            ("Default", 25),
        ],
        &[
            &[
                "customerid",
                "integer",
                "\\N",
                "not null",
                "nextval('sales.customer_customerid_seq'::regclass)",
            ],
            &["personid", "integer", "\\N", "\\N", "\\N"],
            &["storeid", "integer", "\\N", "\\N", "\\N"],
            &["territoryid", "integer", "\\N", "\\N", "\\N"],
            &["rowguid", "uuid", "\\N", "not null", "uuid_generate_v1()"],
            &[
                "modifieddate",
                "timestamp without time zone",
                "\\N",
                "not null",
                "now()",
            ],
        ],
        false,
    );
    result(
        &mut a,
        Some("Indexes"),
        &[("Name", 19), ("Kind", 25), ("Definition", 25)],
        &[
            &[
                "PK_Customer_CustomerID",
                "primary key",
                "btree (customerid)",
            ],
            &["ix_customer_territoryid", "index", "btree (territoryid)"],
        ],
        false,
    );
    result(
        &mut a,
        Some("Constraints"),
        &[("Name", 19), ("Type", 25), ("Definition", 25)],
        &[
            &[
                "FK_Customer_Person_PersonID",
                "foreign key",
                "FOREIGN KEY (personid) REFERENCES person.person(businessentityid)",
            ],
            &[
                "FK_Customer_SalesTerritory_TerritoryID",
                "foreign key",
                "FOREIGN KEY (territoryid) REFERENCES sales.salesterritory(territoryid)",
            ],
            &[
                "FK_Customer_Store_StoreID",
                "foreign key",
                "FOREIGN KEY (storeid) REFERENCES sales.store(businessentityid)",
            ],
        ],
        false,
    );
    finished(&mut a, 11, TxStatus::Idle);
    a.focus = Focus::Results;
    key(&mut a, "Ctrl+PageDown");
    out.shot("psql-d", &a, Crop::Full);

    // the generate commands: real generators on the shape of sales.customer
    let gen_customer = {
        use crate::psql::{GenColumn, GenTable};
        let col = |name: &str, ty: &str, not_null: bool, default: Option<&str>, pk: Option<u32>| {
            GenColumn {
                name: name.into(),
                type_name: ty.into(),
                not_null,
                default: default.map(str::to_owned),
                identity: String::new(),
                generated: false,
                pk_pos: pk,
                uk_pos: None,
            }
        };
        GenTable {
            schema: "sales".into(),
            name: "customer".into(),
            columns: vec![
                col(
                    "customerid",
                    "integer",
                    true,
                    Some("nextval('sales.customer_customerid_seq'::regclass)"),
                    Some(1),
                ),
                col("personid", "integer", false, None, None),
                col("storeid", "integer", false, None, None),
                col("territoryid", "integer", false, None, None),
                col("rowguid", "uuid", true, Some("uuid_generate_v1()"), None),
                col(
                    "modifieddate",
                    "timestamp without time zone",
                    true,
                    Some("now()"),
                    None,
                ),
            ],
            unique: None,
            kind: "r".into(),
        }
    };
    for (name, what) in [
        ("gen-gts", crate::psql::Gen::Select),
        ("gen-gti", crate::psql::Gen::Insert),
        ("gen-gtu", crate::psql::Gen::Update),
        ("gen-gto", crate::psql::Gen::Upsert),
    ] {
        let (mut a, _d, ids2) = scene();
        short_tree(&mut a, ids2);
        let sql = crate::psql::generate(what, &gen_customer);
        query_tab(&mut a, ids2, &sql);
        a.query_mut().unwrap().results_visible = false;
        out.shot(name, &a, Crop::Area(32, 1, 88, 22));
        if let Some(t) = &out.txt {
            std::fs::write(t.join(format!("{name}.sql")), &sql).unwrap();
        }
    }
    for (name, what) in [
        ("gtd", crate::psql::Gen::Delete),
        ("gtc", crate::psql::Gen::Cursor),
        ("gtsi", crate::psql::Gen::Vars),
        ("gtm", crate::psql::Gen::Merge),
        ("gpi", crate::psql::Gen::ProcInsert),
        ("gpu", crate::psql::Gen::ProcUpdate),
        ("gpd", crate::psql::Gen::ProcDelete),
        ("gpo", crate::psql::Gen::ProcUpsert),
        (
            "gtrb",
            crate::psql::Gen::Trigger {
                row: true,
                before: true,
            },
        ),
        (
            "gtsa",
            crate::psql::Gen::Trigger {
                row: false,
                before: false,
            },
        ),
    ] {
        if let Some(t) = &out.txt {
            std::fs::write(
                t.join(format!("gen-{name}.sql")),
                crate::psql::generate(what, &gen_customer),
            )
            .unwrap();
        }
    }
    // gtv: a view over three tables
    {
        use crate::psql::{ViewFk, ViewTable};
        let t = |name: &str, cols: &[&str]| ViewTable {
            schema: "sales".into(),
            name: name.into(),
            columns: cols.iter().map(|c| (*c).to_owned()).collect(),
        };
        let tables = vec![
            t(
                "salesorderheader",
                &[
                    "salesorderid",
                    "orderdate",
                    "customerid",
                    "territoryid",
                    "totaldue",
                ],
            ),
            t(
                "customer",
                &["customerid", "personid", "storeid", "territoryid"],
            ),
            t("salesterritory", &["territoryid", "name", "group"]),
        ];
        let fks = vec![
            ViewFk {
                from: 0,
                to: 1,
                name: "FK_SalesOrderHeader_Customer_CustomerID".into(),
                pairs: vec![("customerid".into(), "customerid".into())],
                nullable: false,
            },
            ViewFk {
                from: 0,
                to: 2,
                name: "FK_SalesOrderHeader_SalesTerritory_TerritoryID".into(),
                pairs: vec![("territoryid".into(), "territoryid".into())],
                nullable: true,
            },
        ];
        let sql = crate::psql::generate_view(&tables, &fks);
        let (mut a, _d, ids2) = scene();
        short_tree(&mut a, ids2);
        query_tab(&mut a, ids2, &sql);
        a.query_mut().unwrap().results_visible = false;
        out.shot("gen-gtv", &a, Crop::Area(32, 1, 88, 24));
        if let Some(t) = &out.txt {
            std::fs::write(t.join("gen-gtv.sql"), &sql).unwrap();
        }
    }

    // ---- developer templates ----------------------------------------------------
    let (mut a, _d, _) = query_scene();
    pick_template(&mut a, "CREATE TABLE");
    out.shot("tpl-picker", &a, Crop::Full);
    // the search box
    let (mut a, _d, _) = query_scene();
    pick_template(&mut a, "CREATE");
    key(&mut a, "Alt+S");
    typing(&mut a, "drop");
    out.shot("tpl-search", &a, Crop::Dialog);

    // Create table
    let (mut a, _d, _) = query_scene();
    pick_template(&mut a, "CREATE TABLE");
    key(&mut a, "Enter");
    {
        use crate::template_dialog::Stop;
        use pgc_sql::devtemplate::ParamValue;
        let columns = field_index(&a, "columns");
        set_text_field(&mut a, "name", "customernote");
        let form = a.templates.form.as_mut().unwrap();
        form.set_choices("schema", Ok(text_choices(SCHEMAS)));
        form.want("schema", "sales".into());
        form.set_choices("schema", Ok(text_choices(SCHEMAS)));
        let text = |s: &str| ParamValue::Text(s.into());
        let col = |n: &str, t: &str, not_null: bool, pk: bool, default: &str| {
            std::collections::BTreeMap::from([
                ("col_name".to_owned(), text(n)),
                ("col_type".to_owned(), text(t)),
                ("not_null".to_owned(), ParamValue::Bool(not_null)),
                ("primary_key".to_owned(), ParamValue::Bool(pk)),
                ("col_default".to_owned(), text(default)),
            ])
        };
        *form.list_rows_mut(columns).unwrap() = vec![
            col("customernoteid", "bigint", false, true, ""),
            col("customerid", "integer", true, false, ""),
            col("note", "text", true, false, ""),
            col("createdby", "text", true, false, "CURRENT_USER"),
            col("modifieddate", "timestamp", true, false, "now()"),
        ];
        form.list_select(columns, 2);
        form.focus = Stop::Field(columns);
        form.refresh();
    }
    out.shot("tpl-table", &a, Crop::Full);
    // editing a row of the list
    key(&mut a, "Enter");
    out.shot("tpl-row", &a, Crop::Full);

    // Create function: the SQL box
    let (mut a, _d, _) = query_scene();
    pick_template(&mut a, "CREATE FUNCTION");
    key(&mut a, "Enter");
    {
        use crate::template_dialog::{FieldValue, Stop};
        use pgc_sql::devtemplate::ParamValue;
        let (args, body) = (field_index(&a, "args"), field_index(&a, "body"));
        set_text_field(&mut a, "name", "customer_total");
        set_text_field(&mut a, "returns", "numeric");
        let form = a.templates.form.as_mut().unwrap();
        form.want("schema", "sales".into());
        form.set_choices("schema", Ok(text_choices(SCHEMAS)));
        let text = |s: &str| ParamValue::Text(s.into());
        *form.list_rows_mut(args).unwrap() = vec![
            std::collections::BTreeMap::from([
                ("arg_name".to_owned(), text("p_customerid")),
                ("arg_type".to_owned(), text("integer")),
                ("arg_default".to_owned(), text("")),
            ]),
            std::collections::BTreeMap::from([
                ("arg_name".to_owned(), text("p_year")),
                ("arg_type".to_owned(), text("integer")),
                ("arg_default".to_owned(), text("2014")),
            ]),
        ];
        if let FieldValue::Sql(ed) = &mut form.fields[body].value {
            ed.set_text(
                "RETURN (\n    SELECT COALESCE(sum(h.totaldue), 0)\n    FROM sales.salesorderheader h\n    WHERE h.customerid = p_customerid\n      AND EXTRACT(year FROM h.orderdate) = p_year\n);",
            );
        }
        form.focus = Stop::Field(body);
        form.refresh();
    }
    out.shot("tpl-function", &a, Crop::Full);

    // Drop table: the table of the tree is pre-filled
    let (mut a, _d, _) = query_scene();
    pick_template(&mut a, "DROP TABLE");
    key(&mut a, "Enter");
    {
        use crate::template_dialog::{Choice, Stop};
        use pgc_sql::devtemplate::{ParamValue, QualName};
        let object = field_index(&a, "object");
        let form = a.templates.form.as_mut().unwrap();
        let tables: Vec<Choice> = SALES_TABLES
            .iter()
            .map(|t| Choice {
                label: format!("sales.{t}"),
                value: ParamValue::Object(QualName::new("sales", *t)),
            })
            .collect();
        form.want("object", "sales.shoppingcartitem".into());
        form.set_choices("object", Ok(tables));
        form.focus = Stop::Field(object);
        form.refresh();
    }
    key(&mut a, "Enter");
    out.shot("tpl-drop", &a, Crop::Full);

    // the example template of the help page "Writing your own templates": written
    // into the templates folder, then found by the picker like any other
    let (mut a, _d, _) = query_scene();
    a.run(Command::InstallTemplates);
    a.dialogs.clear();
    let own = a
        .paths
        .user_templates_dir()
        .join("default")
        .join("Our team");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("lookup_table.pgtemplate"), OWN_TEMPLATE).unwrap();
    pick_template(&mut a, "A code / name table");
    out.shot("tpl-own-picker", &a, Crop::Dialog);
    key(&mut a, "Enter");
    {
        use crate::template_dialog::{FieldValue, Stop};
        let (name, audit) = (field_index(&a, "name"), field_index(&a, "with_audit"));
        set_text_field(&mut a, "name", "orderstatus");
        let form = a.templates.form.as_mut().unwrap();
        form.want("schema", "sales".into());
        form.set_choices("schema", Ok(text_choices(SCHEMAS)));
        form.fields[audit].value = FieldValue::Bool(true);
        form.focus = Stop::Field(name);
        form.refresh();
        let sql = form.result().expect("the example template renders");
        assert!(sql.contains("CREATE TABLE sales.orderstatus ("), "{sql}");
        assert!(sql.contains("code varchar(10) NOT NULL"), "{sql}");
        assert!(sql.contains("modifiedby text NOT NULL"), "{sql}");
        assert!(
            sql.contains("CONSTRAINT orderstatus_pkey PRIMARY KEY (code)"),
            "{sql}"
        );
    }
    out.shot("tpl-own", &a, Crop::Full);

    // ---- database definition ----------------------------------------------------
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.focus = Focus::Tree;
    a.run(Command::ExportDefinition);
    listing(
        &mut a,
        &[
            ("migrations", true),
            ("reports", true),
            ("adventureworks_2026-10.pgdef", false),
        ],
        ".pgdef",
    );
    out.shot("pgdef-export", &a, Crop::Dialog);

    // the sync script, made by the real generator from two small models
    let model = |extra: bool| {
        let note = if extra {
            r#",
            {"name": "note", "type": "text"},
            {"name": "createdby", "type": "text", "not_null": true, "default": "CURRENT_USER"}"#
        } else {
            ""
        };
        let table = if extra {
            r#",
        {
          "name": "customernote",
          "columns": [
            {"name": "customernoteid", "type": "bigint", "not_null": true, "identity": "ALWAYS"},
            {"name": "customerid", "type": "integer", "not_null": true},
            {"name": "note", "type": "text", "not_null": true}
          ],
          "constraints": [
            {"name": "customernote_pkey", "kind": "primary_key", "definition": "PRIMARY KEY (customernoteid)"},
            {"name": "customernote_customerid_fkey", "kind": "foreign_key",
             "definition": "FOREIGN KEY (customerid) REFERENCES sales.customer(customerid)",
             "references": {"schema": "sales", "name": "customer"}}
          ],
          "indexes": [
            {"name": "ix_customernote_customerid",
             "definition": "CREATE INDEX ix_customernote_customerid ON sales.customernote USING btree (customerid)"}
          ]
        }"#
        } else {
            ""
        };
        let view = if extra {
            "SELECT c.customerid, c.territoryid, count(n.*) AS notes FROM sales.customer c LEFT JOIN sales.customernote n ON n.customerid = c.customerid GROUP BY c.customerid, c.territoryid;"
        } else {
            "SELECT c.customerid, c.territoryid FROM sales.customer c;"
        };
        format!(
            r#"{{
  "format": 1,
  "schemas": [
    {{
      "name": "sales",
      "tables": [
        {{
          "name": "customer",
          "columns": [
            {{"name": "customerid", "type": "integer", "not_null": true}},
            {{"name": "personid", "type": "integer"}},
            {{"name": "storeid", "type": "integer"}},
            {{"name": "territoryid", "type": "integer"}}{note}
          ],
          "constraints": [
            {{"name": "PK_Customer_CustomerID", "kind": "primary_key", "definition": "PRIMARY KEY (customerid)"}}
          ]
        }}{table}
      ],
      "views": [
        {{"name": "vcustomeroverview", "definition": "{view}"}}
      ]
    }}
  ]
}}"#
        )
    };
    let want = pgc_model::file::from_text(&model(true)).expect("want model");
    let live = pgc_model::file::from_text(&model(false)).expect("live model");
    let script = pgc_model::sync::sync_script(&want, &live).expect("a difference");
    if let Some(t) = &out.txt {
        std::fs::write(t.join("pgdef-script.sql"), &script).unwrap();
        std::fs::write(t.join("pgdef-file.pgdef"), pgc_model::file::to_text(&want)).unwrap();
    }
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    query_tab(&mut a, ids2, &script);
    a.query_mut().unwrap().results_visible = false;
    out.shot("pgdef-script", &a, Crop::Full);

    // ---- server dashboard -------------------------------------------------------
    let dash = || {
        let (mut a, d, ids2) = scene();
        short_tree(&mut a, ids2);
        a.focus = Focus::Tree;
        dashboard(&mut a);
        (a, d)
    };
    let (mut a, _d) = dash();
    out.shot("dashboard-pane", &a, Crop::Full);
    // without the statement pane (Ctrl+R): room for the whole list
    key(&mut a, "Ctrl+R");
    a.status.clear();
    out.shot("dashboard", &a, Crop::Full);
    let (mut a, _d) = dash();
    key(&mut a, "Enter");
    out.shot("dashboard-row", &a, Crop::Full);
    let (mut a, _d) = dash();
    key(&mut a, "F2");
    out.shot("dashboard-menu", &a, Crop::Full);
    let (mut a, _d) = dash();
    key(&mut a, "F8");
    out.shot("dashboard-terminate", &a, Crop::Dialog);
    // tagging and the transaction duration
    let (mut a, _d) = dash();
    key(&mut a, "Down");
    key(&mut a, "Insert");
    key(&mut a, "Insert");
    key(&mut a, "Alt+T");
    out.shot("dashboard-tags", &a, Crop::Full);

    // the other lists
    let (mut a, _d) = dash();
    {
        use crate::dashboard::List;
        let d = a.tabs[a.active_tab].dashboard_mut().unwrap();
        d.apply_rows(
            List::Blocking,
            to_rows(&[&[
                "41377",
                DB,
                "webshop",
                "idle in transaction",
                "754",
                "41502",
                "webshop",
                "transactionid",
                "ShareLock",
                "",
                "48.6",
                "UPDATE sales.salesorderheader SET status = 5 WHERE salesorderid = $1",
                "UPDATE sales.salesorderheader SET shipdate = now() WHERE salesorderid = $1",
            ]]),
        );
        d.apply_rows(
            List::Progress,
            to_rows(&[
                &[
                    "40112",
                    DB,
                    "",
                    "VACUUM",
                    "sales.salesorderdetail",
                    "scanning heap",
                    "1240",
                    "3105",
                    "blocks",
                    "21",
                    "autovacuum: VACUUM sales.salesorderdetail",
                ],
                &[
                    "41950",
                    DB,
                    "postgres",
                    "CREATE INDEX",
                    "production.transactionhistory",
                    "building index: scanning table",
                    "5120",
                    "8200",
                    "blocks",
                    "9.4",
                    "CREATE INDEX CONCURRENTLY ix_transactionhistory_productid ON production.transactionhistory (productid)",
                ],
            ]),
        );
        d.apply_rows(
            List::Locks,
            to_rows(&[
                &[
                    "41377",
                    "relation",
                    DB,
                    "sales.salesorderheader",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "5/1204",
                    "RowExclusiveLock",
                    "true",
                    "true",
                ],
                &[
                    "41377",
                    "transactionid",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "88104",
                    "",
                    "",
                    "",
                    "5/1204",
                    "ExclusiveLock",
                    "true",
                    "false",
                ],
                &[
                    "41502",
                    "relation",
                    DB,
                    "sales.salesorderheader",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "7/881",
                    "RowExclusiveLock",
                    "true",
                    "true",
                ],
                &[
                    "41502",
                    "transactionid",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "88104",
                    "",
                    "",
                    "",
                    "7/881",
                    "ShareLock",
                    "false",
                    "false",
                ],
                &[
                    "41502",
                    "tuple",
                    DB,
                    "sales.salesorderheader",
                    "412",
                    "7",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "7/881",
                    "ExclusiveLock",
                    "true",
                    "false",
                ],
                &[
                    "40112",
                    "relation",
                    DB,
                    "sales.salesorderdetail",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "",
                    "9/3310",
                    "ShareUpdateExclusiveLock",
                    "true",
                    "false",
                ],
            ]),
        );
        d.apply_rows(
            List::Prepared,
            to_rows(&[&[
                "order-import-7731",
                DB,
                "webshop",
                "88090",
                "2026-11-16 09:12:44",
            ]]),
        );
        d.apply_rows(
            List::Configuration,
            to_rows(&[
                &[
                    "autovacuum",
                    "Autovacuum",
                    "on",
                    "",
                    "Starts the autovacuum subprocess.",
                ],
                &[
                    "autovacuum_naptime",
                    "Autovacuum",
                    "60",
                    "s",
                    "Time to sleep between autovacuum runs.",
                ],
                &[
                    "checkpoint_timeout",
                    "Write-Ahead Log / Checkpoints",
                    "300",
                    "s",
                    "Sets the maximum time between automatic WAL checkpoints.",
                ],
                &[
                    "effective_cache_size",
                    "Query Tuning / Planner Cost Constants",
                    "786432",
                    "8kB",
                    "Sets the planner's assumption about the total size of the data caches.",
                ],
                &[
                    "log_min_duration_statement",
                    "Reporting and Logging / When to Log",
                    "-1",
                    "ms",
                    "Sets the minimum execution time above which all statements will be logged.",
                ],
                &[
                    "max_connections",
                    "Connections and Authentication / Connection Settings",
                    "200",
                    "",
                    "Sets the maximum number of concurrent connections.",
                ],
                &[
                    "max_wal_senders",
                    "Replication / Sending Servers",
                    "10",
                    "",
                    "Sets the maximum number of simultaneously running WAL sender processes.",
                ],
                &[
                    "shared_buffers",
                    "Resource Usage / Memory",
                    "262144",
                    "8kB",
                    "Sets the number of shared memory buffers used by the server.",
                ],
                &[
                    "work_mem",
                    "Resource Usage / Memory",
                    "16384",
                    "kB",
                    "Sets the maximum memory to be used for query workspaces.",
                ],
            ]),
        );
    }
    for name in ["blocking", "progress", "locks", "prepared", "configuration"] {
        key(&mut a, "Alt+Right");
        key(&mut a, "Down");
        key(&mut a, "Up");
        a.status.clear();
        out.shot(&format!("dashboard-{name}"), &a, Crop::Full);
    }

    // ---- SQL profiler -----------------------------------------------------------
    let prof = || {
        let (mut a, d, ids2) = scene();
        short_tree(&mut a, ids2);
        a.tree.select_server(ids2.uid);
        a.follow_selection();
        a.focus = Focus::Tree;
        let (tab, conn) = profiler(&mut a);
        (a, d, tab, conn)
    };
    // Start: the question
    let (mut a, _d, tab, conn) = prof();
    out.shot("profiler-stopped", &a, Crop::Full);
    key(&mut a, "F5");
    let k = a.tabs[a.active_tab].profiler().unwrap().step_keys[0];
    a.on_profiler_event(
        conn,
        tab,
        DbEvent::Lookup {
            id: conn,
            key: k,
            result: Ok(to_rows(&[&[
                "on", "stderr", "default", "-1", "default", "99",
            ]])),
        },
    );
    out.shot("profiler-start", &a, Crop::Dialog);
    // running
    let (mut a, _d, _, _) = prof();
    profiler_records(&mut a);
    a.status.clear();
    out.shot("profiler", &a, Crop::Full);
    key(&mut a, "Enter");
    out.shot("profiler-entry", &a, Crop::Full);
    let (mut a, _d, _, _) = prof();
    profiler_records(&mut a);
    key(&mut a, "Ctrl+F");
    typing(&mut a, "reporting");
    out.shot("profiler-filter", &a, Crop::Dialog);
    let (mut a, _d, _, _) = prof();
    profiler_records(&mut a);
    key(&mut a, "F2");
    out.shot("profiler-menu", &a, Crop::Full);

    // ---- database statistics ----------------------------------------------------
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.focus = Focus::Tree;
    a.settings.ui.tree_width = Some(26);
    a.run(Command::DbStats);
    {
        let s = a.tabs[a.active_tab].db_stats_mut().unwrap();
        let c = s.conn.as_mut().unwrap();
        c.handle.abort();
        c.handle = pgc_db::ConnectionHandle::detached(c.id);
        c.connected = true;
        s.version = Some(pgc_core::ServerVersion(170002));
        s.key = None;
        s.apply_rows(&to_rows(&[
            &[
                "read_at",
                "Database",
                "Read at",
                "time",
                "2026-11-16 10:48:12+01",
            ],
            &["size", "Database", "Size", "bytes", "1310368640"],
            &["backends", "Database", "Backends", "count", "41"],
            &[
                "conn_limit",
                "Database",
                "Connection limit",
                "text",
                "no limit",
            ],
            &[
                "xid_age",
                "Database",
                "Transaction ID age",
                "count",
                "1843207",
            ],
            &["mxid_age", "Database", "Multixact ID age", "count", "12"],
            &[
                "stats_reset",
                "Database",
                "Statistics reset",
                "time",
                "2026-10-01 06:00:11+02",
            ],
            &[
                "xact_commit",
                "Transactions",
                "Committed",
                "count",
                "9041377",
            ],
            &[
                "xact_rollback",
                "Transactions",
                "Rolled back",
                "count",
                "50231",
            ],
            &["deadlocks", "Transactions", "Deadlocks", "count", "4"],
            &["blks_read", "Block I/O", "Blocks read", "count", "7841233"],
            &["blks_hit", "Block I/O", "Blocks hit", "count", "1284977310"],
            &[
                "cache_hit",
                "Block I/O",
                "Cache hit ratio",
                "percent",
                "99.39",
            ],
            &["blk_read_time", "Block I/O", "Read time", "ms", "412877.4"],
            &["blk_write_time", "Block I/O", "Write time", "ms", "98114.2"],
            &["tup_returned", "Tuples", "Returned", "count", "6120447781"],
            &["tup_fetched", "Tuples", "Fetched", "count", "981277340"],
            &["tup_inserted", "Tuples", "Inserted", "count", "13377041"],
            &["tup_updated", "Tuples", "Updated", "count", "8041226"],
            &["tup_deleted", "Tuples", "Deleted", "count", "1002977"],
            &["temp_files", "Temporary files", "Files", "count", "318"],
            &[
                "temp_bytes",
                "Temporary files",
                "Size",
                "bytes",
                "50331648000",
            ],
            &["sessions", "Sessions", "Sessions", "count", "184302"],
            &[
                "session_time",
                "Sessions",
                "Session time",
                "ms",
                "9912004411",
            ],
            &["active_time", "Sessions", "Active time", "ms", "411200877"],
            &["sessions_abandoned", "Sessions", "Abandoned", "count", "37"],
            &[
                "checksum_failures",
                "Checksums",
                "Failures",
                "text",
                "checksums off",
            ],
            &["confl_lock", "Recovery conflicts", "Lock", "count", "0"],
        ]));
    }
    a.focus = Focus::Results;
    a.status.clear();
    out.shot("dbstats", &a, Crop::Full);

    // ---- settings and colour schemes --------------------------------------------
    let (mut a, _d, _) = query_scene();
    a.settings.clipboard.system = true;
    a.run(Command::Settings);
    out.shot("settings", &a, Crop::Dialog);
    key(&mut a, "Space");
    out.shot("settings-scheme", &a, Crop::Dialog);

    for name in crate::theme::ThemeSet::built_in().names() {
        let (mut a, _d, ids2) = scene_with(&name);
        short_tree(&mut a, ids2);
        query_tab(&mut a, ids2, TOP_QUERY);
        start(&mut a);
        result(&mut a, None, TOP_COLS, TOP_ROWS, false);
        tag(&mut a, "SELECT 12");
        finished(&mut a, 184, TxStatus::Idle);
        let file: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        out.shot(&format!("scheme-{file}"), &a, Crop::TopLeft(120, 24));
    }

    // ASCII frames
    let (mut a, _d, _) = query_scene();
    a.settings.ui.ascii_frames = true;
    a.settings.clipboard.system = true;
    a.theme.ascii = true;
    a.run(Command::Settings);
    out.shot("ascii-frames", &a, Crop::Full);

    // the list of files, for the help's own check
    if let Some(t) = &out.txt {
        std::fs::write(t.join("_names.txt"), out.names.borrow().join("\n")).unwrap();
    }
}
