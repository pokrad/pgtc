//! Editor state (D-015, D-038): a `ropey` buffer with cursor, selection, undo/redo,
//! indentation, find/replace and a lexer-state cache for highlighting.
//!
//! Positions are char indices into the rope. Display columns expand tabs to the tab
//! width and use Unicode widths.

use std::cell::RefCell;

use regex::Regex;
use ropey::Rope;
use unicode_width::UnicodeWidthChar;

use crate::lexer::{LexState, Token, lex_line};

/// Cursor movements; each can extend the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// First non-blank character, then column 1.
    Home,
    End,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
}

/// Editing actions (keys are mapped to these by the application keymap).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Move(Motion, bool),
    Insert(String),
    Newline,
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    Indent,
    Unindent,
    SelectAll,
    Undo,
    Redo,
}

/// Find options (D-038).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_words: bool,
    pub regex: bool,
}

impl SearchOptions {
    pub fn compile(&self) -> Result<Regex, String> {
        let pattern = if self.regex {
            self.text.clone()
        } else {
            regex::escape(&self.text)
        };
        let pattern = if self.whole_words {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        let pattern = if self.case_sensitive {
            pattern
        } else {
            format!("(?i){pattern}")
        };
        Regex::new(&pattern).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone)]
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
}

#[derive(Debug, Clone)]
struct Group {
    edits: Vec<Edit>,
    before: (usize, Option<usize>),
    after: (usize, Option<usize>),
    /// Open typing group that further typed characters may join.
    typing: bool,
}

/// Lines between lexer checkpoints (D-038).
pub const CHECKPOINT_LINES: usize = 512;

/// Lexer state at the start of every `CHECKPOINT_LINES`-th line; entry `k` belongs to
/// line `k * CHECKPOINT_LINES`. Only a valid prefix is kept.
#[derive(Debug, Default)]
struct LexCache {
    checkpoints: Vec<LexState>,
}

impl LexCache {
    fn invalidate_from(&mut self, line: usize) {
        self.checkpoints.truncate(line / CHECKPOINT_LINES + 1);
    }

    /// Extends the checkpoints by up to `budget` lines; true when the whole text is done.
    fn extend(&mut self, rope: &Rope, mut budget: usize) -> bool {
        if self.checkpoints.is_empty() {
            self.checkpoints.push(LexState::Normal);
        }
        let total = rope.len_lines();
        loop {
            let k = self.checkpoints.len() - 1;
            let start = k * CHECKPOINT_LINES;
            let end = start + CHECKPOINT_LINES;
            if end >= total {
                return true;
            }
            if budget < CHECKPOINT_LINES {
                return false;
            }
            let state = lex_lines(rope, start, end, self.checkpoints[k].clone());
            self.checkpoints.push(state);
            budget -= CHECKPOINT_LINES;
        }
    }

    /// State at the start of `line`.
    fn state_at(&mut self, rope: &Rope, line: usize) -> LexState {
        let k = line / CHECKPOINT_LINES;
        while self.checkpoints.len() <= k {
            if self.extend(rope, CHECKPOINT_LINES) && self.checkpoints.len() <= k {
                break;
            }
        }
        let k = k.min(self.checkpoints.len() - 1);
        lex_lines(
            rope,
            k * CHECKPOINT_LINES,
            line,
            self.checkpoints[k].clone(),
        )
    }
}

/// Lexes lines `start..end` (state only) with a sequential line iterator.
fn lex_lines(rope: &Rope, start: usize, end: usize, mut state: LexState) -> LexState {
    let mut buf = String::new();
    for slice in rope.lines_at(start).take(end.saturating_sub(start)) {
        let text = match slice.as_str() {
            Some(s) => s,
            None => {
                buf.clear();
                for chunk in slice.chunks() {
                    buf.push_str(chunk);
                }
                buf.as_str()
            }
        };
        let text = text.strip_suffix('\n').unwrap_or(text);
        let text = text.strip_suffix('\r').unwrap_or(text);
        state = lex_line(&state, text, None);
    }
    state
}

fn trim_newline(slice: ropey::RopeSlice<'_>) -> ropey::RopeSlice<'_> {
    let mut len = slice.len_chars();
    if len > 0 && slice.char(len - 1) == '\n' {
        len -= 1;
        if len > 0 && slice.char(len - 1) == '\r' {
            len -= 1;
        }
    }
    slice.slice(..len)
}

/// Result of a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    /// The search went past the end (or start) and continued from the other side.
    pub wrapped: bool,
}

#[derive(Debug)]
pub struct Editor {
    rope: Rope,
    cursor: usize,
    anchor: Option<usize>,
    /// Display column kept while moving up / down.
    goal_col: Option<usize>,
    /// First visible line and display column.
    pub top: usize,
    pub left: usize,
    view: (usize, usize),
    undo: Vec<Group>,
    redo: Vec<Group>,
    read_only: bool,
    tab_width: usize,
    modified: bool,
    lex: RefCell<LexCache>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new("", false, 4)
    }
}

impl Editor {
    pub fn new(text: &str, read_only: bool, tab_width: usize) -> Self {
        Self {
            rope: Rope::from_str(text),
            cursor: 0,
            anchor: None,
            goal_col: None,
            top: 0,
            left: 0,
            view: (20, 80),
            undo: Vec::new(),
            redo: Vec::new(),
            read_only,
            tab_width: tab_width.max(1),
            modified: false,
            lex: RefCell::new(LexCache::default()),
        }
    }

    // ---- accessors ----------------------------------------------------------------

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn modified(&self) -> bool {
        self.modified
    }

    /// Marks the text as saved (or changed).
    pub fn set_modified(&mut self, modified: bool) {
        self.modified = modified;
    }

    pub fn tab_width(&self) -> usize {
        self.tab_width
    }

    pub fn set_tab_width(&mut self, width: usize) {
        self.tab_width = width.max(1);
    }

    /// Replaces the whole text (new document: undo history and cursor reset).
    pub fn set_text(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.cursor = 0;
        self.anchor = None;
        self.goal_col = None;
        self.top = 0;
        self.left = 0;
        self.undo.clear();
        self.redo.clear();
        self.modified = false;
        *self.lex.borrow_mut() = LexCache::default();
    }

    /// Visible size of the text area (lines, columns) for paging and scrolling.
    /// Called before every message; only a changed size brings the cursor back into
    /// view, so scrolling with the mouse wheel may leave the cursor off screen.
    pub fn set_view(&mut self, height: usize, width: usize) {
        let view = (height.max(1), width.max(1));
        if view != self.view {
            self.view = view;
            self.scroll_to_cursor();
        }
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Cursor as (line, display column), 0-based.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let line = self.rope.char_to_line(self.cursor);
        (line, self.display_col(line, self.cursor))
    }

    /// Selected char range, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| (anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection()
            .map(|(a, b)| self.rope.slice(a..b).to_string())
    }

    /// Text of `line` without its line break.
    pub fn line(&self, line: usize) -> String {
        trim_newline(self.rope.line(line)).to_string()
    }

    /// Line of char index `pos` (clamped to the text).
    pub fn line_of_char(&self, pos: usize) -> usize {
        self.rope.char_to_line(pos.min(self.rope.len_chars()))
    }

    pub fn line_to_char(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    // ---- columns ------------------------------------------------------------------

    fn char_width(&self, c: char, col: usize) -> usize {
        if c == '\t' {
            self.tab_width - col % self.tab_width
        } else {
            c.width().unwrap_or(0)
        }
    }

    /// Display column of char index `pos` on `line`.
    fn display_col(&self, line: usize, pos: usize) -> usize {
        let start = self.rope.line_to_char(line);
        let mut col = 0;
        for c in self.rope.slice(start..pos).chars() {
            col += self.char_width(c, col);
        }
        col
    }

    /// Char index on `line` closest to display column `target`.
    pub fn pos_at_col(&self, line: usize, target: usize) -> usize {
        let line = line.min(self.rope.len_lines() - 1);
        let start = self.rope.line_to_char(line);
        let text = trim_newline(self.rope.line(line));
        let mut col = 0;
        for (i, c) in text.chars().enumerate() {
            let w = self.char_width(c, col);
            if col + w > target {
                return start + i;
            }
            col += w;
        }
        start + text.len_chars()
    }

    // ---- actions ------------------------------------------------------------------

    /// Applies an action. Returns false if it was refused (edit in a read-only view).
    pub fn apply(&mut self, action: Action) -> bool {
        let editing = !matches!(action, Action::Move(..) | Action::SelectAll);
        let keeps_goal = matches!(
            action,
            Action::Move(
                Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown,
                _
            )
        );
        if editing && self.read_only {
            return false;
        }
        match action {
            Action::Move(motion, select) => self.move_cursor(motion, select),
            Action::SelectAll => {
                self.anchor = Some(0);
                self.cursor = self.rope.len_chars();
            }
            Action::Insert(text) => {
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                let typing = text.chars().count() == 1 && !text.contains('\n');
                self.replace_selection(&text, typing);
            }
            Action::Newline => self.replace_selection("\n", false),
            Action::Backspace => {
                if self.selection().is_some() {
                    self.replace_selection("", false);
                } else if self.cursor > 0 {
                    let start = self.cursor - 1;
                    self.edit_group(|ed| ed.replace(start, start + 1, ""));
                }
            }
            Action::Delete => {
                if self.selection().is_some() {
                    self.replace_selection("", false);
                } else if self.cursor < self.rope.len_chars() {
                    let at = self.cursor;
                    self.edit_group(|ed| ed.replace(at, at + 1, ""));
                }
            }
            Action::DeleteWordLeft => {
                if self.selection().is_some() {
                    self.replace_selection("", false);
                } else {
                    let start = self.word_left(self.cursor);
                    let end = self.cursor;
                    self.edit_group(|ed| ed.replace(start, end, ""));
                }
            }
            Action::DeleteWordRight => {
                if self.selection().is_some() {
                    self.replace_selection("", false);
                } else {
                    let start = self.cursor;
                    let end = self.word_right(self.cursor);
                    self.edit_group(|ed| ed.replace(start, end, ""));
                }
            }
            Action::Indent => self.indent(),
            Action::Unindent => self.unindent(),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
        }
        if !keeps_goal {
            self.goal_col = None;
        }
        self.scroll_to_cursor();
        true
    }

    /// Removes and returns the selection (cut).
    pub fn cut(&mut self) -> Option<String> {
        if self.read_only {
            return None;
        }
        let text = self.selected_text()?;
        self.replace_selection("", false);
        self.scroll_to_cursor();
        Some(text)
    }

    /// Places the cursor at `pos`; `select` extends the selection (mouse).
    pub fn set_cursor(&mut self, pos: usize, select: bool) {
        let pos = pos.min(self.rope.len_chars());
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = pos;
        self.goal_col = None;
        self.close_typing();
        self.scroll_to_cursor();
    }

    /// Scrolls by `lines` (mouse wheel) without moving the cursor.
    pub fn scroll_lines(&mut self, lines: isize) {
        let max = self.rope.len_lines().saturating_sub(1);
        self.top = (self.top as isize + lines).clamp(0, max as isize) as usize;
    }

    fn move_cursor(&mut self, motion: Motion, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else if let Some((a, b)) = self.selection() {
            // Left / Right without Shift collapse the selection to its side.
            self.anchor = None;
            match motion {
                Motion::Left => {
                    self.cursor = a;
                    return;
                }
                Motion::Right => {
                    self.cursor = b;
                    return;
                }
                _ => {}
            }
        } else {
            self.anchor = None;
        }
        self.close_typing();
        let len = self.rope.len_chars();
        let line = self.rope.char_to_line(self.cursor);
        let last_line = self.rope.len_lines() - 1;
        let vertical = |ed: &mut Self, target_line: usize| {
            let col = match ed.goal_col {
                Some(col) => col,
                None => {
                    let col = ed.display_col(line, ed.cursor);
                    ed.goal_col = Some(col);
                    col
                }
            };
            ed.cursor = ed.pos_at_col(target_line, col);
        };
        let page = self.view.0.max(2) - 1;
        match motion {
            Motion::Left => self.cursor = self.cursor.saturating_sub(1),
            Motion::Right => self.cursor = (self.cursor + 1).min(len),
            Motion::Up => {
                if line == 0 {
                    self.cursor = 0;
                } else {
                    vertical(self, line - 1);
                }
            }
            Motion::Down => {
                if line == last_line {
                    self.cursor = len;
                } else {
                    vertical(self, line + 1);
                }
            }
            Motion::PageUp => {
                vertical(self, line.saturating_sub(page));
                self.top = self.top.saturating_sub(page);
            }
            Motion::PageDown => {
                vertical(self, (line + page).min(last_line));
                self.top = (self.top + page).min(last_line);
            }
            Motion::WordLeft => self.cursor = self.word_left(self.cursor),
            Motion::WordRight => self.cursor = self.word_right(self.cursor),
            Motion::Home => {
                let start = self.rope.line_to_char(line);
                let first = start
                    + self
                        .rope
                        .line(line)
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .count();
                self.cursor = if self.cursor == first { start } else { first };
            }
            Motion::End => {
                self.cursor =
                    self.rope.line_to_char(line) + trim_newline(self.rope.line(line)).len_chars();
            }
            Motion::DocStart => self.cursor = 0,
            Motion::DocEnd => self.cursor = len,
        }
    }

    fn class(c: char) -> u8 {
        if c.is_alphanumeric() || c == '_' {
            2
        } else if c.is_whitespace() {
            0
        } else {
            1
        }
    }

    fn word_left(&self, pos: usize) -> usize {
        let mut i = pos;
        while i > 0 && Self::class(self.rope.char(i - 1)) == 0 {
            i -= 1;
        }
        if i > 0 {
            let cls = Self::class(self.rope.char(i - 1));
            while i > 0 && Self::class(self.rope.char(i - 1)) == cls {
                i -= 1;
            }
        }
        i
    }

    fn word_right(&self, pos: usize) -> usize {
        let len = self.rope.len_chars();
        let mut i = pos;
        if i < len {
            let cls = Self::class(self.rope.char(i));
            while i < len && Self::class(self.rope.char(i)) == cls {
                i += 1;
            }
        }
        while i < len && Self::class(self.rope.char(i)) == 0 && self.rope.char(i) != '\n' {
            i += 1;
        }
        i
    }

    // ---- editing primitives -------------------------------------------------------

    fn close_typing(&mut self) {
        if let Some(g) = self.undo.last_mut() {
            g.typing = false;
        }
    }

    /// Replaces chars `start..end` with `text`; records the edit in the open group.
    fn replace(&mut self, start: usize, end: usize, text: &str) {
        // nothing to remove and nothing to insert: no edit, no undo step, not modified
        if start == end && text.is_empty() {
            return;
        }
        let removed = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        self.rope.insert(start, text);
        let line = self.rope.char_to_line(start);
        self.lex.borrow_mut().invalidate_from(line);
        self.modified = true;
        if let Some(group) = self.undo.last_mut() {
            group.edits.push(Edit {
                at: start,
                removed,
                inserted: text.to_owned(),
            });
        }
    }

    /// Runs `f` as one undo group.
    fn edit_group(&mut self, f: impl FnOnce(&mut Self)) {
        self.close_typing();
        self.undo.push(Group {
            edits: Vec::new(),
            before: (self.cursor, self.anchor),
            after: (0, None),
            typing: false,
        });
        let cursor_before = self.cursor;
        f(self);
        // Cursor after the last edit's inserted text.
        if let Some(e) = self.undo.last().and_then(|g| g.edits.last()).cloned() {
            self.cursor = e.at + e.inserted.chars().count();
            // a real edit starts a new history branch (one that changed nothing does not)
            self.redo.clear();
        } else {
            self.cursor = cursor_before;
        }
        self.anchor = None;
        self.finish_group();
    }

    fn finish_group(&mut self) {
        let (cursor, anchor) = (self.cursor, self.anchor);
        if let Some(g) = self.undo.last_mut() {
            if g.edits.is_empty() {
                self.undo.pop();
            } else {
                g.after = (cursor, anchor);
            }
        }
    }

    fn replace_selection(&mut self, text: &str, typing: bool) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        // Typed characters join the previous typing group (one undo per word).
        let joins = typing
            && start == end
            && !text.chars().all(char::is_whitespace)
            && self.undo.last().is_some_and(|g| {
                g.typing
                    && g.edits
                        .last()
                        .is_some_and(|e| e.at + e.inserted.chars().count() == start)
            });
        if joins {
            self.redo.clear();
            self.replace(start, end, text);
            self.cursor = start + text.chars().count();
            self.anchor = None;
            self.finish_group();
            if let Some(g) = self.undo.last_mut() {
                g.typing = true;
            }
            return;
        }
        let text = text.to_owned();
        self.edit_group(move |ed| ed.replace(start, end, &text));
        if typing && let Some(g) = self.undo.last_mut() {
            g.typing = true;
        }
    }

    /// Lines covered by the selection (a selection ending at column 0 does not include
    /// that line), or the cursor line.
    fn selected_lines(&self) -> (usize, usize) {
        match self.selection() {
            Some((a, b)) => {
                let first = self.rope.char_to_line(a);
                let mut last = self.rope.char_to_line(b);
                if last > first && self.rope.line_to_char(last) == b {
                    last -= 1;
                }
                (first, last)
            }
            None => {
                let l = self.rope.char_to_line(self.cursor);
                (l, l)
            }
        }
    }

    fn select_lines(&mut self, first: usize, last: usize) {
        self.anchor = Some(self.rope.line_to_char(first));
        self.cursor = self.rope.line_to_char(last) + trim_newline(self.rope.line(last)).len_chars();
    }

    /// Tab: a tab character, or indent all selected lines (D-038).
    fn indent(&mut self) {
        if self.selection().is_none() {
            self.replace_selection("\t", false);
            return;
        }
        let (first, last) = self.selected_lines();
        self.edit_group(|ed| {
            for line in first..=last {
                let at = ed.rope.line_to_char(line);
                ed.replace(at, at, "\t");
            }
        });
        self.select_lines(first, last);
        self.fix_group_after();
    }

    /// Shift+Tab: unindent the selected lines or the cursor line (D-038).
    fn unindent(&mut self) {
        let had_selection = self.selection().is_some();
        let (first, last) = self.selected_lines();
        let cursor = self.cursor;
        let cursor_line = self.rope.char_to_line(cursor);
        let mut removed_on_cursor_line = 0;
        let tab_width = self.tab_width;
        self.edit_group(|ed| {
            for line in first..=last {
                let at = ed.rope.line_to_char(line);
                let text: String = ed.rope.line(line).chars().take(tab_width).collect();
                let n = if text.starts_with('\t') {
                    1
                } else {
                    text.chars().take_while(|c| *c == ' ').count()
                };
                if n > 0 {
                    ed.replace(at, at + n, "");
                    if line == cursor_line {
                        removed_on_cursor_line = n;
                    }
                }
            }
        });
        if had_selection {
            self.select_lines(first, last);
        } else {
            let start = self.rope.line_to_char(cursor_line);
            self.cursor = cursor.saturating_sub(removed_on_cursor_line).max(start);
            self.anchor = None;
        }
        self.fix_group_after();
    }

    fn fix_group_after(&mut self) {
        let (cursor, anchor) = (self.cursor, self.anchor);
        if let Some(g) = self.undo.last_mut() {
            g.after = (cursor, anchor);
        }
    }

    fn undo(&mut self) {
        self.close_typing();
        let Some(group) = self.undo.pop() else {
            return;
        };
        for e in group.edits.iter().rev() {
            let end = e.at + e.inserted.chars().count();
            self.rope.remove(e.at..end);
            self.rope.insert(e.at, &e.removed);
            let line = self.rope.char_to_line(e.at);
            self.lex.borrow_mut().invalidate_from(line);
        }
        (self.cursor, self.anchor) = group.before;
        self.modified = true;
        self.redo.push(group);
    }

    fn redo(&mut self) {
        let Some(group) = self.redo.pop() else {
            return;
        };
        for e in &group.edits {
            let end = e.at + e.removed.chars().count();
            self.rope.remove(e.at..end);
            self.rope.insert(e.at, &e.inserted);
            let line = self.rope.char_to_line(e.at);
            self.lex.borrow_mut().invalidate_from(line);
        }
        (self.cursor, self.anchor) = group.after;
        self.modified = true;
        self.undo.push(group);
    }

    // ---- scrolling ----------------------------------------------------------------

    /// Keeps the cursor inside the view.
    pub fn scroll_to_cursor(&mut self) {
        let (height, width) = self.view;
        let (line, col) = self.cursor_line_col();
        if line < self.top {
            self.top = line;
        } else if line >= self.top + height {
            self.top = line + 1 - height;
        }
        if col < self.left {
            self.left = col;
        } else if col >= self.left + width {
            self.left = col + 1 - width;
        }
    }

    // ---- search -------------------------------------------------------------------

    /// Selects the next (or previous) match after (before) the cursor / selection,
    /// wrapping around. `Ok(None)` = no match.
    pub fn find(&mut self, opts: &SearchOptions, forward: bool) -> Result<Option<Found>, String> {
        if opts.text.is_empty() {
            return Ok(None);
        }
        let re = opts.compile()?;
        let (sel_start, sel_end) = self.selection().unwrap_or((self.cursor, self.cursor));
        let from = if forward { sel_end } else { sel_start };
        let lines = self.rope.len_lines();
        let start_line = self.rope.char_to_line(from);
        let found = if forward {
            // rest of the start line, following lines, then wrap
            (0..=lines).find_map(|k| {
                let line = (start_line + k) % lines;
                let wrapped = start_line + k >= lines;
                let min = if k == 0 { from } else { 0 };
                let max = if k == lines { from } else { usize::MAX };
                self.matches_in_line(&re, line)
                    .into_iter()
                    .find(|(a, _)| *a >= min && *a < max)
                    .map(|m| (m, wrapped))
            })
        } else {
            (0..=lines).find_map(|k| {
                let line = (start_line + lines - k % lines) % lines;
                let wrapped = k > start_line;
                let max = if k == 0 { from } else { usize::MAX };
                let min = if k == lines { from } else { 0 };
                self.matches_in_line(&re, line)
                    .into_iter()
                    .rev()
                    .find(|(a, _)| *a < max && *a >= min)
                    .map(|m| (m, wrapped))
            })
        };
        let Some(((a, b), wrapped)) = found else {
            return Ok(None);
        };
        self.close_typing();
        self.anchor = Some(a);
        self.cursor = b;
        self.goal_col = None;
        self.scroll_to_cursor();
        Ok(Some(Found { wrapped }))
    }

    /// Matches in `line` as char ranges (absolute), skipping empty matches.
    fn matches_in_line(&self, re: &Regex, line: usize) -> Vec<(usize, usize)> {
        let start = self.rope.line_to_char(line);
        let text = self.line(line);
        re.find_iter(&text)
            .filter(|m| m.end() > m.start())
            .map(|m| {
                (
                    start + text[..m.start()].chars().count(),
                    start + text[..m.end()].chars().count(),
                )
            })
            .collect()
    }

    /// The selection is exactly one match of `opts`.
    pub fn selection_matches(&self, opts: &SearchOptions) -> bool {
        let (Some(text), Ok(re)) = (self.selected_text(), opts.compile()) else {
            return false;
        };
        re.find(&text)
            .is_some_and(|m| m.start() == 0 && m.end() == text.len())
    }

    /// Replaces the selected match with `replacement` (`$1` groups in regex mode).
    pub fn replace_selected(&mut self, opts: &SearchOptions, replacement: &str) -> bool {
        if self.read_only || !self.selection_matches(opts) {
            return false;
        }
        let Ok(re) = opts.compile() else {
            return false;
        };
        let text = self.selected_text().unwrap_or_default();
        let new = if opts.regex {
            re.replace(&text, replacement).into_owned()
        } else {
            replacement.to_owned()
        };
        self.replace_selection(&new, false);
        self.scroll_to_cursor();
        true
    }

    /// Replaces every match in the text as one undo step; returns the count.
    pub fn replace_all(
        &mut self,
        opts: &SearchOptions,
        replacement: &str,
    ) -> Result<usize, String> {
        if self.read_only || opts.text.is_empty() {
            return Ok(0);
        }
        let re = opts.compile()?;
        let mut matches = Vec::new();
        for line in 0..self.rope.len_lines() {
            matches.extend(self.matches_in_line(&re, line));
        }
        let count = matches.len();
        if count == 0 {
            return Ok(0);
        }
        self.edit_group(|ed| {
            for (a, b) in matches.into_iter().rev() {
                let old = ed.rope.slice(a..b).to_string();
                let new = if opts.regex {
                    re.replace(&old, replacement).into_owned()
                } else {
                    replacement.to_owned()
                };
                ed.replace(a, b, &new);
            }
        });
        self.cursor = self.cursor.min(self.rope.len_chars());
        self.scroll_to_cursor();
        Ok(count)
    }

    // ---- highlighting -------------------------------------------------------------

    /// Tokens of the visible lines `first..first+count`, with each line's text.
    pub fn highlighted_lines(&self, first: usize, count: usize) -> Vec<(String, Vec<Token>)> {
        let last = (first + count).min(self.rope.len_lines());
        if first >= last {
            return Vec::new();
        }
        let mut state = self.lex.borrow_mut().state_at(&self.rope, first);
        (first..last)
            .map(|l| {
                let text = self.line(l);
                let mut tokens = Vec::new();
                state = lex_line(&state, &text, Some(&mut tokens));
                (text, tokens)
            })
            .collect()
    }

    /// The background scan still has lines to lex (D-038).
    pub fn needs_scan(&self) -> bool {
        let done = self.lex.borrow().checkpoints.len() * CHECKPOINT_LINES;
        done + CHECKPOINT_LINES < self.rope.len_lines()
    }

    /// Lexes up to `budget` more lines for the checkpoint cache.
    pub fn background_scan(&self, budget: usize) {
        self.lex.borrow_mut().extend(&self.rope, budget);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        Editor::new(text, false, 4)
    }

    fn type_str(e: &mut Editor, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                e.apply(Action::Newline);
            } else {
                e.apply(Action::Insert(c.to_string()));
            }
        }
    }

    #[test]
    fn wheel_scrolling_is_not_undone_by_the_next_frame() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut e = ed(&text);
        e.set_view(10, 40);
        // the UI passes the size before every message
        for _ in 0..20 {
            e.set_view(10, 40);
            e.scroll_lines(3);
        }
        assert_eq!(e.top, 60);
        // a typed key brings the cursor (still on line 0) back into view
        e.apply(Action::Insert("x".into()));
        assert_eq!(e.top, 0);
        // a new size does too
        e.scroll_lines(30);
        e.set_view(12, 40);
        assert_eq!(e.top, 0);
    }

    #[test]
    fn typing_moving_and_undo_by_word() {
        let mut e = ed("");
        type_str(&mut e, "select 1");
        assert_eq!(e.text(), "select 1");
        // a space starts a new undo step: "select" | " 1"
        e.apply(Action::Undo);
        assert_eq!(e.text(), "select");
        e.apply(Action::Undo);
        assert_eq!(e.text(), "");
        e.apply(Action::Redo);
        e.apply(Action::Redo);
        assert_eq!(e.text(), "select 1");
    }

    #[test]
    fn edits_that_change_nothing_leave_no_trace() {
        let mut e = ed("abc");
        // nothing to delete at the start / end of the text
        e.apply(Action::DeleteWordLeft);
        e.apply(Action::Move(Motion::DocEnd, false));
        e.apply(Action::DeleteWordRight);
        e.apply(Action::Unindent);
        e.apply(Action::Insert(String::new()));
        assert!(!e.modified());
        assert_eq!(e.text(), "abc");
        e.apply(Action::Undo);
        assert_eq!(e.text(), "abc", "no empty undo step");
        // ... and they do not throw away what can be redone
        type_str(&mut e, "d");
        e.apply(Action::Undo);
        e.apply(Action::Move(Motion::DocStart, false));
        e.apply(Action::DeleteWordLeft);
        e.apply(Action::Redo);
        assert_eq!(e.text(), "abcd");
    }

    #[test]
    fn selection_and_delete() {
        let mut e = ed("hello world");
        e.apply(Action::Move(Motion::WordRight, true));
        assert_eq!(e.selected_text().as_deref(), Some("hello "));
        e.apply(Action::Delete);
        assert_eq!(e.text(), "world");
        e.apply(Action::SelectAll);
        assert_eq!(e.cut().as_deref(), Some("world"));
        assert!(e.is_empty());
        e.apply(Action::Undo);
        assert_eq!(e.text(), "world");
    }

    #[test]
    fn vertical_movement_keeps_column_with_tabs() {
        let mut e = ed("\tabc\nx\n\tabcdef");
        e.apply(Action::Move(Motion::End, false)); // col 7
        e.apply(Action::Move(Motion::Down, false));
        assert_eq!(e.cursor_line_col(), (1, 1));
        e.apply(Action::Move(Motion::Down, false));
        assert_eq!(e.cursor_line_col(), (2, 7), "goal column restored");
        e.apply(Action::Move(Motion::Home, false));
        assert_eq!(e.cursor_line_col(), (2, 4), "first non-blank");
        e.apply(Action::Move(Motion::Home, false));
        assert_eq!(e.cursor_line_col(), (2, 0));
    }

    #[test]
    fn block_indent_and_unindent() {
        let mut e = ed("a\nb\nc\nd");
        e.apply(Action::Move(Motion::Down, true)); // select line a (ends at col 0 of b)
        e.apply(Action::Move(Motion::Down, true)); // and b
        e.apply(Action::Indent);
        assert_eq!(e.text(), "\ta\n\tb\nc\nd");
        e.apply(Action::Unindent);
        assert_eq!(e.text(), "a\nb\nc\nd");
        // no selection: Tab inserts a tab, Shift+Tab unindents the line
        let mut e = ed("    x");
        e.apply(Action::Move(Motion::End, false));
        e.apply(Action::Unindent);
        assert_eq!(e.text(), "x");
        assert_eq!(e.cursor(), 1);
        e.apply(Action::Indent);
        assert_eq!(e.text(), "x\t");
        e.apply(Action::Undo);
        e.apply(Action::Undo);
        assert_eq!(e.text(), "    x");
    }

    #[test]
    fn read_only_refuses_edits_but_moves() {
        let mut e = Editor::new("abc", true, 4);
        assert!(!e.apply(Action::Insert("x".into())));
        assert!(e.apply(Action::Move(Motion::Right, true)));
        assert_eq!(e.selected_text().as_deref(), Some("a"));
        assert_eq!(e.cut(), None);
        assert_eq!(e.text(), "abc");
    }

    #[test]
    fn find_and_replace() {
        let mut e = ed("Select a;\nselect b;\nSELECTED");
        let mut o = SearchOptions {
            text: "select".into(),
            ..SearchOptions::default()
        };
        assert!(e.find(&o, true).unwrap().is_some());
        assert_eq!(e.selection(), Some((0, 6)));
        e.find(&o, true).unwrap();
        assert_eq!(e.selection(), Some((10, 16)));
        o.whole_words = true;
        let f = e.find(&o, true).unwrap().unwrap();
        assert_eq!(e.selection(), Some((0, 6)), "SELECTED skipped, wrapped");
        assert!(f.wrapped);
        o.case_sensitive = true;
        assert!(e.find(&o, true).unwrap().is_some());
        assert_eq!(e.selection(), Some((10, 16)));
        assert!(e.find(&o, false).unwrap().is_some());
        assert_eq!(
            e.selection(),
            Some((10, 16)),
            "only one case-sensitive match"
        );
        assert!(e.replace_selected(&o, "SELECT"));
        assert_eq!(e.line(1), "SELECT b;");
        o.case_sensitive = false; // whole words still on: SELECTED is not replaced
        assert_eq!(e.replace_all(&o, "X").unwrap(), 2);
        assert_eq!(e.text(), "X a;\nX b;\nSELECTED");
        e.apply(Action::Undo);
        assert_eq!(e.text(), "Select a;\nSELECT b;\nSELECTED");
        let re = SearchOptions {
            text: r"(\w+) (\w);".into(),
            regex: true,
            ..SearchOptions::default()
        };
        assert_eq!(e.replace_all(&re, "$2 $1;").unwrap(), 2);
        assert_eq!(e.line(0), "a Select;");
        assert!(
            SearchOptions {
                text: "(".into(),
                regex: true,
                ..Default::default()
            }
            .compile()
            .is_err()
        );
    }

    #[test]
    fn highlighting_uses_checkpoints_across_edits() {
        // A dollar quote opened before a checkpoint is still known after it.
        let mut text = String::from("select $$\n");
        for _ in 0..(CHECKPOINT_LINES * 2) {
            text.push_str("select 1;\n");
        }
        text.push_str("$$;\nselect 2;\n");
        let mut e = ed(&text);
        assert!(e.needs_scan());
        e.background_scan(usize::MAX);
        assert!(!e.needs_scan());
        let last = e.len_lines() - 3;
        let lines = e.highlighted_lines(last, 2);
        assert_eq!(lines[0].0, "$$;");
        assert_eq!(lines[1].1[0].kind, crate::lexer::TokenKind::Keyword);
        let inside = e.highlighted_lines(CHECKPOINT_LINES + 3, 1);
        assert_eq!(inside[0].1[0].kind, crate::lexer::TokenKind::String);
        // Removing the opening $$ changes everything below it.
        e.set_cursor(7, false);
        e.apply(Action::Delete);
        e.apply(Action::Delete);
        let inside = e.highlighted_lines(CHECKPOINT_LINES + 3, 1);
        assert_eq!(inside[0].1[0].kind, crate::lexer::TokenKind::Keyword);
    }
}
