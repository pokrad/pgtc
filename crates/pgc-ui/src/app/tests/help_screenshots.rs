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
            // lines and blocks over the whole cell, not as glyphs
            let shape = |x: u16| {
                let (fg, ..) = style_of(x);
                super::svg_cells::cell_shapes(
                    buf[(x, y)].symbol(),
                    (x - area.x) as f32 * cw,
                    py,
                    cw,
                    ch,
                    &hex(fg),
                )
            };
            if super::svg_cells::stretches(sym) {
                // a horizontal line: its run as one shape
                let mut end = x + 1;
                while end < area.right()
                    && buf[(end, y)].symbol() == sym
                    && style_of(end) == style_of(x)
                {
                    text.push_str(sym);
                    end += 1;
                }
                let (fg, ..) = style_of(x);
                let px = (x - area.x) as f32 * cw;
                let w = (end - x) as f32 * cw;
                if let Some(s) = super::svg_cells::cell_shapes(sym, px, py, w, ch, &hex(fg)) {
                    out.push_str(&s);
                }
                x = end;
                continue;
            }
            if let Some(s) = shape(x) {
                out.push_str(&s);
                x += 1;
                continue;
            }
            let style = style_of(x);
            let (mut xs, mut chars) = (String::new(), String::new());
            let mut end = x;
            while end < area.right()
                && !buf[(end, y)].symbol().trim().is_empty()
                && style_of(end) == style
                && shape(end).is_none()
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

/// The templates folder as the screenshots show it (the scene's own is a temporary
/// folder with a new name in every run).
fn shown_templates_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(if cfg!(windows) {
        "C:\\Users\\dev\\AppData\\Roaming\\pgtc\\developer_templates"
    } else {
        "/home/dev/.local/share/pgtc/developer_templates"
    })
}

/// The scene with pgtc's templates installed and the example template of the help
/// page in the section *Our team*; returns the example's file.
fn templates_scene() -> (App, tempfile::TempDir, Ids, std::path::PathBuf) {
    let (mut a, d, ids) = scene();
    short_tree(&mut a, ids);
    a.run(Command::InstallTemplates);
    a.dialogs.clear();
    a.status.clear();
    let own = a
        .paths
        .user_templates_dir()
        .join("default")
        .join("Our team");
    std::fs::create_dir_all(&own).unwrap();
    let file = own.join("lookup_table.pgtemplate");
    std::fs::write(&file, OWN_TEMPLATE).unwrap();
    (a, d, ids, file)
}

/// [`templates_scene`] with the example template open in a template tab (D-074), the
/// cursor in its body.
fn template_tab_scene() -> (App, tempfile::TempDir, Ids) {
    let (mut a, d, ids, file) = templates_scene();
    a.open_template_file(file, OWN_TEMPLATE);
    show_template_path(&mut a, &["default", "Our team", "lookup_table.pgtemplate"]);
    (a, d, ids)
}

/// The file bar of the active template tab shows a path in the user's folder.
fn show_template_path(a: &mut App, parts: &[&str]) {
    let mut path = shown_templates_dir();
    path.extend(parts);
    a.template_mut().unwrap().path = Some(path);
    a.focus = Focus::Tabs;
    a.status.clear();
    a.sync_template_tab();
    template_layout(a);
}

/// The active template tab learns where its parts are on the screenshot's screen (the
/// sizes of its texts, the places of its menus), as it does from every frame.
fn template_layout(a: &mut App) {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
    let mut hits = Hits::default();
    terminal
        .draw(|f| crate::view::render(f, a, &mut hits))
        .unwrap();
    a.set_template_views(&hits);
}

/// Moves the list `id` of the dialog on top to the row whose text (trimmed) is `label`.
fn select_row(a: &mut App, id: &str, label: &str) {
    let d = a.dialogs.last_mut().expect("a dialog");
    let at = match d.widgets.iter().find(|w| w.id == id).map(|w| &w.kind) {
        Some(crate::dialog::WidgetKind::List { items, .. }) => items
            .iter()
            .position(|i| i.trim() == label)
            .unwrap_or_else(|| panic!("no row {label}")),
        _ => panic!("no list {id}"),
    };
    d.list_select(id, at);
    // what follows the selection (a description, a note)
    a.sync_template_manager();
    a.sync_insert_dialog();
}

/// Puts the cursor of the header list of the active template tab on parameter `name`.
fn select_param(a: &mut App, name: &str) {
    use crate::template_tab::{FIXED_ROWS, Part};
    let t = a.template_mut().unwrap();
    let at = t
        .params
        .iter()
        .position(|p| p.name == name)
        .unwrap_or_else(|| panic!("no parameter {name}"));
    t.part = Part::Header;
    t.cursor = FIXED_ROWS + at;
}

/// Opens the dashboard of the selected row with a detached connection.
fn dashboard(a: &mut App) {
    a.settings.ui.tree_width = Some(26);
    a.run(Command::Dashboard);
    let d = a.tabs[a.active_tab].dashboard_mut().unwrap();
    let c = d.own.conn.as_mut().unwrap();
    c.handle.abort();
    c.handle = pgc_db::ConnectionHandle::detached(c.id);
    c.connected = true;
    d.own.version = Some(pgc_core::ServerVersion(170002));
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
    let c = p.own.conn.as_mut().unwrap();
    c.handle.abort();
    c.handle = pgc_db::ConnectionHandle::detached(c.id);
    c.connected = true;
    p.own.version = Some(pgc_core::ServerVersion(170002));
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

    // a menu (the Reference chapter lists the items of all of them)
    {
        let (mut a, _d, _) = query_scene();
        key(&mut a, "F9");
        key(&mut a, "Right");
        key(&mut a, "Right");
        out.shot("menu-query", &a, Crop::TopLeft(96, 20));
    }

    // F1 in the query editor; Help → Keyboard shortcuts
    let (mut a, _d, _) = query_scene();
    key(&mut a, "F1");
    out.shot("f1", &a, Crop::Full);
    let (mut a, _d, _) = query_scene();
    a.run(Command::KeyboardShortcuts);
    out.shot("shortcuts", &a, Crop::Full);

    // ---- connections ------------------------------------------------------------
    let (mut a, _d, _) = scene();
    a.focus = Focus::Tree;
    a.run(Command::NewConnection);
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

    // the connected server: its context menu
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.tree.select_server(ids2.uid);
    a.follow_selection();
    a.focus = Focus::Tree;
    key(&mut a, "F2");
    out.shot("server-menu", &a, Crop::TopLeft(84, 20));

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

    // ---- the editor -------------------------------------------------------------
    let (mut a, _d, _) = query_scene();
    key(&mut a, "Ctrl+F");
    typing(&mut a, "total");
    out.shot("find", &a, Crop::Full);

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

    // completing the argument: the schemas first, then the tables
    let (mut a, _d, _) = query_scene();
    names(&mut a);
    let d = a.psql_dialog("");
    a.dialogs.push(d);
    typing(&mut a, "dt ");
    key(&mut a, "Ctrl+Space");
    assert!(a.psql_completion.is_some());
    out.shot("psql-complete", &a, Crop::Full);

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
        // one picture of a generated statement is enough; the others are text
        if name == "gen-gti" {
            out.shot(name, &a, Crop::Area(32, 1, 88, 22));
        }
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
        ("gfr", crate::psql::Gen::FuncRow),
        ("gfs", crate::psql::Gen::FuncSet),
        ("gft", crate::psql::Gen::FuncTable),
        ("gfe", crate::psql::Gen::FuncExists),
        ("gfj", crate::psql::Gen::FuncJson),
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
    // gtg / gtj / gtr / gtau: sales.customer with its foreign keys
    {
        use crate::psql::{Gen, RelFk, Related};
        let fk =
            |outgoing: bool, name: &str, schema: &str, table: &str, this: &str, other: &str| {
                RelFk {
                    outgoing,
                    name: name.into(),
                    schema: schema.into(),
                    table: table.into(),
                    pairs: vec![(this.into(), other.into())],
                    nullable: true,
                }
            };
        let cols = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let rel = Related {
            fks: vec![
                fk(
                    true,
                    "FK_Customer_Person_PersonID",
                    "person",
                    "person",
                    "personid",
                    "businessentityid",
                ),
                fk(
                    true,
                    "FK_Customer_SalesTerritory_TerritoryID",
                    "sales",
                    "salesterritory",
                    "territoryid",
                    "territoryid",
                ),
                fk(
                    true,
                    "FK_Customer_Store_StoreID",
                    "sales",
                    "store",
                    "storeid",
                    "businessentityid",
                ),
                RelFk {
                    nullable: false,
                    ..fk(
                        false,
                        "FK_SalesOrderHeader_Customer_CustomerID",
                        "sales",
                        "salesorderheader",
                        "customerid",
                        "customerid",
                    )
                },
            ],
            columns: vec![
                (
                    ("person".into(), "person".into()),
                    cols(&["businessentityid", "firstname", "lastname"]),
                ),
                (
                    ("sales".into(), "salesterritory".into()),
                    cols(&["territoryid", "name", "countryregioncode"]),
                ),
                (
                    ("sales".into(), "store".into()),
                    cols(&["businessentityid", "name"]),
                ),
                (
                    ("sales".into(), "salesorderheader".into()),
                    cols(&["salesorderid", "orderdate", "customerid", "totaldue"]),
                ),
            ],
        };
        for (name, what) in [
            ("gtg", Gen::TestData { rows: 100 }),
            ("gtj", Gen::JoinParents),
            ("gtr", Gen::Referencing),
            ("gtau", Gen::Audit),
        ] {
            if let Some(t) = &out.txt {
                std::fs::write(
                    t.join(format!("gen-{name}.sql")),
                    crate::psql::generate_related(what, &gen_customer, &rel).unwrap(),
                )
                .unwrap();
            }
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

    // ---- developer template editor (D-074) ---------------------------------------
    // the manager
    {
        let (mut a, _d, _, _) = templates_scene();
        a.run(Command::TemplateManager);
        select_row(&mut a, "items", "Create lookup table");
        out.shot("tpe-manager", &a, Crop::Full);
    }

    // a new template: the empty tab, then its name
    {
        let (mut a, _d, _, _) = templates_scene();
        a.new_template_tab("Our team".into());
        a.sync_template_tab();
        out.shot("tpe-new", &a, Crop::Full);
    }

    // the template tab
    let (a, _d, _) = template_tab_scene();
    out.shot("tpe-tab", &a, Crop::Full);

    // the parameter dialog: a name, a choice from the database, fixed options, a list
    {
        let (mut a, _d, _) = template_tab_scene();
        select_param(&mut a, "name");
        key(&mut a, "Enter");
        out.shot("tpe-param", &a, Crop::Dialog);
        let (mut a, _d, _) = template_tab_scene();
        select_param(&mut a, "schema");
        key(&mut a, "Enter");
        out.shot("tpe-param-source", &a, Crop::Dialog);
        // fixed options: a new parameter
        let (mut a, _d, _) = template_tab_scene();
        select_param(&mut a, "with_audit");
        key(&mut a, "Insert");
        typing(&mut a, "key_type");
        key(&mut a, "Tab");
        typing(&mut a, "Type of the code");
        let choice = pgc_sql::devtemplate::ParamType::ALL
            .iter()
            .position(|t| *t == pgc_sql::devtemplate::ParamType::Choice)
            .unwrap();
        a.dialogs
            .last_mut()
            .unwrap()
            .set_combo_selected("type", choice);
        a.sync_param_dialog();
        let d = a.dialogs.last_mut().unwrap();
        d.set_text("default", "varchar");
        d.set_memo_text("options", "varchar\nchar\ninteger");
        d.focus_on("options");
        // a query of its own
        a.dialogs
            .last_mut()
            .unwrap()
            .set_combo_selected("values", 2);
        a.sync_param_dialog();
        let d = a.dialogs.last_mut().unwrap();
        d.set_text("name", "owner");
        d.set_text("label", "Owner");
        d.set_text("default", "");
        d.set_memo_text(
            "query",
            "SELECT rolname FROM pg_roles\nWHERE rolcanlogin ORDER BY 1",
        );
        d.focus_on("query");
        out.shot("tpe-param-query", &a, Crop::Dialog);
    }
    {
        // the list parameter of Create table, and one of its fields
        let (mut a, _d, _, file) = templates_scene();
        let table = file
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .join("Tables")
            .join("table.pgtemplate");
        let text = std::fs::read_to_string(&table).unwrap();
        a.open_template_file(table, &text);
        show_template_path(&mut a, &["default", "Tables", "table.pgtemplate"]);
        select_param(&mut a, "columns");
        key(&mut a, "Enter");
        a.dialogs.last_mut().unwrap().focus_on("fields");
        key(&mut a, "Down");
        out.shot("tpe-param-list", &a, Crop::Dialog);
        key(&mut a, "Alt+E");
        out.shot("tpe-field", &a, Crop::Dialog);
    }

    // the helper menu and what it inserts
    {
        let (mut a, _d, _) = template_tab_scene();
        template_layout(&mut a);
        a.run(Command::TemplateHelpers);
        out.shot("tpe-helpers", &a, Crop::Full);
        esc(&mut a);
        a.run(Command::TemplateInsertParam);
        out.shot("tpe-insert-param", &a, Crop::Dialog);
        esc(&mut a);
        a.run(Command::TemplateInsertConstruct);
        select_row(&mut a, "items", "for … endfor");
        out.shot("tpe-insert-construct", &a, Crop::Dialog);
        esc(&mut a);
        a.run(Command::TemplateInsertFilter);
        out.shot("tpe-insert-filter", &a, Crop::Dialog);
        esc(&mut a);
        // a construct around the selected line
        let text = a.editor().text();
        let from = text.find("    name text NOT NULL,").unwrap();
        let from = text[..from].chars().count();
        let to = from + "    name text NOT NULL,".chars().count();
        a.editor().set_cursor(from, false);
        a.editor().set_cursor(to, true);
        a.run(Command::TemplateInsertConstruct);
        select_row(&mut a, "items", "if … endif");
        key(&mut a, "Enter");
        typing(&mut a, "code_length > 0");
        a.sync_template_tab();
        out.shot("tpe-wrapped", &a, Crop::Area(32, 1, 88, 31));
    }
    {
        // Make parameter from selection
        let (mut a, _d, _) = template_tab_scene();
        let text = a.editor().text();
        let at = text.find("Lookup table").unwrap();
        let from = text[..at].chars().count();
        a.editor().set_cursor(from, false);
        a.editor()
            .set_cursor(from + "Lookup table".chars().count(), true);
        a.editor().scroll_to_cursor();
        a.run(Command::TemplateMakeParam);
        typing(&mut a, "comment");
        key(&mut a, "Tab");
        typing(&mut a, "Comment");
        key(&mut a, "Ctrl+Enter");
        a.sync_template_tab();
        a.editor().scroll_to_cursor();
        out.shot("tpe-made", &a, Crop::Full);
    }

    // autocomplete in the body: a parameter in the SQL, the fields of a list's
    // rows, the filters
    {
        let (mut a, _d, _) = template_tab_scene();
        a.editor().set_text("CREATE TABLE ");
        key(&mut a, "Ctrl+End");
        typing(&mut a, "sch");
        out.shot("tpe-ac-sql", &a, Crop::Area(32, 1, 88, 14));

        let (mut a, _d, _, file) = templates_scene();
        let table = file
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .join("Tables")
            .join("table.pgtemplate");
        let text = std::fs::read_to_string(&table).unwrap();
        a.open_template_file(table, &text);
        show_template_path(&mut a, &["default", "Tables", "table.pgtemplate"]);
        a.editor().set_text(
            "CREATE TABLE {{ schema|ident }}.{{ name|ident }} (\n{% for c in columns %}\n    ",
        );
        key(&mut a, "Ctrl+End");
        typing(&mut a, "{{ c.");
        out.shot("tpe-ac-fields", &a, Crop::Area(32, 1, 88, 16));
    }

    // the raw header
    {
        let (mut a, _d, _) = template_tab_scene();
        key(&mut a, "Ctrl+T");
        out.shot("tpe-raw", &a, Crop::Full);
    }

    // the checks: a Jinja error, an unknown name
    {
        let (mut a, _d, _) = template_tab_scene();
        let text = a.editor().text();
        a.editor()
            .set_text(&text.replace("{% endif %}", "{% endfi %}"));
        a.sync_template_tab();
        key(&mut a, "Ctrl+S");
        key(&mut a, "Enter");
        out.shot("tpe-error", &a, Crop::Full);
        let (mut a, _d, _) = template_tab_scene();
        let text = a.editor().text();
        a.editor()
            .set_text(&text.replace("{{ code_length }}", "{{ length }}"));
        a.sync_template_tab();
        out.shot("tpe-checks", &a, Crop::Area(32, 1, 88, 32));
    }

    // saving: a new template, a template of pgtc
    {
        let (mut a, _d, _, _) = templates_scene();
        a.new_template_tab("Our team".into());
        let t = a.template_mut().unwrap();
        t.name = "Create lookup table".into();
        t.touch();
        key(&mut a, "Ctrl+S");
        out.shot("tpe-save", &a, Crop::Dialog);

        let (mut a, _d, _, file) = templates_scene();
        let table = file
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .join("Tables")
            .join("table.pgtemplate");
        let text = std::fs::read_to_string(&table).unwrap();
        a.open_template_file(table, &text);
        typing(&mut a, "-- our rules\n");
        key(&mut a, "Ctrl+S");
        out.shot("tpe-builtin", &a, Crop::Dialog);
    }

    // Test: the template in its real form
    {
        use crate::template_dialog::{FieldValue, Stop};
        let (mut a, _d, _) = template_tab_scene();
        key(&mut a, "F5");
        let (name, audit) = (field_index(&a, "name"), field_index(&a, "with_audit"));
        set_text_field(&mut a, "name", "orderstatus");
        let form = a.templates.form.as_mut().unwrap();
        form.want("schema", "sales".into());
        form.set_choices("schema", Ok(text_choices(SCHEMAS)));
        form.fields[audit].value = FieldValue::Bool(true);
        form.focus = Stop::Field(name);
        form.refresh();
        out.shot("tpe-test", &a, Crop::Full);
    }

    // ---- database definition ----------------------------------------------------
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
    }
    for name in ["blocking", "progress"] {
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
    let (mut a, _d, _, _) = prof();
    profiler_records(&mut a);
    key(&mut a, "Ctrl+F");
    typing(&mut a, "reporting");
    out.shot("profiler-filter", &a, Crop::Dialog);
    // the execution plan of an entry (D-075)
    let (mut a, _d, _, _) = prof();
    profiler_records(&mut a);
    a.tabs[a.active_tab].profiler_mut().unwrap().grid.row = 2;
    key(&mut a, "Ctrl+Q");
    let conn = {
        let p = a.tabs[a.active_tab].profiler_mut().unwrap();
        let job = p.plan.as_mut().unwrap();
        job.handle.abort();
        job.handle = pgc_db::ConnectionHandle::detached(job.handle.id);
        job.handle.id
    };
    a.on_db(pgc_db::DbEvent::Plan {
        id: conn,
        key: 0,
        result: Ok(pgc_db::Plan {
            lines: [
                "HashAggregate  (cost=1189.93..1190.06 rows=10 width=64)",
                "  Group Key: t.name",
                "  ->  Hash Join  (cost=1.23..1032.60 rows=31465 width=40)",
                "        Hash Cond: (h.territoryid = t.territoryid)",
                "        ->  Seq Scan on salesorderheader h  (cost=0.00..845.65 rows=31465 width=12)",
                "        ->  Hash  (cost=1.10..1.10 rows=10 width=36)",
                "              ->  Seq Scan on salesterritory t  (cost=0.00..1.10 rows=10 width=36)",
            ]
            .map(str::to_owned)
            .to_vec(),
            generic: false,
        }),
    });
    out.shot("profiler-plan", &a, Crop::Dialog);

    // ---- database statistics ----------------------------------------------------
    let (mut a, _d, ids2) = scene();
    short_tree(&mut a, ids2);
    a.focus = Focus::Tree;
    a.settings.ui.tree_width = Some(26);
    a.run(Command::DbStats);
    {
        let s = a.tabs[a.active_tab].db_stats_mut().unwrap();
        let c = s.own.conn.as_mut().unwrap();
        c.handle.abort();
        c.handle = pgc_db::ConnectionHandle::detached(c.id);
        c.connected = true;
        s.own.version = Some(pgc_core::ServerVersion(170002));
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

    // ---- AI assistant (D-073) -----------------------------------------------------
    let (mut a, _d, _) = query_scene();
    // a saved key: shown as stars
    let file = a.paths.ai_key_file();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "sk-ant-example").unwrap();
    // ... of the saved provider (the stars are only there with its URL)
    a.settings.ai.base_url = "https://api.anthropic.com".into();
    a.run(Command::AiSettings);
    a.dialogs
        .last_mut()
        .unwrap()
        .set_combo_selected("preset", 1);
    a.update(Msg::Tick, &Hits::default());
    // the provider's models, as read from its list
    a.on_ai_models(crate::app::AiModels {
        url: "https://api.anthropic.com".into(),
        result: Ok(ai_models()),
    });
    let d = a.dialogs.last_mut().unwrap();
    if let Some(i) = d.widgets.iter().position(|w| w.id == "enabled") {
        d.focus = i;
    }
    key(&mut a, "Space");
    out.shot("ai-settings", &a, Crop::Dialog);

    let (mut a, _d, ids) = scene();
    short_tree(&mut a, ids);
    query_tab(&mut a, ids, AI_EDITOR);
    let end = AI_EDITOR.chars().count();
    a.editor().set_cursor(end, false);
    a.settings.ai = pgc_core::settings::AiSettings {
        enabled: true,
        protocol: pgc_core::settings::AiProtocol::Anthropic,
        base_url: "https://api.anthropic.com".into(),
        model: "claude-sonnet-5-5".into(),
        effort: Default::default(),
        models: ai_models(),
    };
    key(&mut a, "F8");
    let lookup = *a
        .template_lookups
        .iter()
        .find(|(_, t)| matches!(t, LookupTarget::AiStructure { .. }))
        .unwrap()
        .0;
    a.lookup_loaded(lookup, Ok(ai_structure()));
    typing(&mut a, AI_PROMPT);
    a.update(Msg::Tick, &Hits::default());
    out.shot("ai-dialog", &a, Crop::Full);
    // autocomplete after `sales.`
    a.dialogs
        .last_mut()
        .unwrap()
        .set_memo_text("prompt", "Total due per customer for 2014, from sales");
    typing(&mut a, ".");
    out.shot("ai-complete", &a, Crop::Full);
    key(&mut a, "Esc");
    a.dialogs
        .last_mut()
        .unwrap()
        .set_memo_text("prompt", AI_PROMPT);
    key(&mut a, "Alt+O");
    a.dialogs.last_mut().unwrap().set_combo_selected("scope", 0);
    key(&mut a, "Alt+S");
    let id = a.ai.run.as_ref().unwrap().id;
    // the steps of the request in Messages (texts of a real run)
    for step in [
        crate::ai::Progress::Thinking,
        crate::ai::Progress::ThinkingText(
            "The person's name comes from person.person through customer.personid,              which may be NULL, so a LEFT JOIN keeps customers without a person."
                .into(),
        ),
        crate::ai::Progress::ThinkingText(
            "Filtering orderdate to 2014 with a half-open range, then summing totaldue              per customer and keeping the ten largest."
                .into(),
        ),
        crate::ai::Progress::Writing,
    ] {
        a.on_ai_progress(crate::app::AiProgress { id, step });
    }
    {
        let q = a.query_mut().unwrap();
        q.show_messages = true;
        q.results_visible = true;
    }
    out.shot("ai-progress", &a, Crop::Full);
    a.on_ai_progress(crate::app::AiProgress {
        id,
        step: crate::ai::Progress::Usage {
            sent: 1140,
            cached: 0,
            received: 212,
        },
    });
    a.on_ai_event(crate::app::AiEvent {
        id,
        result: Ok(AI_ANSWER.into()),
    });
    out.shot("ai-answer", &a, Crop::Full);

    // ---- settings and colour schemes --------------------------------------------
    let (mut a, _d, _) = query_scene();
    a.settings.clipboard.system = true;
    a.run(Command::Settings);
    out.shot("settings", &a, Crop::Dialog);

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
        // Classic is the scheme of every other picture
        if file != "classic" {
            out.shot(&format!("scheme-{file}"), &a, Crop::TopLeft(120, 24));
        }
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

/// The editor text of the AI pictures (D-073).
const AI_EDITOR: &str = "-- reports for the sales team\nSELECT count(*) FROM sales.customer;\n\n";

const AI_TRIGGER_PROMPT: &str =
    "generate before insert row level trigger for humanresources.employee";

const AI_PROMPT: &str = "Total due per customer for 2014, the ten largest, with the person's name";

/// A real answer of Claude (claude-sonnet-5-5) to [`AI_PROMPT`] with this sample,
/// taken with `ai_live_sample` on 2026-11-18.
const AI_ANSWER: &str = "-- Ten largest customers by total due in 2014, with the person's name.
-- LEFT JOIN keeps customers without a person (personid is nullable); their name will be NULL.
SELECT c.customerid,
       p.firstname,
       p.lastname,
       sum(soh.totaldue) AS total_due_2014
FROM sales.salesorderheader AS soh
JOIN sales.customer AS c ON c.customerid = soh.customerid
LEFT JOIN person.person AS p ON p.businessentityid = c.personid
WHERE soh.orderdate >= timestamp '2014-01-01'
  AND soh.orderdate <  timestamp '2015-01-01'
GROUP BY c.customerid, p.firstname, p.lastname
ORDER BY total_due_2014 DESC, c.customerid
LIMIT 10;
";

/// A part of AdventureWorks as `sql/ai/default/structure.sql` reads it.
fn ai_structure() -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<&str>> = vec![
        vec!["r", "sales", "customer", "r"],
        vec!["r", "sales", "salesorderheader", "r"],
        vec!["r", "person", "person", "r"],
        vec![
            "c",
            "sales",
            "customer",
            "1",
            "customerid",
            "integer",
            "true",
            "",
        ],
        vec![
            "c", "sales", "customer", "2", "personid", "integer", "false", "",
        ],
        vec![
            "c",
            "sales",
            "customer",
            "3",
            "territoryid",
            "integer",
            "false",
            "",
        ],
        vec![
            "c",
            "sales",
            "salesorderheader",
            "1",
            "salesorderid",
            "integer",
            "true",
            "",
        ],
        vec![
            "c",
            "sales",
            "salesorderheader",
            "2",
            "orderdate",
            "timestamp without time zone",
            "true",
            "DEFAULT now()",
        ],
        vec![
            "c",
            "sales",
            "salesorderheader",
            "3",
            "customerid",
            "integer",
            "true",
            "",
        ],
        vec![
            "c",
            "sales",
            "salesorderheader",
            "4",
            "totaldue",
            "numeric",
            "true",
            "",
        ],
        vec![
            "c",
            "person",
            "person",
            "1",
            "businessentityid",
            "integer",
            "true",
            "",
        ],
        vec![
            "c",
            "person",
            "person",
            "2",
            "firstname",
            "text",
            "true",
            "",
        ],
        vec!["c", "person", "person", "3", "lastname", "text", "true", ""],
        vec![
            "k",
            "sales",
            "customer",
            "pk_customer",
            "p",
            "PRIMARY KEY (customerid)",
        ],
        vec![
            "k",
            "sales",
            "customer",
            "fk_person",
            "f",
            "FOREIGN KEY (personid) REFERENCES person.person(businessentityid)",
        ],
        vec![
            "k",
            "sales",
            "salesorderheader",
            "pk_soh",
            "p",
            "PRIMARY KEY (salesorderid)",
        ],
        vec![
            "k",
            "sales",
            "salesorderheader",
            "fk_customer",
            "f",
            "FOREIGN KEY (customerid) REFERENCES sales.customer(customerid)",
        ],
        vec![
            "k",
            "person",
            "person",
            "pk_person",
            "p",
            "PRIMARY KEY (businessentityid)",
        ],
    ];
    // humanresources.employee, as in AdventureWorks
    rows.push(vec!["r", "humanresources", "employee", "r"]);
    for c in [
        ["1", "businessentityid", "integer", "true", ""],
        ["2", "nationalidnumber", "character varying(15)", "true", ""],
        ["3", "loginid", "character varying(256)", "true", ""],
        ["4", "jobtitle", "character varying(50)", "true", ""],
        ["5", "birthdate", "date", "true", ""],
        ["6", "maritalstatus", "character(1)", "true", ""],
        ["7", "gender", "character(1)", "true", ""],
        ["8", "hiredate", "date", "true", ""],
        ["9", "salariedflag", "boolean", "true", "DEFAULT true"],
        ["10", "vacationhours", "smallint", "true", "DEFAULT 0"],
        ["11", "sickleavehours", "smallint", "true", "DEFAULT 0"],
        ["12", "currentflag", "boolean", "true", "DEFAULT true"],
        [
            "13",
            "rowguid",
            "uuid",
            "true",
            "DEFAULT uuid_generate_v1()",
        ],
        [
            "14",
            "modifieddate",
            "timestamp without time zone",
            "true",
            "DEFAULT now()",
        ],
        [
            "15",
            "organizationnode",
            "character varying",
            "false",
            "DEFAULT '/'::character varying",
        ],
    ] {
        rows.push(vec![
            "c",
            "humanresources",
            "employee",
            c[0],
            c[1],
            c[2],
            c[3],
            c[4],
        ]);
    }
    for (name, kind, def) in [
        ("pk_employee", "p", "PRIMARY KEY (businessentityid)"),
        (
            "fk_employee_person",
            "f",
            "FOREIGN KEY (businessentityid) REFERENCES person.person(businessentityid)",
        ),
        (
            "ck_employee_birthdate",
            "c",
            "CHECK (birthdate >= '1930-01-01'::date AND birthdate <= (now() - '18 years'::interval))",
        ),
        (
            "ck_employee_gender",
            "c",
            "CHECK (upper(gender::text) = ANY (ARRAY['M'::text, 'F'::text]))",
        ),
        (
            "ck_employee_hiredate",
            "c",
            "CHECK (hiredate >= '1996-07-01'::date AND hiredate <= (now() + '1 day'::interval))",
        ),
        (
            "ck_employee_maritalstatus",
            "c",
            "CHECK (upper(maritalstatus::text) = ANY (ARRAY['M'::text, 'S'::text]))",
        ),
        (
            "ck_employee_sickleavehours",
            "c",
            "CHECK (sickleavehours >= 0 AND sickleavehours <= 120)",
        ),
        (
            "ck_employee_vacationhours",
            "c",
            "CHECK (vacationhours >= '-40'::integer AND vacationhours <= 240)",
        ),
    ] {
        rows.push(vec!["k", "humanresources", "employee", name, kind, def]);
    }
    for r in &mut rows {
        r.resize(8, "");
    }
    rows.into_iter()
        .map(|r| r.into_iter().map(str::to_owned).collect())
        .collect()
}

/// One real AI request with the help's sample (D-073): the answer for the pictures
/// comes from a real model. Runs only when `PGTC_AI_LIVE` is set; uses the user's AI
/// settings and key file (the key is never printed).
#[tokio::test]
#[ignore]
async fn ai_live_sample() {
    if std::env::var("PGTC_AI_LIVE").is_err() {
        return;
    }
    let paths = pgc_core::paths::Paths::from_os().unwrap();
    let settings = pgc_core::settings::Settings::load(&paths.settings_file()).unwrap();
    let key = crate::ai::api_key(&paths.ai_key_file());
    let mut settings = settings;
    // PGTC_AI_LIVE=models: the provider's list of models, then a request with a
    // chosen model and effort
    if std::env::var("PGTC_AI_LIVE").as_deref() == Ok("models") {
        let request = crate::ai::models_request(&settings.ai, key.as_deref());
        let list = crate::ai::fetch_models(request).await.unwrap();
        for m in &list {
            println!("MODEL {} = {}", m.id, m.name);
        }
        settings.ai.model = "claude-haiku-5-5".into();
        settings.ai.effort = pgc_core::settings::AiEffort::Low;
    }
    // PGTC_AI_LIVE=trigger: the second sample (an empty editor)
    let (prompt, editor) = match std::env::var("PGTC_AI_LIVE").as_deref() {
        Ok("trigger") => (AI_TRIGGER_PROMPT, ""),
        _ => (AI_PROMPT, AI_EDITOR),
    };
    let ask = crate::ai::Ask {
        server: "PostgreSQL 17.2".into(),
        database: DB.into(),
        structure: crate::ai::Structure::from_rows(ai_structure()).text(None),
        editor: crate::ai::editor_text(
            editor,
            editor.chars().count(),
            None,
            crate::ai::EDITOR_LIMIT,
        ),
        prompt: prompt.into(),
    };
    let request = crate::ai::request(&settings.ai, key.as_deref(), &ask);
    let started = std::time::Instant::now();
    let steps = move |step| println!("STEP {:>5} ms {step:?}", started.elapsed().as_millis());
    match crate::ai::send(request, steps).await {
        Ok(answer) => println!("=====ANSWER=====\n{answer}\n=====END====="),
        Err(e) => panic!("{e}"),
    }
}

/// Models as Anthropic's list names them (a part of the list of 2026-11-18).
fn ai_models() -> Vec<pgc_core::settings::AiModel> {
    [
        ("claude-fable-5-2", "Claude Fable 5.2"),
        ("claude-opus-5-5", "Claude Opus 5.5"),
        ("claude-sonnet-5-5", "Claude Sonnet 5.5"),
        ("claude-haiku-5-5", "Claude Haiku 5.5"),
        ("claude-opus-5", "Claude Opus 5"),
        ("claude-sonnet-5", "Claude Sonnet 5"),
    ]
    .iter()
    .map(|(id, name)| pgc_core::settings::AiModel {
        id: (*id).to_owned(),
        name: (*name).to_owned(),
    })
    .collect()
}
