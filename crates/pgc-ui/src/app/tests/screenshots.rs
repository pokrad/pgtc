use super::*;

use pgc_core::settings::SavedConnection;
use pgc_db::TxStatus;
use ratatui::layout::Rect;

use crate::keymap::Command;
use crate::query_tab::{LineMap, Sent};
use crate::theme::ThemeSet;
use crate::view::Hits;

/// Writes HTML previews of two real screens per built-in colour scheme to the file
/// named by `PGTC_PREVIEW_OUT` (Windows Terminal "Campbell" palette). Not part of
/// the normal test run.
#[tokio::test]
#[ignore]
async fn scheme_preview_html() {
    let Ok(out) = std::env::var("PGTC_PREVIEW_OUT") else {
        return;
    };
    let palette = &crate::theme_check::PALETTES[0];
    let mut html = String::new();
    for name in ThemeSet::built_in().names() {
        let mut screens = Vec::new();
        for screen in 0..2 {
            let (mut a, _d) = app();
            a.settings.ui.color_scheme = name.clone();
            a.theme = a.themes.get(&name).clone();
            // tree: a connected server with databases, a second one disconnected
            let uid = add_server(&mut a, "production");
            add_server(&mut a, "staging");
            fake_connecting(&mut a, uid);
            a.server_connected(uid, pgc_core::ServerVersion(170002));
            let dbs = a.tree.servers[0].children[0].nid;
            let row = |oid, name: &str| pgc_db::NodeRow {
                oid,
                name: name.into(),
                label: None,
                partitioned: false,
            };
            a.tree
                .set_children(dbs, vec![row(1, "billing"), row(2, "postgres")]);
            a.tree.node_mut(dbs).unwrap().loading = false;
            // query tab with SQL and a paused result
            a.new_query_tab(None);
            typing(&mut a, "-- revenue per customer, October");
            for line in [
                "SELECT c.name, sum(o.total) AS revenue, max(o.note) AS note",
                "FROM orders o JOIN customers c ON c.id = o.customer_id",
                "WHERE o.created_at >= '2026-10-01' AND o.status <> 'void'",
                "GROUP BY c.name ORDER BY 2 DESC LIMIT 50;",
            ] {
                key(&mut a, "Enter");
                typing(&mut a, line);
            }
            let q = a.query_mut().unwrap();
            q.start_run(
                1,
                Sent {
                    offset: 0,
                    plpgsql_lines: None,
                },
            );
            let none = |_: usize| 0;
            let lines = LineMap {
                line_of: &none,
                line_start: &none,
            };
            let col = |n: &str, t| pgc_db::Column {
                name: n.into(),
                type_oid: t,
            };
            q.apply(
                pgc_db::QueryEvent::ResultStart {
                    columns: vec![col("name", 25), col("revenue", 1700), col("note", 25)],
                    title: None,
                },
                &lines,
            );
            let r = |n: &str, v: &str, note: Option<&str>| {
                vec![Some(n.into()), Some(v.into()), note.map(Into::into)]
            };
            q.apply(
                pgc_db::QueryEvent::Rows {
                    rows: vec![
                        r(
                            "Acme d.o.o.",
                            "48210.50",
                            Some(
                                "priority
client",
                            ),
                        ),
                        r("Northwind", "31877.00", None),
                        r("Čakovec Tools", "12904.75", Some("new")),
                        r("Globex", "9120.00", None),
                    ],
                    more: true,
                },
                &lines,
            );
            if screen == 0 {
                a.focus = Focus::Results;
                if let Some(rs) = a.active_result_set() {
                    rs.move_cursor(1, 1, 0);
                }
                key(&mut a, "F9"); // File menu
                key(&mut a, "Down");
                key(&mut a, "Down");
            } else {
                a.focus = Focus::Tree;
                a.tree.cursor = 2;
                a.run(Command::Settings);
                key(&mut a, "Alt+S");
                key(&mut a, "Space"); // open the colour scheme list
            }
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
            let mut hits = Hits::default();
            terminal
                .draw(|f| crate::view::render(f, &a, &mut hits))
                .unwrap();
            let buf = terminal.backend().buffer();
            let mut pre = String::new();
            for y in 0..30 {
                let mut run_style = String::new();
                let mut run = String::new();
                for x in 0..110 {
                    let cell = &buf[(x, y)];
                    let (fg, bg) = crate::theme_check::shown(cell.style(), palette);
                    let m = cell.modifier;
                    let css = format!(
                        "color:#{:02x}{:02x}{:02x};background:#{:02x}{:02x}{:02x}{}{}{}",
                        fg.0,
                        fg.1,
                        fg.2,
                        bg.0,
                        bg.1,
                        bg.2,
                        if m.contains(ratatui::style::Modifier::BOLD) {
                            ";font-weight:700"
                        } else {
                            ""
                        },
                        if m.contains(ratatui::style::Modifier::ITALIC) {
                            ";font-style:italic"
                        } else {
                            ""
                        },
                        if m.contains(ratatui::style::Modifier::UNDERLINED) {
                            ";text-decoration:underline"
                        } else {
                            ""
                        },
                    );
                    if css != run_style && !run.is_empty() {
                        pre.push_str(&format!("<span style=\"{run_style}\">{run}</span>"));
                        run.clear();
                    }
                    run_style = css;
                    let sym = cell.symbol();
                    let sym = if sym.is_empty() { " " } else { sym };
                    run.push_str(
                        &sym.replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;"),
                    );
                }
                pre.push_str(&format!(
                    "<span style=\"{run_style}\">{run}</span>
"
                ));
            }
            screens.push(pre);
        }
        html.push_str(&format!(
            "<!--scheme {name}-->
{}
<!--screen-->
{}
<!--end-->
",
            screens[0], screens[1]
        ));
    }
    std::fs::write(out, html).unwrap();
}

/// Writes the README screenshots (SVG, Dracula scheme) into the folder named by
/// `PGTC_SCREENSHOT_DIR`: real screens drawn by pgtc's renderer with made-up data.
/// Not part of the normal test run.
#[tokio::test]
#[ignore]
async fn readme_screenshots() {
    let Ok(out) = std::env::var("PGTC_SCREENSHOT_DIR") else {
        return;
    };
    const W: u16 = 120;
    const H: u16 = 34;
    // a connected server with databases, a query tab with SQL and a result
    let scene = || {
        let (mut a, d) = app();
        a.settings.ui.color_scheme = "Dracula".into();
        a.theme = a.themes.get("Dracula").clone();
        a.screen = Rect::new(0, 0, W, H);
        let uid = a.tree.add(SavedConnection {
            name: "production".into(),
            username: "app".into(),
            ..SavedConnection::default()
        });
        add_server(&mut a, "staging");
        fake_connecting(&mut a, uid);
        a.server_connected(uid, pgc_core::ServerVersion(170002));
        let dbs = a.tree.servers[0].children[0].nid;
        let row = |oid, name: &str| pgc_db::NodeRow {
            oid,
            name: name.into(),
            label: None,
            partitioned: false,
        };
        a.tree.set_children(
            dbs,
            vec![row(1, "billing"), row(2, "postgres"), row(3, "shop")],
        );
        a.tree.node_mut(dbs).unwrap().loading = false;
        a.new_query_tab(None);
        // a query tab connected to "shop" (no real connection behind it)
        let q = a.query_mut().unwrap();
        q.binding = Some((uid, "shop".into()));
        q.conn = Some(crate::query_tab::QueryConn {
            id: 99,
            database: "shop".into(),
            handle: pgc_db::ConnectionHandle::detached(99),
            cancel: None,
            state: crate::query_tab::QueryConnState::Connected,
            ssl_mode: pgc_core::settings::SslMode::default(),
            ssl_root_cert: None,
        });
        typing(&mut a, "-- revenue per customer, October");
        for line in [
            "SELECT c.name, sum(o.total) AS revenue, count(*) AS orders",
            "FROM orders o",
            "JOIN customers c ON c.id = o.customer_id",
            "WHERE o.created_at >= '2026-10-01' AND o.status <> 'void'",
            "GROUP BY c.name",
            "ORDER BY revenue DESC;",
        ] {
            key(&mut a, "Enter");
            typing(&mut a, line);
        }
        let q = a.query_mut().unwrap();
        q.start_run(
            1,
            Sent {
                offset: 0,
                plpgsql_lines: None,
            },
        );
        let none = |_: usize| 0;
        let lines = LineMap {
            line_of: &none,
            line_start: &none,
        };
        let col = |n: &str, t| pgc_db::Column {
            name: n.into(),
            type_oid: t,
        };
        q.apply(
            pgc_db::QueryEvent::ResultStart {
                columns: vec![col("name", 25), col("revenue", 1700), col("orders", 20)],
                title: None,
            },
            &lines,
        );
        let r = |n: &str, v: &str, c: &str| vec![Some(n.into()), Some(v.into()), Some(c.into())];
        q.apply(
            pgc_db::QueryEvent::Rows {
                rows: vec![
                    r("Acme d.o.o.", "48210.50", "112"),
                    r("Northwind Traders", "31877.00", "87"),
                    r("Čakovec Tools", "12904.75", "41"),
                    r("Globex", "9120.00", "30"),
                    r("Initech", "4410.20", "12"),
                ],
                more: false,
            },
            &lines,
        );
        q.apply(
            pgc_db::QueryEvent::Command {
                tag: "SELECT 5".into(),
            },
            &lines,
        );
        q.apply(
            pgc_db::QueryEvent::Finished {
                elapsed: std::time::Duration::from_millis(38),
                status: TxStatus::Idle,
            },
            &lines,
        );
        a.tree.cursor = 2;
        a.focus = Focus::Tabs;
        (a, d)
    };
    let svg = |a: &App| -> String {
        let palette = &crate::theme_check::PALETTES[0];
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
        let mut hits = Hits::default();
        terminal
            .draw(|f| crate::view::render(f, a, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        let (cw, ch) = (9.0_f32, 19.0_f32);
        let hex = |c: (u8, u8, u8)| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" \
                 viewBox=\"0 0 {} {}\" font-family=\"Cascadia Mono, Consolas, 'DejaVu Sans Mono', Menlo, monospace\" \
                 font-size=\"15\" shape-rendering=\"crispEdges\">\n",
            W as f32 * cw,
            H as f32 * ch,
            W as f32 * cw,
            H as f32 * ch
        );
        // crispEdges: shown smaller (README grid) the row backgrounds are not
        // anti-aliased, so no page colour shows between rows; the window colour
        // under everything covers renderers that ignore it
        let (_, window) =
            crate::theme_check::shown(a.theme.get(crate::theme::Role::CoreNormal), palette);
        out.push_str(&format!(
            "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
            W as f32 * cw,
            H as f32 * ch,
            hex(window)
        ));
        for y in 0..H {
            // backgrounds as runs of one colour
            let mut x = 0;
            while x < W {
                let (_, bg) = crate::theme_check::shown(buf[(x, y)].style(), palette);
                let start = x;
                while x < W && crate::theme_check::shown(buf[(x, y)].style(), palette).1 == bg {
                    x += 1;
                }
                out.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
                    start as f32 * cw,
                    y as f32 * ch,
                    (x - start) as f32 * cw + 0.5,
                    // overlap the next row: no hairline gaps when scaled
                    ch + 0.5,
                    hex(bg)
                ));
            }
            // every character at its own column, so the grid stays exact
            for x in 0..W {
                let cell = &buf[(x, y)];
                let sym = cell.symbol();
                if sym.trim().is_empty() {
                    continue;
                }
                let (fg, _) = crate::theme_check::shown(cell.style(), palette);
                let m = cell.modifier;
                let bold = if m.contains(ratatui::style::Modifier::BOLD) {
                    " font-weight=\"700\""
                } else {
                    ""
                };
                let under = if m.contains(ratatui::style::Modifier::UNDERLINED) {
                    " text-decoration=\"underline\""
                } else {
                    ""
                };
                let sym = sym
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                out.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\" fill=\"{}\"{bold}{under}>{sym}</text>\n",
                    x as f32 * cw,
                    y as f32 * ch + ch * 0.75,
                    hex(fg)
                ));
            }
        }
        out.push_str("</svg>\n");
        out
    };
    let dir = std::path::Path::new(&out);
    std::fs::create_dir_all(dir).unwrap();
    // 1. query tab: editor and result grid
    let (a, _d) = scene();
    std::fs::write(dir.join("query.svg"), svg(&a)).unwrap();
    // 2. autocomplete after "c."
    let (mut a, _d) = scene();
    a.editor().set_text("SELECT c. FROM customers c");
    a.editor().set_cursor(9, false);
    a.query_mut().unwrap().names.columns.insert(
        (None, "customers".into()),
        vec![
            "id".into(),
            "name".into(),
            "email".into(),
            "country".into(),
            "created_at".into(),
        ],
    );
    a.query_mut().unwrap().names.objects = Some(Vec::new());
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
        auto: false,
    };
    popup.set_items(built.items, false);
    a.completion = Some(popup);
    std::fs::write(dir.join("autocomplete.svg"), svg(&a)).unwrap();
    // 3. the psql command window (F4)
    let (mut a, _d) = scene();
    let d = a.psql_dialog("");
    a.dialogs.push(d);
    key(&mut a, "Down");
    key(&mut a, "Down");
    key(&mut a, "Down");
    std::fs::write(dir.join("psql.svg"), svg(&a)).unwrap();
    // 4. the Query menu
    let (mut a, _d) = scene();
    key(&mut a, "F9");
    key(&mut a, "Right");
    key(&mut a, "Right");
    std::fs::write(dir.join("menu.svg"), svg(&a)).unwrap();
    // 5. developer templates (F7): the picker on "Create table"
    let picker = || {
        let (mut a, d) = scene();
        a.run(Command::InstallTemplates);
        a.dialogs.clear();
        a.run(Command::NewFromTemplate);
        for _ in 0..60 {
            let desc = a
                .dialogs
                .last()
                .and_then(|d| d.widgets.iter().find(|w| w.id == "desc"))
                .map(|w| w.label.text.clone())
                .unwrap_or_default();
            if desc.starts_with("CREATE TABLE") {
                break;
            }
            key(&mut a, "Down");
        }
        (a, d)
    };
    let (a, _d) = picker();
    std::fs::write(dir.join("templates.svg"), svg(&a)).unwrap();
    // 6. the Create table form, filled in, with the live preview
    let (mut a, _d) = picker();
    key(&mut a, "Enter");
    {
        use crate::template_dialog::{Choice, FieldValue, Stop};
        use pgc_sql::devtemplate::ParamValue;
        let form = a.template.as_mut().expect("Create table form");
        let text = |s: &str| ParamValue::Text(s.into());
        let schemas = ["public", "sales", "audit"]
            .iter()
            .map(|s| Choice {
                label: (*s).into(),
                value: text(s),
            })
            .collect();
        form.set_choices("schema", Ok(schemas));
        let index = |name: &str| {
            form.fields
                .iter()
                .position(|f| f.param.name == name)
                .unwrap()
        };
        let (name, columns) = (index("name"), index("columns"));
        form.fields[name].value = FieldValue::Text {
            text: "order_items".into(),
            cursor: 11,
        };
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
            col("id", "bigint", false, true, ""),
            col("order_id", "bigint", true, false, ""),
            col("quantity", "integer", true, false, "1"),
            col("price", "numeric(10,2)", true, false, ""),
        ];
        form.list_select(columns, 0);
        form.focus = Stop::Field(columns);
        form.refresh();
    }
    std::fs::write(dir.join("template.svg"), svg(&a)).unwrap();
    // 7. Find object (Ctrl+F) on the database "shop", columns included
    let (mut a, _d) = scene();
    {
        use crate::objects::ObjType;
        use crate::tree::{DbConn, RowKind};
        let shop = a.tree.servers[0].children[0]
            .children
            .as_ref()
            .unwrap()
            .iter()
            .find(|n| n.name == "shop")
            .unwrap()
            .nid;
        let groups = a.tree.groups(&ObjType::Database.children(false));
        let n = a.tree.node_mut(shop).unwrap();
        n.conn = Some(DbConn {
            state: crate::tree::ConnState::Connected,
            handle: Some(pgc_db::ConnectionHandle::detached(51)),
        });
        n.children = Some(groups);
        a.tree.select_kind(RowKind::Node(shop));
    }
    a.focus = Focus::Tree;
    key(&mut a, "Ctrl+F");
    typing(&mut a, "cust");
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
                "public",
                "customers",
                "schemas:2200/tables:101",
                "2",
            ]),
            r([
                "table",
                "sales",
                "customer_notes",
                "schemas:2300/tables:301",
                "2",
            ]),
            r([
                "view",
                "sales",
                "customer_revenue",
                "schemas:2300/views:310",
                "6",
            ]),
            r([
                "sequence",
                "public",
                "customers_id_seq",
                "schemas:2200/sequences:102",
                "12",
            ]),
            r([
                "function",
                "sales",
                "customer_balance(integer)",
                "schemas:2300/functions:320",
                "13",
            ]),
            r([
                "index",
                "public",
                "customers_email_key",
                "schemas:2200/tables:101/indexes:103",
                "20",
            ]),
            r([
                "constraint",
                "public",
                "orders_customer_id_fkey",
                "schemas:2200/tables:100/constraints:104",
                "21",
            ]),
            r([
                "column",
                "public",
                "orders.customer_id",
                "schemas:2200/tables:100/columns:2",
                "90",
            ]),
            r([
                "column",
                "sales",
                "invoices.customer_id",
                "schemas:2300/tables:302/columns:3",
                "90",
            ]),
        ]),
    );
    a.dialogs[0].list_select("list", 2);
    std::fs::write(dir.join("find.svg"), svg(&a)).unwrap();
}
