//! Autocomplete in query tabs (D-052): the name cache of a tab, the list of entries
//! for an [`Analysis`] and the popup at the cursor.

use std::collections::{HashMap, HashSet};

use pgc_core::quote::quote_ident;
use pgc_editor::complete::{Analysis, Want};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::theme::{Role, Theme};

/// Rows shown at most.
pub const LIST_ROWS: usize = 10;
/// Letters of a name after which the list opens by itself.
pub const AUTO_LETTERS: usize = 3;

/// A schema, relation, function or type of the tab's database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obj {
    /// `None` for schemas.
    pub schema: Option<String>,
    pub name: String,
    pub kind: String,
    /// On the search path (usable without a schema).
    pub visible: bool,
}

impl Obj {
    fn is_relation(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "table" | "view" | "materialized view" | "foreign table"
        )
    }

    fn is_function(&self) -> bool {
        matches!(self.kind.as_str(), "function" | "procedure" | "aggregate")
    }
}

/// (schema, table) of a column list; `None` schema = the table on the search path.
pub type TableKey = (Option<String>, String);

/// Names read through a query tab's connection (D-052).
#[derive(Debug, Clone, Default)]
pub struct Names {
    pub objects: Option<Vec<Obj>>,
    pub objects_pending: bool,
    pub columns: HashMap<TableKey, Vec<String>>,
    pub columns_pending: HashSet<TableKey>,
    /// Raised when the cache is dropped, so that answers to older requests are ignored.
    pub generation: u64,
}

impl Names {
    /// Drops everything (after DDL, on tree refresh).
    pub fn clear(&mut self) {
        *self = Self {
            generation: self.generation + 1,
            ..Self::default()
        };
    }

    /// `objects.sql` rows: schema, name, kind, visible.
    pub fn set_objects(&mut self, rows: Vec<Vec<String>>) {
        self.objects_pending = false;
        self.objects = Some(
            rows.into_iter()
                .filter_map(|r| {
                    let [schema, name, kind, visible]: [String; 4] = r.try_into().ok()?;
                    Some(Obj {
                        schema: (!schema.is_empty()).then_some(schema),
                        name,
                        kind,
                        visible: visible == "t" || visible == "true",
                    })
                })
                .collect(),
        );
    }
}

/// Something the list needs from the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    Objects,
    Columns(TableKey),
}

/// One entry of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub kind: &'static str,
    /// Text that replaces the typed prefix.
    pub insert: String,
}

/// Entries for `a`, what is still missing, and whether the list waits for it.
#[derive(Debug, Default)]
pub struct Built {
    pub items: Vec<Item>,
    pub needs: Vec<Need>,
    pub loading: bool,
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "table" => "table",
        "view" => "view",
        "materialized view" => "mat. view",
        "foreign table" => "foreign table",
        "sequence" => "sequence",
        "function" => "function",
        "procedure" => "procedure",
        "aggregate" => "aggregate",
        "schema" => "schema",
        "type" => "type",
        _ => "",
    }
}

/// Order of the groups: columns, relations, functions, schemas, types, keywords.
fn rank(kind: &str) -> u8 {
    match kind {
        "column" => 0,
        "table" | "view" | "mat. view" | "foreign table" | "sequence" => 1,
        "function" | "procedure" | "aggregate" => 2,
        "schema" => 3,
        "type" => 4,
        _ => 5,
    }
}

/// The list for analysis `a` with the names known so far.
pub fn build(a: &Analysis, names: &Names, keywords: &[&str]) -> Built {
    let mut b = Built::default();
    let prefix = a.prefix.to_lowercase();
    let fits = |name: &str| name.to_lowercase().starts_with(&prefix);
    let mut out: Vec<Item> = Vec::new();
    let push_name = |out: &mut Vec<Item>, name: &str, kind: &'static str| {
        if fits(name) {
            out.push(Item {
                label: name.to_owned(),
                kind,
                insert: quote_ident(name),
            });
        }
    };
    let objects = names.objects.as_deref();
    if objects.is_none() {
        b.needs.push(Need::Objects);
        b.loading = true;
    }
    let objects = objects.unwrap_or(&[]);
    let columns_of =
        |b: &mut Built, out: &mut Vec<Item>, key: TableKey| match names.columns.get(&key) {
            Some(cols) => {
                for c in cols {
                    push_name(out, c, "column");
                }
            }
            None => {
                b.loading = true;
                b.needs.push(Need::Columns(key));
            }
        };
    let relations_and_schemas =
        |out: &mut Vec<Item>, push: &mut dyn FnMut(&mut Vec<Item>, &Obj)| {
            for o in objects {
                if o.kind == "schema" || (o.visible && o.is_relation()) {
                    push(out, o);
                }
            }
        };
    let mut push_obj = |out: &mut Vec<Item>, o: &Obj| {
        if fits(&o.name) {
            out.push(Item {
                label: o.name.clone(),
                kind: kind_label(&o.kind),
                insert: quote_ident(&o.name),
            });
        }
    };
    match &a.want {
        Want::Relations => relations_and_schemas(&mut out, &mut push_obj),
        Want::Qualified(q) => {
            let table = a
                .tables
                .iter()
                .find(|t| t.alias.as_deref() == Some(q.as_str()))
                .or_else(|| a.tables.iter().find(|t| t.alias.is_none() && t.name == *q));
            if let Some(t) = table {
                columns_of(&mut b, &mut out, (t.schema.clone(), t.name.clone()));
            } else if objects.iter().any(|o| o.kind == "schema" && o.name == *q) {
                for o in objects {
                    if o.schema.as_deref() == Some(q.as_str()) {
                        push_obj(&mut out, o);
                    }
                }
            } else if names.objects.is_some() {
                columns_of(&mut b, &mut out, (None, q.clone()));
            }
        }
        Want::Any => {
            for t in &a.tables {
                columns_of(&mut b, &mut out, (t.schema.clone(), t.name.clone()));
            }
            for o in objects {
                if o.visible && (o.is_relation() || o.is_function()) {
                    push_obj(&mut out, o);
                }
            }
            for k in keywords {
                if k.starts_with(&prefix) {
                    out.push(Item {
                        label: k.to_uppercase(),
                        kind: "keyword",
                        insert: k.to_uppercase(),
                    });
                }
            }
        }
    }
    out.sort_by(|x, y| {
        (rank(x.kind), x.label.to_lowercase(), x.kind).cmp(&(
            rank(y.kind),
            y.label.to_lowercase(),
            y.kind,
        ))
    });
    out.dedup_by(|x, y| x.label == y.label && x.kind == y.kind);
    // a name typed out completely is not offered again (only itself)
    if out.len() == 1 && out[0].label.eq_ignore_ascii_case(&a.prefix) && !b.loading {
        out.clear();
    }
    b.items = out;
    b
}

/// The open list (one at a time, for the active query tab).
#[derive(Debug, Clone)]
pub struct Popup {
    /// Query tab id.
    pub tab: u64,
    /// In the template form's `sql` box (not the tab's editor), D-052.
    pub in_form: bool,
    pub analysis: Analysis,
    pub items: Vec<Item>,
    pub loading: bool,
    pub selected: usize,
    pub offset: usize,
    /// Opened by typing (not by key): shown only while it has entries.
    pub auto: bool,
}

impl Popup {
    pub fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + LIST_ROWS {
            self.offset = self.selected + 1 - LIST_ROWS;
        }
    }

    /// Drawn (and taking keys): a list opened by typing only while it has entries.
    pub fn shown(&self) -> bool {
        !self.auto || !self.items.is_empty()
    }

    /// New entries (the selection stays on the same label when it is still there).
    pub fn set_items(&mut self, items: Vec<Item>, loading: bool) {
        let label = self.items.get(self.selected).map(|i| i.label.clone());
        self.items = items;
        self.loading = loading;
        self.selected = label
            .and_then(|l| self.items.iter().position(|i| i.label == l))
            .unwrap_or(0);
        self.offset = self.selected.saturating_sub(LIST_ROWS - 1);
    }
}

/// Draws the list under the cursor `at` (above it when there is no room) inside
/// `screen`.
pub fn render(frame: &mut Frame, popup: &Popup, at: Position, screen: Rect, theme: &Theme) {
    let mut lines: Vec<(String, &str)> = popup
        .items
        .iter()
        .map(|i| (i.label.clone(), i.kind))
        .collect();
    if lines.is_empty() {
        lines.push((
            if popup.loading {
                "loading…"
            } else {
                "no suggestions"
            }
            .to_owned(),
            "",
        ));
    }
    let label_w = lines
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0)
        .min(40);
    let kind_w = lines.iter().map(|(_, k)| k.len()).max().unwrap_or(0);
    let width = (label_w + kind_w + 5).min(screen.width as usize) as u16;
    let rows = lines.len().min(LIST_ROWS) as u16;
    let height = rows + 2;
    let prefix = popup.analysis.prefix.chars().count() as u16;
    let x =
        at.x.saturating_sub(prefix + 1)
            .min(screen.right().saturating_sub(width));
    let below = at.y + 1;
    let y = if below + height <= screen.bottom() {
        below
    } else {
        at.y.saturating_sub(height)
    };
    let area = Rect::new(x, y, width, height).intersection(screen);
    let normal = theme.get(Role::MenuNormal);
    let selected = theme.get(Role::MenuSelected);
    let quiet = theme.get(Role::MenuDisabled);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::bordered()
            .border_set(theme.plain_border())
            .border_style(normal)
            .style(normal),
        area,
    );
    let inner_w = area.width.saturating_sub(2) as usize;
    let shown: Vec<Line> = lines
        .iter()
        .enumerate()
        .skip(popup.offset)
        .take(rows as usize)
        .map(|(n, (label, kind))| {
            let on = n == popup.selected && !popup.items.is_empty();
            let style = if on { selected } else { normal };
            let label: String = label.chars().take(label_w).collect();
            let gap = inner_w.saturating_sub(label.chars().count() + kind.len() + 2);
            Line::from(vec![
                Span::styled(format!(" {label}{}", " ".repeat(gap)), style),
                Span::styled(format!("{kind} "), if on { selected } else { quiet }),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(shown),
        Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), rows),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgc_editor::complete::TableRef;

    fn obj(schema: Option<&str>, name: &str, kind: &str, visible: bool) -> Obj {
        Obj {
            schema: schema.map(str::to_owned),
            name: name.to_owned(),
            kind: kind.to_owned(),
            visible,
        }
    }

    fn names() -> Names {
        let mut n = Names {
            objects: Some(vec![
                obj(None, "public", "schema", true),
                obj(None, "sales", "schema", true),
                obj(Some("public"), "orders", "table", true),
                obj(Some("public"), "Order Items", "table", true),
                obj(Some("public"), "order_total", "function", true),
                obj(Some("public"), "order_total", "function", true),
                obj(Some("sales"), "invoices", "table", false),
                obj(Some("public"), "orders_id_seq", "sequence", true),
            ]),
            ..Names::default()
        };
        n.columns.insert(
            (None, "orders".into()),
            vec!["id".into(), "ordered_at".into()],
        );
        n
    }

    fn analysis(prefix: &str, want: Want, tables: Vec<TableRef>) -> Analysis {
        Analysis {
            prefix: prefix.to_owned(),
            start: 0,
            want,
            tables,
        }
    }

    fn labels(b: &Built) -> Vec<String> {
        b.items
            .iter()
            .map(|i| format!("{} {}", i.label, i.kind))
            .collect()
    }

    #[test]
    fn relations_after_from() {
        let b = build(
            &analysis("ord", Want::Relations, vec![]),
            &names(),
            &["order"],
        );
        assert_eq!(labels(&b), ["Order Items table", "orders table"]);
        assert_eq!(b.items[0].insert, "\"Order Items\"");
        assert!(b.needs.is_empty() && !b.loading);
    }

    #[test]
    fn any_gives_columns_names_functions_and_upper_case_keywords() {
        let t = TableRef {
            schema: None,
            name: "orders".into(),
            alias: Some("o".into()),
        };
        let b = build(
            &analysis("ord", Want::Any, vec![t]),
            &names(),
            &["order", "or"],
        );
        assert_eq!(
            labels(&b),
            [
                "ordered_at column",
                "Order Items table",
                "orders table",
                "order_total function",
                "ORDER keyword"
            ]
        );
        assert_eq!(b.items.last().unwrap().insert, "ORDER");
    }

    #[test]
    fn qualified_alias_schema_and_loading() {
        let t = TableRef {
            schema: None,
            name: "orders".into(),
            alias: Some("o".into()),
        };
        let n = names();
        let b = build(&analysis("", Want::Qualified("o".into()), vec![t]), &n, &[]);
        assert_eq!(labels(&b), ["id column", "ordered_at column"]);
        // a schema: its objects, also those not on the search path
        let b = build(
            &analysis("", Want::Qualified("sales".into()), vec![]),
            &n,
            &[],
        );
        assert_eq!(labels(&b), ["invoices table"]);
        // unknown columns: asked for, list waits
        let t = TableRef {
            schema: Some("sales".into()),
            name: "invoices".into(),
            alias: None,
        };
        let b = build(
            &analysis("", Want::Qualified("invoices".into()), vec![t]),
            &n,
            &[],
        );
        assert!(b.loading);
        assert_eq!(
            b.needs,
            [Need::Columns((Some("sales".into()), "invoices".into()))]
        );
        // nothing loaded yet
        let b = build(
            &analysis("x", Want::Relations, vec![]),
            &Names::default(),
            &[],
        );
        assert_eq!(b.needs, [Need::Objects]);
        assert!(b.loading);
    }

    #[test]
    fn objects_rows_and_clear() {
        let mut n = Names::default();
        n.set_objects(vec![
            vec!["".into(), "public".into(), "schema".into(), "t".into()],
            vec!["public".into(), "t1".into(), "table".into(), "f".into()],
        ]);
        let o = n.objects.as_ref().unwrap();
        assert_eq!(o[0], obj(None, "public", "schema", true));
        assert!(!o[1].visible);
        let g = n.generation;
        n.clear();
        assert!(n.objects.is_none());
        assert_eq!(n.generation, g + 1);
    }
}
