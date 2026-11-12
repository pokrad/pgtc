//! Developer templates (D-013, D-046): the picker, the form's choice lists and
//! defaults, the rows of list parameters.

use pgc_editor::Action;
use pgc_sql::devtemplate::{self, ParamType, ParamValue, QualName, TemplateLibrary};

use super::lookups::LookupTarget;
use super::{App, CONNECTION_BUSY, Focus};
use crate::dialog::{Dialog, DialogKind, Widget};
use crate::template_dialog::{Choice, FormEvent, TemplateForm};
use crate::tree::RowKind;

/// The template picker (D-046): the library, (name, description) per entry, the rows
/// shown (`None` = folder row) and the filter they were built for.
pub(super) struct Picker {
    lib: TemplateLibrary,
    names: Vec<(String, String)>,
    rows: Vec<Option<usize>>,
    filter: Option<String>,
}

impl App {
    /// F7 / Tools → Generate SQL from developer template: only for a connected, idle
    /// query tab.
    pub(super) fn open_template_picker(&mut self) {
        let Some(q) = self.query() else {
            self.error("Developer templates insert SQL into a query tab.\nOpen a query tab first.");
            return;
        };
        if !q.connected() {
            self.error("The query tab has no active connection.\nSelect a database in the tree and connect the tab first.");
            return;
        }
        if q.busy() || q.paused() || q.export.is_some() {
            self.error(CONNECTION_BUSY);
            return;
        }
        let Some(version) = self.tab_server_version() else {
            self.error("The server version of the tab is not known yet.");
            return;
        };
        // the templates folder is the only source (D-013)
        let lib = TemplateLibrary::scan(&self.paths.user_templates_dir());
        if lib.entries.is_empty() {
            self.error("No developer templates — run Settings → Install templates.");
            return;
        }
        let names = lib
            .entries
            .iter()
            .map(
                |e| match e.resolve(version).map(|(text, _)| devtemplate::parse(text)) {
                    Some(Ok(t)) => (t.name.unwrap_or_else(|| e.stem().to_owned()), t.description),
                    Some(Err(err)) => (format!("{} (error)", e.stem()), err),
                    None => (
                        format!("{} (not for this server version)", e.stem()),
                        String::new(),
                    ),
                },
            )
            .collect();
        self.picker = Some(Picker {
            lib,
            names,
            rows: Vec::new(),
            filter: None,
        });
        self.dialogs.push(Dialog::new(
            DialogKind::TemplatePicker,
            "Generate SQL from developer template",
            64,
            vec![
                Widget::field("search", "&Search:", "", 9),
                Widget::list("items", "", Vec::new(), 0, 14),
                Widget::label_id("desc", " "),
                Widget::button("ok", "&OK", true),
                Widget::button("cancel", "&Cancel", false),
            ],
        ));
        self.sync_picker();
    }

    /// Picker: rebuilds the tree rows when the search text changed; shows the
    /// description of the selected template.
    pub(super) fn sync_picker(&mut self) {
        let Some(d) = self.dialogs.last_mut() else {
            return;
        };
        if d.kind != DialogKind::TemplatePicker {
            return;
        }
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let filter = d.text("search").unwrap_or("").trim().to_lowercase();
        if p.filter.as_deref() != Some(filter.as_str()) {
            let (rows, labels, marks) = picker_rows(p, &filter);
            p.rows = rows;
            p.filter = Some(filter);
            let first = p.rows.iter().position(Option::is_some).unwrap_or(0);
            d.set_list("items", labels, marks, first);
        }
        let desc = d
            .selected("items")
            .and_then(|i| p.rows.get(i).copied().flatten())
            .map(|e| p.names[e].1.clone())
            .unwrap_or_default();
        d.set_label("desc", if desc.is_empty() { " " } else { &desc });
    }

    pub(super) fn template_chosen(&mut self, dialog: Dialog) {
        let entry = dialog.selected("items").and_then(|i| {
            self.picker
                .as_ref()
                .and_then(|p| p.rows.get(i).copied().flatten())
        });
        let Some(e) = entry else {
            // a folder row: keep the picker open
            self.dialogs.push(dialog);
            return;
        };
        let Some(version) = self.tab_server_version() else {
            return;
        };
        let picker = self.picker.take().expect("picker");
        let entry = &picker.lib.entries[e];
        let Some((text, origin)) = entry.resolve(version) else {
            self.error("This template has no version for the tab's server.");
            return;
        };
        let file = match devtemplate::parse(text) {
            Ok(f) => f,
            Err(err) => {
                self.error(&format!("Template {origin}:\n{err}"));
                return;
            }
        };
        let Some(q) = self.query() else {
            return;
        };
        let database = q.database().unwrap_or("").to_owned();
        let tab = self.tabs[self.active_tab].id;
        let title = format!("New: {}", picker.names[e].0);
        let ctx = devtemplate::TemplateContext { version, database };
        let mut form = TemplateForm::new(tab, title, file, ctx, self.tab_width());
        self.template_defaults(&mut form);
        self.template = Some(form);
        self.row_choices.clear();
        self.start_lookups();
    }

    /// Choice defaults from the tree selection when it is in the tab's server and
    /// database (D-046).
    fn template_defaults(&self, form: &mut TemplateForm) {
        use crate::objects::ObjType as T;
        use devtemplate::Source as S;
        let Some((uid, db)) = self.query().and_then(|q| q.binding.clone()) else {
            return;
        };
        let Some(RowKind::Node(nid)) = self.tree.selected().map(|r| r.kind) else {
            return;
        };
        let Some((server, database, objects)) = self.tree.object_path(nid) else {
            return;
        };
        if self.tree.servers[server].uid != uid {
            return;
        }
        let same_db = database.as_deref() == Some(db.as_str());
        let schema = objects
            .iter()
            .find(|o| o.0 == T::Schema)
            .map(|o| o.1.clone());
        let nearest = |kinds: &[T]| {
            objects
                .iter()
                .rev()
                .find(|o| kinds.contains(&o.0))
                .map(|o| o.1.clone())
        };
        // functions and procedures: the tree label is `name(args)` (D-050)
        let nearest_label = |kinds: &[T]| {
            objects
                .iter()
                .rev()
                .find(|o| kinds.contains(&o.0))
                .map(|o| o.2.clone())
        };
        let mut wants = Vec::new();
        for f in &form.fields {
            let Some(source) = f.param.source else {
                continue;
            };
            let qualified =
                |name: Option<String>| Some(QualName::new(schema.clone()?, name?).to_string());
            let label = match source {
                S::Roles => objects
                    .last()
                    .filter(|o| o.0 == T::Role)
                    .map(|o| o.1.clone()),
                _ if !same_db => None,
                S::Schemas => schema.clone(),
                S::Tables => qualified(nearest(&[T::Table, T::ForeignTable, T::Partition])),
                S::Views => qualified(nearest(&[T::View])),
                S::MaterializedViews => qualified(nearest(&[T::MatView])),
                S::Sequences => qualified(nearest(&[T::Sequence])),
                S::Functions => qualified(nearest_label(&[T::Function, T::TriggerFunction])),
                S::Procedures => qualified(nearest_label(&[T::Procedure])),
                S::Types => qualified(nearest(&[T::Type, T::Domain])),
                S::UserTypes => qualified(nearest(&[T::Type])),
                S::Domains => qualified(nearest(&[T::Domain])),
                S::Indexes => qualified(nearest(&[T::Index])),
                S::Triggers => nearest(&[T::Trigger]).and_then(|name| {
                    let table = nearest(&[T::Table, T::Partition, T::View])?;
                    Some(
                        QualName {
                            table: Some(table),
                            ..QualName::new(schema.clone()?, name)
                        }
                        .to_string(),
                    )
                }),
            };
            if let Some(label) = label {
                wants.push((f.param.name.clone(), label));
            }
        }
        for (name, label) in wants {
            form.want(&name, label);
        }
    }

    /// Asks the tab's connection for every live choice list of the form.
    fn start_lookups(&mut self) {
        let Some(form) = &self.template else {
            return;
        };
        let mut wanted = Vec::new();
        for f in &form.fields {
            match f.param.kind {
                ParamType::Choice if f.param.options.is_empty() => {
                    wanted.push((
                        LookupTarget::Field(f.param.name.clone(), shape(&f.param)),
                        sql_of(&f.param),
                    ));
                }
                ParamType::List => {
                    for sub in &f.param.fields {
                        if sub.kind == ParamType::Choice && sub.options.is_empty() {
                            wanted.push((
                                LookupTarget::Row(
                                    f.param.name.clone(),
                                    sub.name.clone(),
                                    shape(sub),
                                ),
                                sql_of(sub),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        let Some(handle) = self
            .query()
            .and_then(|q| q.conn.as_ref())
            .map(|c| c.handle.clone())
        else {
            return;
        };
        for (target, sql) in wanted {
            self.next_key += 1;
            self.template_lookups.insert(self.next_key, target);
            handle.lookup(self.next_key, sql);
        }
    }

    pub(super) fn form_event(&mut self, event: FormEvent) {
        match event {
            FormEvent::None => {}
            FormEvent::Cancel => {
                self.template = None;
                self.drop_form_lookups();
            }
            FormEvent::Ok => {
                let Some(form) = &self.template else {
                    return;
                };
                match form.result() {
                    Ok(sql) => {
                        let tab = form.tab;
                        self.template = None;
                        self.drop_form_lookups();
                        if let Some(i) = self.tab_index(tab) {
                            self.active_tab = i;
                            self.focus = Focus::Tabs;
                            self.edit(Action::Insert(sql));
                            self.status = "Template inserted".to_owned();
                        }
                    }
                    Err(e) => self.error(&e),
                }
            }
            FormEvent::AddRow(field) => self.open_row_dialog(field, None),
            FormEvent::EditRow(field, row) => self.open_row_dialog(field, Some(row)),
        }
    }

    /// The small dialog with the fields of one row of a `list` parameter.
    fn open_row_dialog(&mut self, field: usize, row: Option<usize>) {
        let Some(form) = &self.template else {
            return;
        };
        let param = form.fields[field].param.clone();
        let current = row.and_then(|r| match &form.fields[field].value {
            crate::template_dialog::FieldValue::List { rows, .. } => rows.get(r).cloned(),
            _ => None,
        });
        let lw = param
            .fields
            .iter()
            .map(|f| f.label().chars().count() as u16 + 2)
            .max()
            .unwrap_or(8)
            .min(24);
        let mut widgets = Vec::new();
        for sub in &param.fields {
            let id = crate::dialog::intern(&sub.name);
            let label = format!("{}{}:", sub.label(), if sub.required { "*" } else { "" });
            let value = current.as_ref().and_then(|r| r.get(&sub.name));
            match sub.kind {
                ParamType::Bool => {
                    let checked = match value {
                        Some(ParamValue::Bool(b)) => *b,
                        _ => sub.default_bool(),
                    };
                    widgets.push(Widget::checkbox(id, sub.label(), checked));
                }
                ParamType::Choice => {
                    let items = self.row_items(&param.name, sub);
                    let text = match value {
                        Some(ParamValue::Text(s)) => s.clone(),
                        Some(ParamValue::Object(q)) => q.to_string(),
                        _ => sub.default_text(),
                    };
                    match items {
                        Some(items) => {
                            let mut labels: Vec<String> = vec![String::new()];
                            labels.extend(items.iter().map(|c| c.label.clone()));
                            let selected = labels.iter().position(|l| *l == text).unwrap_or(0);
                            widgets.push(Widget::combo(id, &label, labels, selected, lw));
                        }
                        None => widgets.push(Widget::field(id, &label, &text, lw)),
                    }
                }
                _ => {
                    let text = match value {
                        Some(ParamValue::Text(s)) => s.clone(),
                        _ => sub.default_text(),
                    };
                    widgets.push(Widget::field(id, &label, &text, lw));
                }
            }
        }
        widgets.push(Widget::button("ok", "&OK", true));
        widgets.push(Widget::button("cancel", "&Cancel", false));
        let title = if row.is_some() { "Edit row" } else { "Add row" };
        self.row_edit = Some((field, row));
        self.dialogs.push(Dialog::new(
            DialogKind::TemplateRow,
            &format!("{}: {title}", param.label()),
            60,
            widgets,
        ));
    }

    /// Items for a choice field of a list row: fixed options or the loaded list;
    /// `None` = not loaded (a text input is used instead).
    fn row_items(&self, list: &str, sub: &devtemplate::Param) -> Option<Vec<Choice>> {
        if !sub.options.is_empty() {
            return Some(
                sub.options
                    .iter()
                    .map(|o| Choice {
                        label: o.clone(),
                        value: ParamValue::Text(o.clone()),
                    })
                    .collect(),
            );
        }
        self.row_choices
            .get(&(list.to_owned(), sub.name.clone()))
            .cloned()
    }

    pub(super) fn row_done(&mut self, dialog: Dialog) {
        let Some((field, row)) = self.row_edit else {
            return;
        };
        let Some(param) = self
            .template
            .as_ref()
            .map(|f| f.fields[field].param.clone())
        else {
            return;
        };
        let mut values = std::collections::BTreeMap::new();
        for sub in &param.fields {
            let v = match sub.kind {
                ParamType::Bool => ParamValue::Bool(dialog.checked(&sub.name)),
                ParamType::Choice => match self.row_items(&param.name, sub) {
                    Some(items) => match dialog.selected(&sub.name) {
                        Some(i) if i > 0 => items[i - 1].value.clone(),
                        _ => ParamValue::Text(String::new()),
                    },
                    None => {
                        ParamValue::Text(dialog.text(&sub.name).unwrap_or("").trim().to_owned())
                    }
                },
                _ => ParamValue::Text(dialog.text(&sub.name).unwrap_or("").trim().to_owned()),
            };
            values.insert(sub.name.clone(), v);
        }
        if let Err(e) = devtemplate::validate(&param.fields, &values) {
            self.dialogs.push(dialog);
            self.error(&e);
            return;
        }
        self.row_edit = None;
        let Some(form) = self.template.as_mut() else {
            return;
        };
        if let Some(rows) = form.list_rows_mut(field) {
            let at = match row {
                Some(r) if r < rows.len() => {
                    rows[r] = values;
                    r
                }
                _ => {
                    rows.push(values);
                    rows.len() - 1
                }
            };
            form.list_select(field, at);
        }
        form.refresh();
    }
}

fn shape(p: &devtemplate::Param) -> devtemplate::Shape {
    p.source
        .map_or(devtemplate::Shape::Name, devtemplate::Source::shape)
}

fn sql_of(p: &devtemplate::Param) -> String {
    match (p.source, &p.query) {
        (Some(s), _) => s.sql().to_owned(),
        (None, Some(q)) => q.clone(),
        (None, None) => "SELECT NULL WHERE false".to_owned(),
    }
}

/// Picker rows: folders (indented) with their templates; with a filter only the
/// matching templates and their folders. Returns rows, labels, highlight marks.
fn picker_rows(p: &Picker, filter: &str) -> (Vec<Option<usize>>, Vec<String>, Vec<bool>) {
    let mut rows = Vec::new();
    let mut labels = Vec::new();
    let mut marks = Vec::new();
    let mut open: Vec<String> = Vec::new();
    for (i, e) in p.lib.entries.iter().enumerate() {
        let (name, desc) = &p.names[i];
        if !filter.is_empty()
            && !name.to_lowercase().contains(filter)
            && !desc.to_lowercase().contains(filter)
            && !e.rel.to_lowercase().contains(filter)
        {
            continue;
        }
        let folders: Vec<String> = e.folders().into_iter().map(str::to_owned).collect();
        let common = open
            .iter()
            .zip(&folders)
            .take_while(|(a, b)| a == b)
            .count();
        for (depth, f) in folders.iter().enumerate().skip(common) {
            rows.push(None);
            labels.push(format!("{}▾ {f}", "  ".repeat(depth)));
            marks.push(true);
        }
        open = folders;
        rows.push(Some(i));
        labels.push(format!("{}  {name}", "  ".repeat(open.len())));
        marks.push(false);
    }
    (rows, labels, marks)
}
