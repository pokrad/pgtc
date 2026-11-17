//! Parts of the application state that belong together, kept out of the long field
//! list of [`App`](super::App). Each is changed only by the handlers of its area.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use pgc_core::places::Place;
use ratatui::layout::{Position, Rect};

use super::closing::{AfterSave, Bulk};
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
}

/// Closing tabs, quitting, disconnecting (D-019, D-043): the questions on their way
/// and what is waited for.
#[derive(Debug, Default)]
pub(super) struct Closing {
    /// What to do once a "Save changes?" save has succeeded.
    pub(super) after_save: Option<AfterSave>,
    /// Quitting / disconnecting: what, and the tabs whose unsaved text the user chose
    /// not to save.
    pub(super) bulk: Option<(Bulk, Vec<u64>)>,
    /// Quitting / disconnecting waits for COMMITs to finish.
    pub(super) bulk_pending: Option<Bulk>,
    /// Quitting waits for profilers to put the logging settings back (D-058).
    pub(super) quit_after_restore: bool,
    /// The event loop ends.
    pub(super) quit: bool,
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
}

/// A request on its way.
pub(super) struct AiRun {
    pub(super) id: u64,
    /// Query tab the answer goes into.
    pub(super) tab: u64,
    pub(super) prompt: String,
    pub(super) abort: Option<tokio::task::AbortHandle>,
}

/// The AI assistant (D-073).
#[derive(Default)]
pub(super) struct AiState {
    pub(super) sink: Option<super::ai::AiSink>,
    pub(super) next_id: u64,
    pub(super) run: Option<AiRun>,
    /// The structure read for the open `F8` dialog of query tab `.0`.
    pub(super) structure: Option<(u64, crate::ai::Structure)>,
    /// The scope the dialog's info line was computed for.
    pub(super) scope_seen: Option<usize>,
    /// Prompts of this session, oldest first, and the one shown (`↑` / `↓`).
    pub(super) history: Vec<String>,
    pub(super) history_pos: Option<usize>,
    /// The preset the AI settings dialog last filled in.
    pub(super) preset_seen: usize,
    /// Without a sink (tests): the request that would have been sent.
    pub(super) unsent: Option<crate::ai::Request>,
}
