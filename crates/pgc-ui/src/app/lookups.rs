//! Lookups through a query tab's connection and their answers: choice lists of the
//! template form (D-046), autocomplete names (D-052), tables for the generate commands
//! (D-054).

use pgc_db::DbError;
use pgc_sql::devtemplate;

use super::App;

/// What a template lookup fills: a choice field, or a choice field of list rows
/// (list name, field name); how the rows become values.
#[derive(Debug, Clone)]
pub(super) enum LookupTarget {
    Field(String, devtemplate::Shape),
    Row(String, String, devtemplate::Shape),
    /// Columns for a generate command of query tab `tab` (D-054).
    Generate {
        tab: u64,
        what: crate::psql::Gen,
        shown: String,
    },
    /// Autocomplete names of query tab `tab` (D-052).
    Names {
        tab: u64,
        generation: u64,
        need: crate::completion::Need,
    },
}

impl LookupTarget {
    /// The query tab a lookup was made for; `None` for the template form's lists.
    pub(super) fn tab(&self) -> Option<u64> {
        match self {
            LookupTarget::Generate { tab, .. } | LookupTarget::Names { tab, .. } => Some(*tab),
            LookupTarget::Field(..) | LookupTarget::Row(..) => None,
        }
    }
}

impl App {
    pub(super) fn lookup_loaded(&mut self, key: u64, result: Result<Vec<Vec<String>>, DbError>) {
        let Some(target) = self.template_lookups.remove(&key) else {
            return;
        };
        let result = result.map_err(|e| e.to_string());
        match target {
            LookupTarget::Field(name, shape) => {
                if let Some(form) = self.template.as_mut() {
                    form.set_choices(
                        &name,
                        result.map(|rows| crate::template_dialog::choices_from_rows(rows, shape)),
                    );
                }
            }
            LookupTarget::Row(list, field, shape) => {
                if let Ok(rows) = result {
                    self.row_choices.insert(
                        (list, field),
                        crate::template_dialog::choices_from_rows(rows, shape),
                    );
                }
            }
            LookupTarget::Names {
                tab,
                generation,
                need,
            } => self.names_loaded(tab, generation, need, result),
            LookupTarget::Generate { tab, what, shown } => {
                self.generate_loaded(tab, what, &shown, result)
            }
        }
    }

    /// The template form closed: answers for its choice lists are no longer wanted.
    /// Lookups of query tabs (autocomplete, generate commands) stay.
    pub(super) fn drop_form_lookups(&mut self) {
        self.template_lookups.retain(|_, t| t.tab().is_some());
    }

    /// The connection of query tab `tab` is gone: answers to its lookups will not
    /// come, and autocomplete reads its names again on the next connection (D-052).
    pub(super) fn drop_tab_lookups(&mut self, tab: u64) {
        self.template_lookups.retain(|_, t| t.tab() != Some(tab));
        if let Some(q) = self.tab_mut(tab).and_then(|t| t.query.as_mut()) {
            q.names.clear();
        }
    }
}
