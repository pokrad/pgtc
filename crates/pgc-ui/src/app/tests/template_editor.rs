//! The developer template editor (D-074): manager, template tabs, saving, test.

use pgc_sql::devtemplate::{ParamType, Source};

use super::*;
use crate::dialog::{Dialog, WidgetKind};
use crate::template_tab::Part;

fn list_items(d: &Dialog, id: &str) -> Vec<String> {
    match d.widgets.iter().find(|w| w.id == id).map(|w| &w.kind) {
        Some(WidgetKind::List { items, .. }) => items.clone(),
        _ => panic!("no list {id}"),
    }
}

fn top(a: &App) -> &Dialog {
    a.dialogs.last().expect("a dialog")
}

/// The text of the message (or question) on top.
fn message(a: &App) -> String {
    top(a).widgets[0].label.text.clone()
}

/// An app with pgtc's templates installed and an empty section `Mine`.
fn app_with_templates() -> (App, tempfile::TempDir) {
    let (a, dir) = app();
    let templates = a.paths.user_templates_dir();
    devtemplate::install(&templates).unwrap();
    std::fs::create_dir_all(templates.join("default").join("Mine")).unwrap();
    (a, dir)
}

/// Opens the built-in *Create table* template in a tab.
fn open_table_template(a: &mut App) -> std::path::PathBuf {
    let path = a
        .paths
        .user_templates_dir()
        .join("default")
        .join("Tables")
        .join("table.pgtemplate");
    let text = std::fs::read_to_string(&path).unwrap();
    a.open_template_file(path.clone(), &text);
    path
}

fn body_text(a: &App) -> String {
    a.tabs[a.active_tab].view.editor.text()
}

#[test]
fn manager_shows_sections_templates_and_variants() {
    let (mut a, _d) = app_with_templates();
    assert!(
        a.is_enabled(Command::TemplateManager),
        "no connection needed"
    );
    a.run(Command::TemplateManager);
    assert_eq!(top(&a).kind, DialogKind::TemplateManager);
    let items = list_items(top(&a), "items");
    assert!(items.contains(&"▾ Tables".to_owned()), "{items:?}");
    assert!(items.contains(&"    Create table".to_owned()), "{items:?}");
    // the empty section is listed here (the F7 picker hides it)
    assert!(items.contains(&"▾ Mine".to_owned()), "{items:?}");
    // a template with a file for newer servers has a row for each
    assert!(
        items
            .iter()
            .any(|i| i.trim() == "Create trigger with function"),
        "{items:?}"
    );
    assert!(
        items
            .iter()
            .any(|i| i.trim() == "Create trigger with function  [12_plus]"),
        "{items:?}"
    );
    // the search box filters by name, description and file
    key(&mut a, "Shift+Tab");
    typing(&mut a, "sequence");
    let items = list_items(top(&a), "items");
    assert_eq!(
        items,
        vec!["▾ Sequences", "    Drop sequence", "    Create sequence"]
    );
    esc(&mut a);
    assert!(a.dialogs.is_empty());
    assert!(a.templates.manager.is_none());
}

#[test]
fn manager_changes_the_templates_folder() {
    let (mut a, _d) = app_with_templates();
    let dir = a.paths.user_templates_dir();
    a.run(Command::TemplateManager);
    // New section
    key(&mut a, "t");
    assert!(matches!(top(&a).kind, DialogKind::TemplateName { .. }));
    typing(&mut a, "Reports");
    key(&mut a, "Enter");
    assert!(dir.join("default").join("Reports").is_dir());
    assert_eq!(top(&a).kind, DialogKind::TemplateManager, "it stays open");
    assert!(list_items(top(&a), "items").contains(&"▾ Reports".to_owned()));
    // a bad name is refused, the dialog stays
    key(&mut a, "t");
    typing(&mut a, "a:b");
    key(&mut a, "Enter");
    assert!(message(&a).contains("cannot contain"));
    key(&mut a, "Enter");
    assert!(matches!(top(&a).kind, DialogKind::TemplateName { .. }));
    esc(&mut a);

    // Copy, then Rename, then Delete a template
    let select = |a: &mut App, label: &str| {
        let at = list_items(top(a), "items")
            .iter()
            .position(|i| i.trim() == label)
            .unwrap_or_else(|| panic!("no row {label}"));
        a.dialogs.last_mut().unwrap().list_select("items", at);
    };
    select(&mut a, "Create table");
    key(&mut a, "c");
    key(&mut a, "Enter");
    let copy = dir
        .join("default")
        .join("Tables")
        .join("table_copy.pgtemplate");
    assert!(copy.is_file());
    // both files have the header name "Create table": the first is the original
    select(&mut a, "Drop table");
    key(&mut a, "r");
    typing(&mut a, "2");
    key(&mut a, "Enter");
    assert!(
        dir.join("default")
            .join("Tables")
            .join("drop_table2.pgtemplate")
            .is_file()
    );
    assert!(
        !dir.join("default")
            .join("Tables")
            .join("drop_table.pgtemplate")
            .exists()
    );
    select(&mut a, "Drop table");
    key(&mut a, "d");
    assert!(matches!(
        top(&a).kind,
        DialogKind::ConfirmTemplateDelete { .. }
    ));
    key(&mut a, "y");
    assert!(
        !dir.join("default")
            .join("Tables")
            .join("drop_table2.pgtemplate")
            .exists()
    );
    // a section with templates is not deleted
    select(&mut a, "▾ Tables");
    key(&mut a, "d");
    key(&mut a, "y");
    assert!(message(&a).contains("not empty"), "{}", message(&a));
    key(&mut a, "Enter");
    assert!(dir.join("default").join("Tables").is_dir());
    // an empty one is
    select(&mut a, "▾ Mine");
    key(&mut a, "d");
    key(&mut a, "y");
    assert!(!dir.join("default").join("Mine").exists());
}

#[test]
fn new_template_from_the_manager_to_the_file() {
    let (mut a, _d) = app_with_templates();
    let dir = a.paths.user_templates_dir();
    a.run(Command::TemplateManager);
    let at = list_items(top(&a), "items")
        .iter()
        .position(|i| i == "▾ Mine")
        .unwrap();
    a.dialogs.last_mut().unwrap().list_select("items", at);
    key(&mut a, "n");
    assert!(a.dialogs.is_empty(), "the manager closes");
    let t = a.template().expect("a template tab");
    assert_eq!(t.section, "Mine");
    assert_eq!(t.part, Part::Header);
    assert_eq!(a.tabs[a.active_tab].label(), "Template: New template");
    // the file bar says which section it was made for
    assert!(
        screen_text_sized(&a, 120, 34)
            .contains("New template — section: Mine · version folder: default"),
        "{}",
        screen_text_sized(&a, 120, 34)
    );
    assert_eq!(a.context(), Some(Context::Template));

    // name and description
    key(&mut a, "Enter");
    assert!(matches!(top(&a).kind, DialogKind::TemplateProps { .. }));
    typing(&mut a, "Refresh view");
    key(&mut a, "Tab");
    typing(&mut a, "REFRESH MATERIALIZED VIEW");
    key(&mut a, "Enter");
    assert_eq!(a.template().unwrap().name, "Refresh view");
    assert_eq!(a.tabs[a.active_tab].label(), "Template: Refresh view");

    // a parameter: a required choice of materialized views
    key(&mut a, "Insert");
    assert!(matches!(
        top(&a).kind,
        DialogKind::TemplateParam { field: false, .. }
    ));
    typing(&mut a, "view");
    key(&mut a, "Alt+R");
    let choice = ParamType::ALL
        .iter()
        .position(|t| *t == ParamType::Choice)
        .unwrap();
    a.dialogs
        .last_mut()
        .unwrap()
        .set_combo_selected("type", choice);
    a.sync_param_dialog();
    // the inputs of a choice are there now; the typed name is kept
    assert_eq!(top(&a).text("name"), Some("view"));
    assert!(top(&a).selected("values").is_some());
    a.dialogs
        .last_mut()
        .unwrap()
        .set_combo_selected("values", 1);
    a.sync_param_dialog();
    let source = Source::ALL
        .iter()
        .position(|s| *s == Source::MaterializedViews)
        .unwrap();
    a.dialogs
        .last_mut()
        .unwrap()
        .set_combo_selected("source", source);
    key(&mut a, "Ctrl+Enter");
    assert!(a.dialogs.is_empty(), "{}", message(&a));
    let t = a.template().unwrap();
    assert_eq!(t.params.len(), 1);
    let p = &t.params[0];
    assert_eq!((p.name.as_str(), p.kind), ("view", ParamType::Choice));
    assert_eq!(p.source, Some(Source::MaterializedViews));
    assert!(p.required && p.options.is_empty() && p.query.is_none());
    // a second one with the same name is refused
    key(&mut a, "Insert");
    typing(&mut a, "view");
    key(&mut a, "Ctrl+Enter");
    assert!(message(&a).contains("used already"));
    key(&mut a, "Enter");
    esc(&mut a);
    assert!(a.templates.param_edit.is_none());

    // the body, with the helper menu
    key(&mut a, "Tab");
    assert_eq!(a.template().unwrap().part, Part::Body);
    assert_eq!(a.context(), Some(Context::Editor));
    typing(&mut a, "REFRESH MATERIALIZED VIEW ");
    key(&mut a, "F7");
    assert!(a.popup.is_some());
    key(&mut a, "p");
    assert!(matches!(top(&a).kind, DialogKind::TemplateInsert { .. }));
    let items = list_items(top(&a), "items");
    assert!(items[0].starts_with("view "), "{items:?}");
    assert!(items.iter().any(|i| i.starts_with("server_version")));
    key(&mut a, "Enter");
    typing(&mut a, ";");
    assert_eq!(body_text(&a), "REFRESH MATERIALIZED VIEW {{ view|ident }};");
    // the preview follows: a required value without a default is its own name
    let t = a.template().unwrap();
    assert_eq!(t.preview.text(), "REFRESH MATERIALIZED VIEW schema.view;\n");
    assert_eq!(t.status(), "No problems found");
    assert!(t.assumed);

    // Save: section, version folder and file name are asked the first time
    key(&mut a, "Ctrl+S");
    assert!(matches!(top(&a).kind, DialogKind::TemplateSave { .. }));
    assert_eq!(top(&a).selected_combo_item("section"), Some("Mine"));
    assert_eq!(top(&a).text("file"), Some("refresh_view"));
    assert_eq!(top(&a).text("folder"), Some("default"));
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty(), "{}", message(&a));
    let path = dir
        .join("default")
        .join("Mine")
        .join("refresh_view.pgtemplate");
    let file = devtemplate::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file.name.as_deref(), Some("Refresh view"));
    assert_eq!(file.description, "REFRESH MATERIALIZED VIEW");
    assert_eq!(file.params.len(), 1);
    assert_eq!(file.body, "REFRESH MATERIALIZED VIEW {{ view|ident }};");
    assert_eq!(a.template().unwrap().path.as_deref(), Some(path.as_path()));
    assert!(!a.tabs[a.active_tab].modified());
    assert!(a.status.starts_with("Saved "));

    // Ctrl+S works from the header list too
    key(&mut a, "Alt+Up");
    assert!(a.template_list_focused());
    assert_eq!(
        a.command_for(KeyCombo::parse("Ctrl+S").unwrap()),
        Some(Command::SaveFile)
    );
    key(&mut a, "Tab");
    // saved again without a dialog; closing does not ask
    typing(&mut a, " ");
    assert!(a.tabs[a.active_tab].modified());
    key(&mut a, "Ctrl+S");
    assert!(a.dialogs.is_empty());
    assert!(
        std::fs::read_to_string(&path).unwrap().ends_with(";\n")
            || !a.tabs[a.active_tab].modified()
    );
    key(&mut a, "Ctrl+W");
    assert!(a.template().is_none());
    assert!(a.dialogs.is_empty());

    // the manager opens the file again; a second Edit shows the same tab
    a.run(Command::TemplateManager);
    let at = list_items(top(&a), "items")
        .iter()
        .position(|i| i.trim() == "Refresh view")
        .unwrap();
    a.dialogs.last_mut().unwrap().list_select("items", at);
    key(&mut a, "Enter");
    assert_eq!(a.template().unwrap().name, "Refresh view");
    assert_eq!(a.template().unwrap().part, Part::Body);
    let tabs = a.tabs.len();
    a.run(Command::TemplateManager);
    a.dialogs.last_mut().unwrap().list_select("items", at);
    key(&mut a, "Enter");
    assert_eq!(a.tabs.len(), tabs);
}

#[test]
fn a_template_that_does_not_parse_is_not_saved() {
    let (mut a, _d) = app_with_templates();
    let path = open_table_template(&mut a);
    let before = std::fs::read_to_string(&path).unwrap();
    // a Jinja error on a line of its own at the end
    key(&mut a, "Ctrl+End");
    typing(&mut a, "{% if %}");
    let lines = a.editor().len_lines();
    key(&mut a, "Ctrl+Home");
    key(&mut a, "Ctrl+S");
    assert!(
        message(&a).starts_with("The template is not saved"),
        "{}",
        message(&a)
    );
    key(&mut a, "Enter");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert!(a.tabs[a.active_tab].modified());
    // the cursor is on the line of the error, and the status row names it
    assert_eq!(a.editor().cursor_line_col().0, lines - 1);
    let status = a.template().unwrap().status();
    assert!(status.starts_with("Body, line "), "{status}");
    // closing asks; Yes cannot save, so the tab stays
    key(&mut a, "Ctrl+W");
    assert!(matches!(top(&a).kind, DialogKind::SaveChanges { .. }));
    key(&mut a, "y");
    assert!(message(&a).starts_with("The template is not saved"));
    key(&mut a, "Enter");
    assert!(a.template().is_some());
    assert!(a.closing.nothing_waits());
    // No closes it without saving
    key(&mut a, "Ctrl+W");
    key(&mut a, "n");
    assert!(a.template().is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn saving_a_template_of_pgtc_asks_first() {
    let (mut a, _d) = app_with_templates();
    let path = open_table_template(&mut a);
    let before = std::fs::read_to_string(&path).unwrap();
    assert!(!a.tabs[a.active_tab].modified());
    assert_eq!(a.template().unwrap().name, "Create table");
    typing(&mut a, "-- mine\n");
    key(&mut a, "Ctrl+S");
    assert!(matches!(
        top(&a).kind,
        DialogKind::ConfirmBuiltinSave { .. }
    ));
    // Save as copy: the save dialog with another file name
    key(&mut a, "c");
    assert!(matches!(top(&a).kind, DialogKind::TemplateSave { .. }));
    assert_eq!(top(&a).text("file"), Some("table_copy"));
    assert_eq!(top(&a).selected_combo_item("section"), Some("Tables"));
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty(), "{}", message(&a));
    let copy = path.with_file_name("table_copy.pgtemplate");
    assert!(std::fs::read_to_string(&copy).unwrap().contains("-- mine"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert_eq!(a.template().unwrap().path.as_deref(), Some(copy.as_path()));
    assert!(!a.tabs[a.active_tab].modified());
    // the copy is the user's own: saved without a question
    typing(&mut a, "-- more\n");
    key(&mut a, "Ctrl+S");
    assert!(a.dialogs.is_empty());
    assert!(std::fs::read_to_string(&copy).unwrap().contains("-- more"));

    // Save anyway writes pgtc's file, and does not ask again in this tab
    key(&mut a, "Ctrl+W");
    open_table_template(&mut a);
    typing(&mut a, "-- changed\n");
    key(&mut a, "Ctrl+S");
    key(&mut a, "s");
    assert!(a.dialogs.is_empty());
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("-- changed")
    );
    typing(&mut a, "-- again\n");
    key(&mut a, "Ctrl+S");
    assert!(a.dialogs.is_empty());
    assert!(std::fs::read_to_string(&path).unwrap().contains("-- again"));
    // what was written is still the same template
    let file = devtemplate::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let original = devtemplate::parse(&before).unwrap();
    assert_eq!(file.params, original.params);
    assert_eq!(file.name, original.name);
}

#[test]
fn save_as_asks_before_overwriting_another_file() {
    let (mut a, _d) = app_with_templates();
    let path = open_table_template(&mut a);
    let other = path.with_file_name("drop_table.pgtemplate");
    let before = std::fs::read_to_string(&other).unwrap();
    a.run(Command::SaveFileAs);
    assert!(matches!(top(&a).kind, DialogKind::TemplateSave { .. }));
    a.dialogs.last_mut().unwrap().set_text("file", "drop_table");
    key(&mut a, "Enter");
    assert!(
        matches!(top(&a).kind, DialogKind::ConfirmTemplateOverwrite { .. }),
        "{}",
        message(&a)
    );
    key(&mut a, "n");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), before);
    assert_eq!(a.template().unwrap().path.as_deref(), Some(path.as_path()));
    // a wrong version folder is refused
    a.run(Command::SaveFileAs);
    a.dialogs.last_mut().unwrap().set_text("folder", "sixteen");
    key(&mut a, "Enter");
    assert!(message(&a).contains("version folder"));
    key(&mut a, "Enter");
    // a new section and a version folder
    a.dialogs.last_mut().unwrap().set_text("folder", "16_plus");
    a.dialogs.last_mut().unwrap().set_text("file", "mine");
    let d = a.dialogs.last_mut().unwrap();
    let last = match &d.widgets[0].kind {
        WidgetKind::Combo { items, .. } => items.len() - 1,
        _ => panic!("the section combo"),
    };
    d.set_combo_selected("section", last);
    d.set_text("new_section", "Own/Deep");
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty(), "{}", message(&a));
    let saved = a
        .paths
        .user_templates_dir()
        .join("16_plus")
        .join("Own")
        .join("Deep")
        .join("mine.pgtemplate");
    assert!(saved.is_file());
    let t = a.template().unwrap();
    assert_eq!(
        (t.folder.as_str(), t.section.as_str()),
        ("16_plus", "Own/Deep")
    );
    // the preview is made for the first version of that folder
    assert_eq!(t.version_note(), "Preview: PostgreSQL 16 (assumed)");
}

#[test]
fn raw_header_and_the_list() {
    let (mut a, _d) = app_with_templates();
    open_table_template(&mut a);
    let params = a.template().unwrap().params.clone();
    key(&mut a, "Ctrl+T");
    let t = a.template().unwrap();
    assert!(t.raw && t.part == Part::Header);
    // the header of the file, as it was written by hand
    assert!(t.header.text().contains("name        = \"Create table\""));
    assert_eq!(
        a.context(),
        Some(Context::Editor),
        "the header is a text now"
    );
    assert_eq!(a.checked(Command::TemplateRawHeader), Some(true));
    // typing goes into the header
    key(&mut a, "Ctrl+End");
    typing(&mut a, "[[param]]");
    assert!(a.template().unwrap().header.text().ends_with("[[param]]"));
    // it does not parse: the header stays as text, saving is refused
    key(&mut a, "Ctrl+T");
    assert!(a.template().unwrap().raw);
    assert!(a.status.contains("error"), "{}", a.status);
    assert!(a.template().unwrap().status().starts_with("Header:"));
    key(&mut a, "Ctrl+S");
    assert!(message(&a).starts_with("The template is not saved"));
    key(&mut a, "Enter");
    // repaired: back to the list, nothing lost
    for _ in 0.."[[param]]".len() {
        key(&mut a, "Backspace");
    }
    key(&mut a, "Ctrl+T");
    let t = a.template().unwrap();
    assert!(!t.raw);
    assert_eq!(t.params, params);
    assert_eq!(a.context(), Some(Context::Template));
}

#[test]
fn header_list_keys() {
    let (mut a, _d) = app_with_templates();
    open_table_template(&mut a);
    // from the body to the header list and around
    assert_eq!(a.template().unwrap().part, Part::Body);
    key(&mut a, "Alt+Down");
    assert_eq!(a.template().unwrap().part, Part::Preview);
    key(&mut a, "Tab");
    assert_eq!(a.template().unwrap().part, Part::Header);
    key(&mut a, "Shift+Tab");
    assert_eq!(a.template().unwrap().part, Part::Preview);
    key(&mut a, "Alt+Up");
    assert_eq!(a.template().unwrap().part, Part::Body);
    // Tab in the body indents
    let before = body_text(&a);
    key(&mut a, "Tab");
    assert_ne!(body_text(&a), before);
    key(&mut a, "Ctrl+Z");
    assert_eq!(body_text(&a), before);
    key(&mut a, "Alt+Up");
    assert!(a.template_list_focused());
    // typing does nothing in the list
    typing(&mut a, "xyz");
    assert_eq!(body_text(&a), before);
    assert!(!a.tabs[a.active_tab].modified());

    let names = |a: &App| -> Vec<String> {
        a.template()
            .unwrap()
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect()
    };
    let first = names(&a);
    key(&mut a, "Home");
    assert!(
        !a.is_enabled(Command::TemplateDeleteParam),
        "on the name row"
    );
    key(&mut a, "Down");
    key(&mut a, "Down");
    assert_eq!(a.template().unwrap().selected_param(), Some(0));
    assert!(!a.is_enabled(Command::TemplateParamUp));
    key(&mut a, "Ctrl+Down");
    assert_eq!(names(&a)[1], first[0]);
    assert_eq!(a.template().unwrap().selected_param(), Some(1));
    key(&mut a, "Shift+Up");
    assert_eq!(names(&a), first);
    assert!(a.tabs[a.active_tab].modified());
    // Delete asks
    key(&mut a, "Delete");
    assert!(matches!(
        top(&a).kind,
        DialogKind::ConfirmTemplateParamDelete { index: 0, .. }
    ));
    key(&mut a, "n");
    assert_eq!(names(&a), first);
    key(&mut a, "Delete");
    key(&mut a, "y");
    assert_eq!(names(&a), first[1..]);
    // the checks notice that the body still uses it
    assert!(
        a.template()
            .unwrap()
            .status()
            .starts_with(&format!("Not declared: {}", first[0])),
        "{}",
        a.template().unwrap().status()
    );
    // the preview is hidden and shown from the list too
    key(&mut a, "Ctrl+R");
    assert!(!a.template().unwrap().preview_visible);
    key(&mut a, "Alt+Shift+Down");
    let t = a.template().unwrap();
    assert!(t.preview_visible && t.preview_percent == 45);
    // the context menu
    key(&mut a, "F2");
    assert!(a.popup.is_some());
    esc(&mut a);
    // F1 lists the keys of the list
    let help = a.key_help_lines().lines.join("\n");
    assert!(help.contains("Template header"), "{help}");
    assert!(help.contains("Add a parameter"), "{help}");
    assert!(help.contains("Test the template"), "{help}");
    key(&mut a, "Tab");
    let help = a.key_help_lines().lines.join("\n");
    assert!(help.contains("Template body"), "{help}");
    assert!(help.contains("Helper menu of the template body"), "{help}");
    assert!(!help.contains("Execute the selection"), "{help}");
}

#[test]
fn list_parameter_with_fields() {
    let (mut a, _d) = app();
    a.new_template_tab(String::new());
    key(&mut a, "Insert");
    typing(&mut a, "cols");
    let list = ParamType::ALL
        .iter()
        .position(|t| *t == ParamType::List)
        .unwrap();
    a.dialogs
        .last_mut()
        .unwrap()
        .set_combo_selected("type", list);
    a.sync_param_dialog();
    // without fields it is refused
    key(&mut a, "Ctrl+Enter");
    assert!(message(&a).contains("needs `fields`"), "{}", message(&a));
    key(&mut a, "Enter");
    // Add opens the dialog of a field, over the parameter's
    key(&mut a, "Alt+A");
    assert!(matches!(
        top(&a).kind,
        DialogKind::TemplateParam { field: true, .. }
    ));
    assert_eq!(a.dialogs.len(), 2);
    typing(&mut a, "col");
    let ident = ParamType::ALL
        .iter()
        .position(|t| *t == ParamType::Identifier)
        .unwrap();
    a.dialogs
        .last_mut()
        .unwrap()
        .set_combo_selected("type", ident);
    key(&mut a, "Ctrl+Enter");
    assert_eq!(a.dialogs.len(), 1, "{}", message(&a));
    assert_eq!(list_items(top(&a), "fields").len(), 1);
    key(&mut a, "Alt+A");
    typing(&mut a, "kind");
    key(&mut a, "Ctrl+Enter");
    let fields = list_items(top(&a), "fields");
    assert!(
        fields[0].starts_with("col ") && fields[1].starts_with("kind "),
        "{fields:?}"
    );
    // Up moves the selected (last added) field
    key(&mut a, "Alt+U");
    assert!(list_items(top(&a), "fields")[0].starts_with("kind "));
    // Esc in a field's dialog keeps the parameter's
    key(&mut a, "Alt+E");
    assert_eq!(a.dialogs.len(), 2);
    esc(&mut a);
    assert_eq!(a.dialogs.len(), 1);
    key(&mut a, "Ctrl+Enter");
    assert!(a.dialogs.is_empty(), "{}", message(&a));
    let t = a.template().unwrap();
    let p = &t.params[0];
    assert_eq!(p.kind, ParamType::List);
    let names: Vec<&str> = p.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["kind", "col"]);
    assert_eq!(p.fields[1].kind, ParamType::Identifier);

    // inserting it writes the whole loop
    key(&mut a, "Tab");
    a.run(Command::TemplateInsertParam);
    key(&mut a, "Enter");
    assert_eq!(
        body_text(&a),
        "{% for row in cols %}\n    {{ row.kind }} {{ row.col|ident }}{% if not loop.last %},{% endif %}\n\n{% endfor %}\n"
    );
    assert_eq!(a.template().unwrap().preview.text(), "    kind col\n");
}

#[test]
fn helpers_insert_into_the_body() {
    let (mut a, _d) = app();
    a.new_template_tab(String::new());
    key(&mut a, "Tab");
    typing(&mut a, "SELECT 42;");
    // Make parameter from the selection `42`
    assert!(
        !a.is_enabled(Command::TemplateMakeParam),
        "nothing selected"
    );
    a.editor().set_cursor(7, false);
    a.editor().set_cursor(9, true);
    assert!(a.is_enabled(Command::TemplateMakeParam));
    a.run(Command::TemplateMakeParam);
    assert_eq!(top(&a).text("default"), Some("42"));
    typing(&mut a, "answer");
    key(&mut a, "Ctrl+Enter");
    assert_eq!(body_text(&a), "SELECT {{ answer }};");
    let t = a.template().unwrap();
    assert_eq!(t.params[0].default_text(), "42");
    assert_eq!(t.preview.text(), "SELECT 42;\n");
    assert_eq!(t.part, Part::Body);

    // a filter goes after the value of the {{ }} the cursor is in
    a.editor().set_cursor(12, false);
    a.run(Command::TemplateInsertFilter);
    let items = list_items(top(&a), "items");
    assert_eq!(items[0], "ident");
    key(&mut a, "Enter");
    assert_eq!(body_text(&a), "SELECT {{ answer|ident }};");
    assert_eq!(a.template().unwrap().preview.text(), "SELECT \"42\";\n");

    // a construct wraps the selection; the cursor is where the condition goes
    key(&mut a, "Ctrl+A");
    a.run(Command::TemplateInsertConstruct);
    let items = list_items(top(&a), "items");
    assert_eq!(items[0], "if … endif");
    assert!(items.iter().any(|i| i == "has(\"Merge\")"), "{items:?}");
    key(&mut a, "Enter");
    assert_eq!(
        body_text(&a),
        "{% if  %}\nSELECT {{ answer|ident }};\n{% endif %}\n"
    );
    typing(&mut a, "answer");
    assert_eq!(a.template().unwrap().preview.text(), "SELECT \"42\";\n");
    // a feature check, at the end
    key(&mut a, "Ctrl+End");
    a.run(Command::TemplateInsertConstruct);
    let at = list_items(top(&a), "items")
        .iter()
        .position(|i| i == "has(\"Merge\")")
        .unwrap();
    a.dialogs.last_mut().unwrap().list_select("items", at);
    a.sync_insert_dialog();
    let note = top(&a)
        .widgets
        .iter()
        .find(|w| w.id == "note")
        .unwrap()
        .label
        .text
        .clone();
    assert!(note.contains("PostgreSQL 15"), "{note}");
    key(&mut a, "Enter");
    assert!(body_text(&a).ends_with("{% if has(\"Merge\") %}\n\n{% endif %}\n"));
    assert!(a.templates.insert.is_empty());
}

#[test]
fn test_opens_the_real_form_and_inserts_nothing() {
    let (mut a, _d) = app_with_templates();
    open_table_template(&mut a);
    let body = body_text(&a);
    key(&mut a, "F5");
    let form = a.templates.form.as_ref().expect("the form");
    assert_eq!(form.title, "Test: Create table");
    // no database is selected in the tree: the lists say so
    let failed = form.fields.iter().any(|f| {
        matches!(
            &f.value,
            crate::template_dialog::FieldValue::Choice {
                state: crate::template_dialog::ChoiceState::Failed(e),
                ..
            } if e.contains("object tree")
        )
    });
    assert!(failed, "the choice lists cannot be read");
    assert!(a.template_lookups.is_empty());
    esc(&mut a);
    assert!(a.templates.form.is_none());
    assert_eq!(body_text(&a), body);
    assert!(!a.tabs[a.active_tab].modified());

    // a template with an error is not tested
    key(&mut a, "Ctrl+End");
    typing(&mut a, "{% endfor %}");
    key(&mut a, "F5");
    assert!(a.templates.form.is_none());
    assert!(message(&a).starts_with("The template has an error"));
}

#[test]
fn test_closes_with_ok_without_inserting() {
    let (mut a, _d) = app();
    a.new_template_tab(String::new());
    key(&mut a, "Tab");
    typing(&mut a, "SELECT 1;");
    key(&mut a, "F5");
    assert!(a.templates.form.is_some());
    a.form_event(FormEvent::Ok);
    assert!(a.templates.form.is_none());
    assert_eq!(body_text(&a), "SELECT 1;");
    assert!(a.status.contains("nothing was inserted"));
}

#[test]
fn variant_for_a_server_version() {
    let (mut a, _d) = app_with_templates();
    // a new template has no file yet
    a.new_template_tab(String::new());
    a.run(Command::TemplateAddVariant);
    assert!(message(&a).starts_with("Save the template first"));
    key(&mut a, "Enter");
    key(&mut a, "Ctrl+W");

    let path = open_table_template(&mut a);
    let tabs = a.tabs.len();
    a.run(Command::TemplateAddVariant);
    assert!(matches!(top(&a).kind, DialogKind::TemplateVariant { .. }));
    typing(&mut a, "x");
    key(&mut a, "Enter");
    assert!(message(&a).contains("major version"));
    key(&mut a, "Enter");
    key(&mut a, "Backspace");
    typing(&mut a, "16");
    key(&mut a, "Enter");
    assert_eq!(a.tabs.len(), tabs + 1);
    let variant = a
        .paths
        .user_templates_dir()
        .join("16_plus")
        .join("Tables")
        .join("table.pgtemplate");
    let t = a.template().unwrap();
    assert_eq!(t.path.as_deref(), Some(variant.as_path()));
    assert_eq!(t.folder, "16_plus");
    assert_eq!(t.version_note(), "Preview: PostgreSQL 16 (assumed)");
    // not written until it is saved
    assert!(!variant.exists());
    assert!(a.tabs[a.active_tab].modified());
    key(&mut a, "Ctrl+S");
    assert!(a.dialogs.is_empty(), "{}", message(&a));
    assert!(variant.is_file());
    // the same template; the editor writes LF, a checkout on Windows may have CRLF
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap().replace("\r\n", "\n");
    assert_eq!(
        devtemplate::parse(&read(&variant)).unwrap(),
        devtemplate::parse(&read(&path)).unwrap()
    );
    // asked again from the first tab: the tab of the variant is shown
    a.active_tab = tabs - 1;
    a.run(Command::TemplateAddVariant);
    typing(&mut a, "16");
    key(&mut a, "Enter");
    assert_eq!(a.tabs.len(), tabs + 1);
    assert_eq!(a.template().unwrap().folder, "16_plus");
}

#[test]
fn quitting_asks_about_a_changed_template() {
    let (mut a, _d) = app();
    a.new_template_tab(String::new());
    key(&mut a, "Tab");
    typing(&mut a, "SELECT 1;");
    key(&mut a, "F10");
    assert!(matches!(top(&a).kind, DialogKind::QuitSaveChanges { .. }));
    // Yes: where to save is asked; saved, then pgtc quits
    key(&mut a, "y");
    assert!(matches!(top(&a).kind, DialogKind::TemplateSave { .. }));
    a.dialogs.last_mut().unwrap().set_text("file", "one");
    key(&mut a, "Enter");
    assert!(a.dialogs.is_empty(), "{}", message(&a));
    assert!(
        a.paths
            .user_templates_dir()
            .join("default")
            .join("one.pgtemplate")
            .is_file()
    );
    assert!(a.should_quit());
}

#[test]
fn template_tab_is_drawn() {
    let (mut a, _d) = app_with_templates();
    open_table_template(&mut a);
    a.sync_template_tab();
    let screen = screen_text_sized(&a, 120, 34);
    assert!(screen.contains("Template: Create table"), "{screen}");
    assert!(screen.contains("table.pgtemplate"), "{screen}");
    assert!(screen.contains("Name:        Create table"), "{screen}");
    assert!(screen.contains(" Parameters "), "{screen}");
    assert!(screen.contains(" Add "), "{screen}");
    assert!(screen.contains("Preview: PostgreSQL"), "{screen}");
    assert!(screen.contains("No problems found"), "{screen}");
    assert!(screen.contains("CREATE TABLE"), "{screen}");
    // narrow: the header is above the body; raw: the header is text
    let narrow = screen_text_sized(&a, 66, 30);
    assert!(narrow.contains("Name:        Create table"), "{narrow}");
    assert!(narrow.contains("CREATE TABLE"), "{narrow}");
    key(&mut a, "Ctrl+T");
    let screen = screen_text_sized(&a, 120, 34);
    assert!(
        screen.contains("name        = \"Create table\""),
        "{screen}"
    );
}

#[test]
fn mouse_in_a_template_tab() {
    let (mut a, _d) = app_with_templates();
    open_table_template(&mut a);
    a.sync_template_tab();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 34)).unwrap();
    let mut hits = Hits::default();
    terminal
        .draw(|f| crate::view::render(f, &a, &mut hits))
        .unwrap();
    let h = hits.template_tab.clone();
    assert!(h.body.width > 0 && h.preview.width > 0 && h.header.width > 0);
    let click = |a: &mut App, hits: &Hits, x: u16, y: u16| {
        a.update(
            Msg::Input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })),
            hits,
        );
    };
    // a row of the list
    let (row, area) = h.rows[3];
    click(&mut a, &hits, area.x + 2, area.y);
    let t = a.template().unwrap();
    assert_eq!((t.part, t.cursor), (Part::Header, row));
    // the preview and the body take the focus
    click(&mut a, &hits, h.preview.x + 1, h.preview.y);
    assert_eq!(a.template().unwrap().part, Part::Preview);
    click(&mut a, &hits, h.body.x + 6, h.body.y + 1);
    assert_eq!(a.template().unwrap().part, Part::Body);
    assert_eq!(a.editor().cursor_line_col().0, 1);
    // the Add button
    let (_, add) = h.buttons[0];
    click(&mut a, &hits, add.x + 1, add.y);
    assert!(matches!(top(&a).kind, DialogKind::TemplateParam { .. }));
}

/// Test (D-074) with a connected database selected in the tree: the choice lists come
/// through the tree's connection to it, the version is the server's.
#[tokio::test]
async fn test_reads_choice_lists_through_the_tree_connection() {
    use crate::objects::ObjType;
    use crate::template_dialog::{ChoiceState, FieldValue};
    use crate::tree::{DbConn, NodeKind, RowKind};
    use pgc_db::NodeRow;
    let (mut a, _d) = app_with_templates();
    let uid = add_server(&mut a, "srv");
    fake_connecting(&mut a, uid);
    a.server_connected(uid, pgc_core::ServerVersion(160004));
    a.tree.by_uid(uid).unwrap().handle = Some(pgc_db::ConnectionHandle::detached(50));
    let dbs = a.tree.servers[0]
        .children
        .iter()
        .find(|n| n.kind == NodeKind::Group(ObjType::Database))
        .unwrap()
        .nid;
    a.tree.set_children(
        dbs,
        vec![NodeRow {
            oid: 5,
            name: "shop".into(),
            label: None,
            partitioned: false,
        }],
    );
    a.tree.node_mut(dbs).unwrap().expanded = true;
    let db = a.tree.node(dbs).unwrap().children.as_ref().unwrap()[0].nid;
    a.next_conn += 1;
    let conn = a.next_conn;
    a.connections.insert(conn, Target::Database(uid, db));
    a.tree.node_mut(db).unwrap().conn = Some(DbConn {
        state: ConnState::Connected,
        handle: Some(pgc_db::ConnectionHandle::detached(conn)),
    });
    a.tree.select_kind(RowKind::Node(db));

    open_table_template(&mut a);
    a.sync_template_tab();
    let t = a.template().unwrap();
    assert_eq!(t.version_note(), "Preview: PostgreSQL 16");
    key(&mut a, "F5");
    let form = a.templates.form.as_ref().expect("the form");
    assert_eq!(form.ctx.database, "shop");
    assert_eq!(form.ctx.version, pgc_core::ServerVersion(160004));
    // the lists are asked for on the tree's connection; their answers fill the form
    assert!(!a.template_lookups.is_empty());
    let (key_of, _) = a
        .template_lookups
        .iter()
        .find(|(_, t)| matches!(t, LookupTarget::Field(name, _) if name == "schema"))
        .map(|(k, t)| (*k, t.clone()))
        .expect("the lookup of the schema list");
    a.on_db(DbEvent::Lookup {
        id: conn,
        key: key_of,
        result: Ok(vec![vec!["public".to_owned()], vec!["sales".to_owned()]]),
    });
    let form = a.templates.form.as_ref().unwrap();
    let schema = form
        .fields
        .iter()
        .find(|f| f.param.name == "schema")
        .unwrap();
    match &schema.value {
        FieldValue::Choice { items, state, .. } => {
            assert_eq!(*state, ChoiceState::Ready);
            assert_eq!(items.len(), 2);
        }
        other => panic!("{other:?}"),
    }
    esc(&mut a);
    assert!(
        a.template_lookups.is_empty(),
        "the other answers are not waited for"
    );
}

/// Autocomplete in the body (D-074): the template's own names, filters and
/// statements; in the SQL a parameter is written as its whole mark.
#[test]
fn autocomplete_in_the_body() {
    let (mut a, _d) = app_with_templates();
    open_table_template(&mut a);
    a.editor().set_text("");
    let labels = |a: &App| -> Vec<String> {
        a.completion
            .as_ref()
            .map(|p| p.items.iter().map(|i| i.label.clone()).collect())
            .unwrap_or_default()
    };
    // in the SQL: after the third letter, the parameter as its mark
    typing(&mut a, "CREATE TABLE sc");
    assert!(a.completion.is_none());
    typing(&mut a, "h");
    assert_eq!(labels(&a), ["schema", "SCHEMA", "SCHEMAS"]);
    key(&mut a, "Enter");
    assert!(a.completion.is_none());
    assert_eq!(body_text(&a), "CREATE TABLE {{ schema|ident }}");
    // in a mark: from the first letter; Tab takes the entry too
    typing(&mut a, ".{{ n");
    assert_eq!(labels(&a), ["name", "none", "not"]);
    key(&mut a, "Tab");
    assert_eq!(body_text(&a), "CREATE TABLE {{ schema|ident }}.{{ name");
    // filters after `|`, narrowed by typing
    typing(&mut a, "|");
    assert!(labels(&a).contains(&"qtLiteral".to_owned()));
    typing(&mut a, "id");
    assert_eq!(labels(&a), ["ident"]);
    key(&mut a, "Enter");
    typing(&mut a, " }}\n");
    assert!(body_text(&a).ends_with(".{{ name|ident }}\n"));
    // statements right after `{%`, by key
    typing(&mut a, "{% ");
    assert!(a.completion.is_none());
    key(&mut a, "Ctrl+Space");
    assert_eq!(labels(&a)[..2], ["if", "elif"]);
    // Esc closes only the list
    key(&mut a, "Esc");
    assert!(a.completion.is_none());
    assert!(a.template().is_some());
    typing(&mut a, "f");
    assert_eq!(labels(&a), ["for"]);
    key(&mut a, "Enter");
    typing(&mut a, "c in col");
    key(&mut a, "Ctrl+Space");
    assert_eq!(labels(&a), ["columns"]);
    key(&mut a, "Enter");
    typing(&mut a, " %}{{ c.");
    // the fields of the list the loop goes over
    assert_eq!(
        labels(&a),
        [
            "col_name",
            "col_type",
            "not_null",
            "primary_key",
            "col_default"
        ]
    );
    typing(&mut a, "col_t");
    assert_eq!(labels(&a), ["col_type"]);
    key(&mut a, "Enter");
    assert!(body_text(&a).ends_with("{% for c in columns %}{{ c.col_type"));
    // feature names in has("…")
    typing(&mut a, " }}{% if has(\"");
    assert!(labels(&a).contains(&"NullsNotDistinct".to_owned()));
    typing(&mut a, "Mer");
    assert_eq!(labels(&a), ["Merge"]);
    key(&mut a, "Enter");
    assert!(body_text(&a).ends_with("has(\"Merge"));
    // F1 lists the key; in the header list there is no list
    let help = a.key_help_lines().lines.join("\n");
    assert!(help.contains("Autocomplete"), "{help}");
    key(&mut a, "Alt+Up");
    key(&mut a, "Ctrl+Space");
    assert!(a.completion.is_none());
}
