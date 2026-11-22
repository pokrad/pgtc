//! Parts of the application state that belong together, kept out of the long field
//! list of [`App`](super::App). Each is changed only by the handlers of its area.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use pgc_core::places::Place;
use ratatui::layout::{Position, Rect};

use super::closing::Bulk;
use super::templates::Picker;
use crate::template_dialog::{Choice, TemplateForm};

/// What the last frame drew, and where: the sizes the keys page by and the areas the
/// mouse and the context menus need.
#[derive(Debug, Default)]
pub(crate) struct LastFrame {
    /// The whole screen (sizes the Help windows).
    pub(crate) screen: Rect,
    /// Rows of the tree pane.
    pub(super) tree_height: usize,
    /// Text area of the active editor (for mouse clicks).
    pub(super) editor_area: Rect,
    /// Grid area of the active tab (anchor of the results context menu), and its size
    /// (rows incl. header, columns).
    pub(super) grid_area: Rect,
    pub(super) grid_size: (usize, usize),
    /// The parts of the active template tab (D-074).
    pub(super) template: crate::template_tab::TplHits,
}

/// Mouse actions that span several events.
#[derive(Debug, Default)]
pub(super) struct MouseState {
    /// The editor / results split is being dragged (D-019).
    pub(super) split_drag: bool,
    /// The tree / tabs border is being dragged (D-045).
    pub(super) pane_drag: bool,
    /// Time and place of the last left click, for double clicks (D-067).
    pub(super) last_click: Option<(Instant, Position)>,
}

/// Where the file dialogs are (D-070); remembered while pgtc runs.
#[derive(Debug)]
pub(super) struct FileDialogState {
    /// Directory shown in the file dialogs.
    pub(super) dir: PathBuf,
    /// The dialogs show the places instead of `dir` (empty until they are read).
    pub(super) places: Option<Vec<Place>>,
    /// Number of the newest request to the background (D-072); an answer with an
    /// older number is not waited for any more.
    pub(super) seq: u64,
    /// The listing the open dialog waits for.
    pub(super) listing: Option<u64>,
    /// The chosen path that is being checked.
    pub(super) probing: Option<u64>,
    /// Height of the open dialog's listing (a resize while it shows *Reading…* would
    /// otherwise leave it one row high).
    pub(super) list_rows: u16,
    /// The tab the open Save as dialog was opened for: its file is written, whichever
    /// tab is the active one when the dialog is done (a tab may open behind it).
    pub(super) save_tab: Option<u64>,
}

/// What a tab that is being closed waits for (D-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TabWait {
    /// Its text is being saved ("Save changes?" → Yes); then it closes.
    Save,
    /// Its COMMIT is on its way ("open transaction" → Commit); committed, it closes.
    Commit,
}

/// Where closing many tabs at once is (D-043).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BulkStage {
    /// The questions: about unsaved text tab by tab, then about open transactions.
    Asking,
    /// This tab is being saved; the questions go on when it is.
    Saving(u64),
    /// The COMMITs of these tabs are on their way; all answered, the tabs close.
    Committing(Vec<u64>),
}

/// Quitting, or disconnecting / deleting / reconnecting a server (D-043).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BulkClose {
    pub(super) what: Bulk,
    /// The tabs whose unsaved text the user chose not to save.
    pub(super) skip: Vec<u64>,
    pub(super) stage: BulkStage,
}

/// Closing tabs, quitting, disconnecting (D-019, D-043): what is on its way and what
/// it waits for. The one place that says so — a tab does not keep a note of its own.
/// Its state changes only through the methods below; the handlers call them when a
/// save or a run ends, a session is lost or a question is given up.
#[derive(Debug, Default)]
pub(super) struct Closing {
    /// Single tabs that close once what they wait for has happened.
    tabs: Vec<(u64, TabWait)>,
    /// Closing many tabs at once.
    pub(super) bulk: Option<BulkClose>,
    /// An edited connection being connected again (D-035): its uid and the objects
    /// down to the tree row that was selected, to open the tree to it again.
    pub(super) reopen: Option<(u64, Vec<(crate::objects::ObjType, u32)>)>,
    /// Quitting waits for profilers to put the logging settings back (D-058).
    pub(super) quit_after_restore: bool,
    /// The event loop ends.
    pub(super) quit: bool,
}

impl Closing {
    /// Tab `tab` closes once `what` has happened.
    pub(super) fn wait(&mut self, tab: u64, what: TabWait) {
        self.tabs.retain(|(t, _)| *t != tab);
        self.tabs.push((tab, what));
    }

    /// `what` has happened to tab `tab`: true when the tab waited for it (it waits
    /// no longer).
    pub(super) fn waited(&mut self, tab: u64, what: TabWait) -> bool {
        let before = self.tabs.len();
        self.tabs.retain(|w| *w != (tab, what));
        self.tabs.len() != before
    }

    /// Closing many tabs is at its questions for `what`: starts it, or goes on with
    /// it (the tabs not to save are remembered while it is the same thing).
    pub(super) fn ask(&mut self, what: Bulk) -> &mut BulkClose {
        let skip = match self.bulk.take() {
            Some(b) if b.what == what => b.skip,
            _ => Vec::new(),
        };
        self.bulk.insert(BulkClose {
            what,
            skip,
            stage: BulkStage::Asking,
        })
    }

    /// Closing many tabs waits for the save of `tab`.
    pub(super) fn bulk_saving(&mut self, tab: u64) {
        if let Some(b) = &mut self.bulk {
            b.stage = BulkStage::Saving(tab);
        }
    }

    /// Tab `tab` was saved: what closing many tabs goes on with, when it waited for
    /// that save.
    pub(super) fn bulk_saved(&mut self, tab: u64) -> Option<Bulk> {
        match &mut self.bulk {
            Some(b) if b.stage == BulkStage::Saving(tab) => {
                b.stage = BulkStage::Asking;
                Some(b.what)
            }
            _ => None,
        }
    }

    /// The questions of closing many tabs are answered: what it is, and it waits for
    /// the COMMITs of `tabs` now. `None` when it is not at its questions.
    pub(super) fn commit(&mut self, tabs: Vec<u64>) -> Option<Bulk> {
        match &mut self.bulk {
            Some(b) if b.stage == BulkStage::Asking => {
                b.stage = BulkStage::Committing(tabs);
                Some(b.what)
            }
            _ => None,
        }
    }

    /// What closing many tabs is, while it waits for COMMITs.
    pub(super) fn committing(&self) -> Option<Bulk> {
        match &self.bulk {
            Some(b) if matches!(b.stage, BulkStage::Committing(_)) => Some(b.what),
            _ => None,
        }
    }

    /// Closing many tabs still waits for the COMMIT of `tab`.
    pub(super) fn commit_awaited(&self, tab: u64) -> bool {
        matches!(&self.bulk, Some(BulkClose { stage: BulkStage::Committing(tabs), .. })
            if tabs.contains(&tab))
    }

    /// The COMMIT of `tab` was answered, or will not be: true when closing many tabs
    /// waited for it.
    pub(super) fn commit_answered(&mut self, tab: u64) -> bool {
        match &mut self.bulk {
            Some(BulkClose {
                stage: BulkStage::Committing(tabs),
                ..
            }) => {
                let before = tabs.len();
                tabs.retain(|t| *t != tab);
                tabs.len() != before
            }
            _ => false,
        }
    }

    /// A save for closing was given up or has failed: nothing waits for a save any
    /// more (the tabs stay open, closing many tabs stops).
    pub(super) fn saves_off(&mut self) {
        self.tabs.retain(|(_, w)| *w != TabWait::Save);
        if matches!(&self.bulk, Some(b) if matches!(b.stage, BulkStage::Saving(_))) {
            self.bulk = None;
        }
    }

    /// The questions of closing many tabs were given up (`Cancel`, `Esc`).
    pub(super) fn questions_off(&mut self) {
        if matches!(&self.bulk, Some(b) if b.stage == BulkStage::Asking) {
            self.bulk = None;
        }
    }

    /// A COMMIT that closing many tabs waited for did not commit, or its session is
    /// gone: closing many tabs stops.
    pub(super) fn commits_off(&mut self) {
        if self.committing().is_some() {
            self.bulk = None;
        }
    }

    /// No single tab waits for anything.
    #[cfg(test)]
    pub(super) fn nothing_waits(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Tab `tab` is closed: nothing waits for it.
    pub(super) fn forget_tab(&mut self, tab: u64) {
        self.tabs.retain(|(t, _)| *t != tab);
        self.commit_answered(tab);
    }
}

/// `Esc` waiting for a digit (D-032): the number of the wait, so that an older
/// timeout is not taken for it.
#[derive(Debug, Default)]
pub(super) struct EscState {
    pub(super) seq: u64,
    pub(super) pending: bool,
}

/// Developer templates (D-046).
#[derive(Default)]
pub struct TemplateState {
    /// The open form.
    pub form: Option<TemplateForm>,
    /// The picker's state while its dialog is open.
    pub(super) picker: Option<Picker>,
    /// Choice lists read for the rows of list parameters: (list, field) → choices.
    pub(super) row_choices: HashMap<(String, String), Vec<Choice>>,
    /// The row being edited: (list field, row; `None` = a new row).
    pub(super) row_edit: Option<(usize, Option<usize>)>,
    /// The template manager's state while its dialog is open (D-074).
    pub(super) manager: Option<super::template_editor::Manager>,
    /// The parameter being edited in a template tab's parameter dialog.
    pub(super) param_edit: Option<super::template_editor::ParamEdit>,
    /// What the rows of the open insert dialog put into the body: (text, note).
    pub(super) insert: Vec<(String, String)>,
}

/// A request on its way.
pub(super) struct AiRun {
    pub(super) id: u64,
    /// Query tab the answer goes into.
    pub(super) tab: u64,
    pub(super) prompt: String,
    pub(super) started: Instant,
    pub(super) abort: Option<tokio::task::AbortHandle>,
    /// Where the answer belongs: the cursor and the selection of the editor when the
    /// request was sent (the model was told its answer replaces that selection).
    pub(super) place: AiPlace,
}

/// The place in the editor an AI request was sent for (D-073).
pub(super) struct AiPlace {
    pub(super) cursor: usize,
    pub(super) selection: Option<(usize, usize)>,
    pub(super) selected: Option<String>,
    /// The editor's revision then: the same now means nothing was edited meanwhile.
    pub(super) revision: u64,
}

/// The AI assistant (D-073).
#[derive(Default)]
pub(super) struct AiState {
    pub(super) sink: Option<super::ai::AiSink>,
    pub(super) next_id: u64,
    pub(super) run: Option<AiRun>,
    /// The structure read for the open `F8` dialog of query tab `.0`, or why it
    /// could not be read.
    pub(super) structure: Option<(u64, Result<crate::ai::Structure, String>)>,
    /// The scope the dialog's size was computed for, the characters a request sends
    /// with it and an empty prompt, and the note about the editor text.
    pub(super) size: Option<(usize, usize, &'static str)>,
    /// The prompt of a dialog that was closed without sending: there again in the
    /// next one.
    pub(super) draft: String,
    /// Prompts of this session, oldest first, and the one shown (`↑` / `↓`).
    pub(super) history: Vec<String>,
    pub(super) history_pos: Option<usize>,
    /// The preset the AI settings dialog last filled in.
    pub(super) preset_seen: usize,
    /// The AI settings dialog still shows the saved key as stars, untouched.
    pub(super) key_stars: bool,
    /// The stars were taken away because the dialog shows another provider's URL
    /// (the key is the saved provider's); they come back with the saved URL.
    pub(super) key_hidden: bool,
    /// The open AI settings dialog: the provider's models as read, and the chosen
    /// model (its id; may be one typed by hand).
    pub(super) form_models: Vec<pgc_core::settings::AiModel>,
    pub(super) form_model: String,
    /// The entry of the model combo that was last looked at.
    pub(super) model_seen: usize,
    /// Protocol, base URL and key field the model list was last asked for.
    pub(super) fetch_seen: Option<(usize, String, String)>,
    /// Without a sink (tests): the model list request that would have been sent.
    pub(super) unsent_models: Option<crate::ai::Request>,
    /// The autocomplete list of the prompt box.
    pub(super) completion: Option<crate::completion::Popup>,
    /// Without a sink (tests): the request that would have been sent.
    pub(super) unsent: Option<crate::ai::Request>,
}
