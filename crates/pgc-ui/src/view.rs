//! Rendering of the main window (D-009, D-016): menu bar, tree pane, tabbed right pane,
//! status line, F-key bar. Only reads the state; records click areas in [`Hits`].

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use pgc_editor::EditorStyles;

use crate::app::{App, Focus, TabKind};
use crate::icons::{Icon, icon};
use crate::keys::KeyCombo;
use crate::menu::{self, MenuHits};
use crate::theme::Role;
use crate::tree::RowKind;

/// Screen areas from the last frame, used for mouse clicks.
#[derive(Debug, Default, Clone)]
pub struct Hits {
    pub menu: MenuHits,
    pub tree: Rect,
    pub right: Rect,
    pub tabs: Vec<Rect>,
    /// The two border columns between the tree and the tabs (drag handle) and the
    /// area they divide (D-045).
    pub pane_border: Rect,
    pub main: Rect,
    /// Close marks `×` of the tabs: (tab index, area) (D-043).
    pub tab_close: Vec<(usize, Rect)>,
    /// Key-hint bar entries: the key a click presses.
    pub fkeys: Vec<(KeyCombo, Rect)>,
    /// Outer area and widget areas of the top dialog.
    pub dialog: Option<(Rect, Vec<Rect>)>,
    /// The whole screen.
    pub screen: Rect,
    /// Editor area of the active tab (text + gutter).
    pub editor: Rect,
    /// Screen position of the editor cursor (anchor of the autocomplete list).
    pub editor_cursor: Option<Position>,
    /// Context menu box and its items.
    pub popup: Option<Rect>,
    /// Developer template form: click areas (D-046).
    pub template: Vec<(crate::template_dialog::Stop, Rect)>,
    pub popup_items: Vec<(usize, Rect)>,
    /// Query tab: the bar above the results (Results / Messages / result sets).
    pub result_bar: Vec<(BarItem, Rect)>,
    /// Query tab: the results bar row (drag handle of the split) and the area the
    /// split divides (D-019).
    pub split: Rect,
    pub split_area: Rect,
    /// Query tab: grid or messages area, visible grid cells, message rows.
    pub grid: Rect,
    pub grid_cells: Vec<crate::grid::CellArea>,
    pub message_rows: Vec<(usize, Rect)>,
    /// Dashboard tab: list sub-tabs and grid cells (D-057).
    pub dashboard: crate::dashboard::DashHits,
}

/// Clickable items of the results bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarItem {
    Results,
    Messages,
    Set(usize),
}

/// Tree pane width: the dragged width or a third of the window, leaving both panes
/// usable (D-045).
pub fn tree_width(wanted: Option<u16>, total: u16) -> u16 {
    let max = total.saturating_sub(30).max(20);
    wanted
        .unwrap_or((total / 3).max(20))
        .clamp(20, max)
        .min(total)
}

pub fn render(frame: &mut Frame, app: &App, hits: &mut Hits) {
    let area = frame.area();
    hits.screen = area;
    frame.render_widget(Block::new().style(app.theme.get(Role::CoreNormal)), area);

    let [menu_row, main, status_row, fkey_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let left_width = tree_width(app.settings.ui.tree_width, main.width);
    hits.main = main;
    hits.pane_border = Rect::new(
        (main.x + left_width).saturating_sub(1),
        main.y,
        2.min(main.width),
        main.height,
    );
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Fill(1)]).areas(main);

    render_tree(frame, app, left);
    hits.tree = left;
    hits.editor_cursor = None;
    render_tabs(frame, app, right, hits);
    hits.right = right;
    if let Some(p) = app.completion.as_ref().filter(|p| p.shown() && !p.in_form)
        && let Some(at) = hits.editor_cursor
    {
        crate::completion::render(frame, p, at, area, &app.theme);
    }
    render_status(frame, app, status_row);
    render_fkeys(frame, app, fkey_row, hits);

    menu::render_bar(
        frame,
        menu_row,
        &app.menus,
        app.menu,
        &app.theme,
        &mut hits.menu,
    );
    let item_state = menu::ItemState {
        enabled: &|c| app.is_enabled(c),
        shortcut: &|c| app.shortcut_text(c),
        checked: &|c| app.checked(c),
    };
    if let Some(state) = app.menu {
        menu::render_dropdown(
            frame,
            &app.menus,
            state,
            &app.theme,
            &item_state,
            &mut hits.menu,
        );
    } else {
        hits.menu.dropdown = None;
        hits.menu.items.clear();
    }

    hits.popup = None;
    hits.popup_items.clear();
    if let Some(popup) = &app.popup {
        let (area, items) = menu::render_entries(
            frame,
            &popup.entries,
            popup.item,
            popup.anchor,
            &app.theme,
            &item_state,
        );
        hits.popup = Some(area);
        hits.popup_items = items;
    }

    hits.template.clear();
    if let Some(form) = &app.template {
        let t = &app.theme;
        let styles = EditorStyles {
            text: t.get(Role::EditorText),
            keyword: t.get(Role::EditorKeyword),
            string: t.get(Role::EditorString),
            number: t.get(Role::EditorNumber),
            comment: t.get(Role::EditorComment),
            quoted_ident: t.get(Role::EditorQuotedIdent),
            operator: t.get(Role::EditorOperator),
            selection: t.get(Role::EditorSelection),
            line_number: t.get(Role::EditorLineNumber),
        };
        let editing = app.dialogs.is_empty();
        hits.template = form.render(frame, &app.theme, &styles);
        if !editing {
            // a row dialog is open on top: no cursor in the form
            hits.template.clear();
        }
        // the autocomplete list of the form's `sql` box, over the form (D-052)
        if let Some(p) = app.completion.as_ref().filter(|p| p.shown() && p.in_form)
            && let Some(at) = form.sql_cursor.get()
        {
            crate::completion::render(frame, p, at, frame.area(), &app.theme);
        }
    }

    hits.dialog = None;
    for dialog in &app.dialogs {
        hits.dialog = Some(dialog.render(frame, &app.theme));
    }
}

fn pane_block(app: &App, title: &str, focused: bool) -> Block<'static> {
    let title_style = if focused {
        app.theme.get(Role::CoreTitleFocused)
    } else {
        app.theme.get(Role::CoreTitle)
    };
    // windows have double frames, separators inside them single lines (D-014)
    let block = Block::bordered()
        .border_set(app.theme.double_border())
        .border_style(app.theme.get(Role::CoreBorder))
        .style(app.theme.get(Role::CoreNormal));
    if title.is_empty() {
        block
    } else {
        block.title_top(Line::from(Span::styled(format!(" {title} "), title_style)).centered())
    }
}

/// A single separator line across a window row, joined to its double frame: the
/// `╟` / `╢` go on the frame columns left and right of `row` (D-014).
fn separator(frame: &mut Frame, app: &App, row: Rect) {
    let border = app.theme.get(Role::CoreBorder);
    let (left, right) = app.theme.separator_ends();
    let line = app.theme.hline().repeat(row.width as usize);
    let buf = frame.buffer_mut();
    buf.set_string(row.x.saturating_sub(1), row.y, left, border);
    buf.set_string(row.x, row.y, line, border);
    buf.set_string(row.right(), row.y, right, border);
}

fn render_tree(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Tree;
    let block = pane_block(app, "Connections", focused);
    let hint = app.theme.get(Role::CoreHint);
    let normal = app.theme.get(Role::CoreNormal);
    let rows = app.tree.rows();
    if rows.is_empty() {
        let text = vec![
            Line::from(Span::styled(" No connections yet.", hint)),
            Line::from(Span::styled(" F9 → File → New connection…", hint)),
        ];
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let selected = if focused {
        app.theme.get(Role::CoreSelected)
    } else {
        app.theme.get(Role::CoreSelectedUnfocused)
    };
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(app.tree.offset)
        .take(inner.height as usize)
        .map(|(i, row)| {
            let t = app.tree.row_text(*row);
            let ic = t.icon.map_or("  ", |ic| icon(ic, app.settings.ui.icons));
            let text = format!(" {}{} {}{}", "  ".repeat(t.depth), t.marker, ic, t.label);
            let text: String = text.chars().take(width).collect();
            let style = if i == app.tree.cursor {
                selected
            } else if matches!(row.kind, RowKind::Loading(_)) {
                hint
            } else {
                normal
            };
            Line::from(Span::styled(format!("{text:<width$}"), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_tabs(frame: &mut Frame, app: &App, area: Rect, hits: &mut Hits) {
    let tab = &app.tabs[app.active_tab];
    // no title: the tab strip names the tab, the status row says where it comes from
    let block = pane_block(app, "", app.focus != Focus::Tree);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    hits.result_bar.clear();
    hits.split = Rect::default();
    hits.split_area = Rect::default();
    hits.grid = Rect::default();
    hits.grid_cells.clear();
    hits.message_rows.clear();
    hits.dashboard = crate::dashboard::DashHits::default();
    hits.tabs.clear();
    hits.tab_close.clear();
    hits.editor = Rect::default();
    if inner.height < 5 || inner.width < 4 {
        return;
    }
    // inside the frame: tab strip, ─, content, ─, tab status (D-014, D-043)
    let [strip, top_sep, inner, bottom_sep, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    separator(frame, app, top_sep);
    separator(frame, app, bottom_sep);
    render_tab_status(frame, app, tab, status);

    let bar = app.theme.get(Role::TabsBar);
    frame.render_widget(Block::new().style(bar), strip);
    let mut spans = Vec::new();
    let mut x = strip.x;
    for (i, tab) in app.tabs.iter().enumerate() {
        let ic = match &tab.kind {
            TabKind::Default | TabKind::Object => Icon::ObjectView,
            TabKind::Query(_) => Icon::Query,
            TabKind::Dashboard => Icon::Server,
        };
        // every tab but the default one has a close mark (D-043)
        let closable = i > 0;
        let close = if closable { " ×" } else { "" };
        let label = format!(
            " {}{}{close} ",
            icon(ic, app.settings.ui.icons),
            tab.label()
        );
        let width = label.chars().count() as u16;
        if closable {
            let cx = x + width - 2;
            if cx < strip.right() {
                hits.tab_close.push((i, Rect::new(cx, strip.y, 1, 1)));
            }
        }
        let style = if i == app.active_tab {
            app.theme.get(Role::TabsActive)
        } else {
            app.theme.get(Role::TabsInactive)
        };
        hits.tabs.push(Rect::new(
            x,
            strip.y,
            width.min(strip.right().saturating_sub(x)),
            1,
        ));
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(" ", bar));
        x = x.saturating_add(width + 1);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), strip);

    // Dashboard tab: graphs and lists instead of an editor (D-057)
    if let Some(d) = &tab.dashboard {
        let focused = app.focus == Focus::Tabs;
        hits.dashboard = crate::dashboard::render(frame, d, inner, &app.theme, focused, &|f, r| {
            separator(f, app, r)
        });
        hits.grid = hits.dashboard.grid;
        return;
    }
    let view = &tab.view;
    // Query tab: editor on top, results below (D-019)
    let inner = match &tab.query {
        Some(q) if q.results_visible && inner.height >= 8 => {
            let results_h = (inner.height as u32 * q.results_percent as u32 / 100) as u16;
            // separator, labels, separator and at least one row
            let results_h = results_h.clamp(4, inner.height - 3);
            let editor_area = Rect {
                height: inner.height - results_h,
                ..inner
            };
            let results_area = Rect {
                y: inner.y + editor_area.height,
                height: results_h,
                ..inner
            };
            hits.split = Rect {
                height: 1,
                ..results_area
            };
            hits.split_area = inner;
            render_results(frame, app, q, results_area, hits);
            editor_area
        }
        _ => inner,
    };
    hits.editor = inner;
    if view.loading && view.is_empty() {
        let hint = app.theme.get(Role::CoreHint);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" loading…", hint))),
            inner,
        );
        return;
    }
    let t = &app.theme;
    let styles = EditorStyles {
        text: t.get(Role::EditorText),
        keyword: t.get(Role::EditorKeyword),
        string: t.get(Role::EditorString),
        number: t.get(Role::EditorNumber),
        comment: t.get(Role::EditorComment),
        quoted_ident: t.get(Role::EditorQuotedIdent),
        operator: t.get(Role::EditorOperator),
        selection: t.get(Role::EditorSelection),
        line_number: t.get(Role::EditorLineNumber),
    };
    let cursor = pgc_editor::render(
        &view.editor,
        inner,
        frame.buffer_mut(),
        &styles,
        app.settings.editor.line_numbers,
    );
    let editing = app.focus == Focus::Tabs
        && app.dialogs.is_empty()
        && app.template.is_none()
        && app.menu.is_none()
        && app.popup.is_none();
    if editing && let Some(pos) = cursor {
        hits.editor_cursor = Some(pos);
        frame.set_cursor_position(pos);
        crate::dialog::show_cursor_cell(frame, pos);
    }
}

/// The results pane of a query tab: bar + grid or messages (D-019, D-041).
fn render_results(
    frame: &mut Frame,
    app: &App,
    q: &crate::query_tab::QueryState,
    area: Rect,
    hits: &mut Hits,
) {
    let t = &app.theme;
    let focused = app.focus == Focus::Results;
    // ─ (drag handle), labels, ─, body (D-019)
    let [split_row, bar_area, label_sep, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    separator(frame, app, split_row);
    separator(frame, app, label_sep);
    let bar = t.get(Role::TabsBar);
    frame.render_widget(Block::new().style(bar), bar_area);
    // (label, active, item); `None` = separator
    let mut items: Vec<Option<(String, bool, BarItem)>> = vec![
        Some(("Results".into(), !q.show_messages, BarItem::Results)),
        Some((
            format!("Messages ({})", q.messages.len()),
            q.show_messages,
            BarItem::Messages,
        )),
    ];
    if !q.show_messages && !q.results.is_empty() {
        items.push(None);
        for (i, rs) in q.results.iter().enumerate() {
            items.push(Some((
                rs.label(i + 1),
                i == q.active_result,
                BarItem::Set(i),
            )));
        }
    }
    let mut x = bar_area.x;
    let mut spans = Vec::new();
    for entry in items {
        let Some((label, active, what)) = entry else {
            spans.push(Span::styled(format!("{} ", app.theme.vline()), bar));
            x = x.saturating_add(2);
            continue;
        };
        let style = if active {
            t.get(Role::TabsActive)
        } else {
            t.get(Role::TabsInactive)
        };
        let text = format!(" {label} ");
        let w = text.chars().count() as u16;
        hits.result_bar.push((what, Rect::new(x, bar_area.y, w, 1)));
        spans.push(Span::styled(text, style));
        spans.push(Span::styled(" ", bar));
        x = x.saturating_add(w + 1);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), bar_area);
    hits.grid = body;

    if q.show_messages {
        let mut y = body.y;
        let hint = t.get(Role::MessagesInfo);
        // keep the cursor message visible: start a few messages before it
        let first = q.message_cursor.saturating_sub(body.height as usize / 2);
        for (i, m) in q.messages.iter().enumerate().skip(first) {
            if y >= body.bottom() {
                break;
            }
            let style = match m.kind {
                crate::query_tab::MessageKind::Error => t.get(Role::MessagesError),
                crate::query_tab::MessageKind::Notice => t.get(Role::MessagesNotice),
                _ => hint,
            };
            let style = if focused && i == q.message_cursor {
                t.get(Role::GridCursor)
            } else {
                style
            };
            let start = y;
            for line in m.text.lines() {
                if y >= body.bottom() {
                    break;
                }
                let text: String = format!(" {line}")
                    .chars()
                    .take(body.width as usize)
                    .collect();
                let w = body.width as usize;
                frame.render_widget(
                    Paragraph::new(format!("{text:<w$}")).style(style),
                    Rect::new(body.x, y, body.width, 1),
                );
                y += 1;
            }
            hits.message_rows
                .push((i, Rect::new(body.x, start, body.width, y - start)));
        }
        return;
    }
    match q.results.get(q.active_result) {
        Some(rs) => {
            let styles = crate::grid::GridStyles {
                text: t.get(Role::CoreNormal),
                header: t.get(Role::GridHeader),
                null: t.get(Role::GridNull),
                cursor: t.get(Role::GridCursor),
                row_number: t.get(Role::GridRowNumber),
                // lines in the border colour on the grid background
                separator: {
                    let text = t.get(Role::CoreNormal);
                    match t.get(Role::CoreBorder).fg {
                        Some(fg) => text.fg(fg),
                        None => text,
                    }
                },
                ascii: t.ascii,
            };
            hits.grid_cells = crate::grid::render(rs, body, frame.buffer_mut(), &styles, focused);
        }
        None => {
            let text = if q.busy() {
                " running…"
            } else if q.messages.is_empty() {
                " F5 executes the text (or the selection)."
            } else {
                " No result rows — see Messages (Alt+M)."
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(text, t.get(Role::CoreHint)))),
                body,
            );
        }
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    // window background, like mc's hint line: it must not run into the F-key bar (D-014)
    let style = app.theme.get(Role::CoreNormal);
    let text = if app.esc_pending() {
        " Esc: press a digit for F1..F10".to_owned()
    } else {
        format!(" {}", app.status)
    };
    // tab state is shown in each tab's own status bar (D-043)
    let width = area.width as usize;
    let line: String = text.chars().take(width).collect();
    frame.render_widget(Paragraph::new(format!("{line:<width$}")).style(style), area);
}

/// Status row of one tab, at the bottom inside its frame: where it is connected and
/// what it is doing (D-043).
fn render_tab_status(frame: &mut Frame, app: &App, tab: &crate::app::Tab, area: Rect) {
    let style = app.theme.get(Role::StatusNormal);
    let gap = app.theme.get(Role::CoreNormal);
    let mut left = String::from(" ");
    let mut right = String::new();
    if let Some((l, r)) = app.dashboard_status(tab) {
        left.push_str(&l);
        right = format!(" {r} ");
    } else {
        left.push_str(&app.tab_connection_label(tab));
    }
    if let Some(q) = &tab.query {
        match q.tx {
            pgc_db::TxStatus::Idle => {}
            pgc_db::TxStatus::InTransaction => left.push_str(" · Transaction open"),
            pgc_db::TxStatus::Failed => left.push_str(" · Transaction failed – rollback"),
        }
        if q.plpgsql {
            left.push_str(" · PL/pgSQL");
        }
        if let Some(e) = &q.export {
            right.push_str(&format!(
                "Exporting… {} rows (Alt+Shift+C cancels)  ",
                crate::grid::thousands(e.written as usize)
            ));
        } else if let Some(r) = q.running.as_ref().filter(|r| !r.paused) {
            let secs = r.started.elapsed().as_secs_f64();
            if r.cancel_requested.is_some() {
                right.push_str(&format!("Cancelling… {secs:.1} s  "));
            } else {
                right.push_str(&format!("Executing… {secs:.1} s (Alt+Shift+C cancels)  "));
            }
        } else if let Some(rs) = q.results.get(q.active_result)
            && rs.more
        {
            right.push_str(&format!(
                "{} rows loaded – more available  ",
                crate::grid::thousands(rs.rows.len())
            ));
        } else if let Some(f) = &q.finished {
            right.push_str(&format!("{}  ", f.text()));
        }
    }
    if tab.dashboard.is_none() {
        let (line, col) = tab.view.editor.cursor_line_col();
        right.push_str(&format!("Ln {}, Col {} ", line + 1, col + 1));
    }
    if !right.starts_with(' ') {
        right.insert(0, ' ');
    }
    left.push(' ');
    let width = area.width as usize;
    let right: String = right.chars().take(width).collect();
    let right_width = right.chars().count();
    // one frame character at least between the two parts
    let left_width = width.saturating_sub(right_width + 1);
    let left: String = left.chars().take(left_width).collect();
    let fill = width - right_width - left.chars().count();
    let line = Line::from(vec![
        Span::styled(left, style),
        Span::styled(" ".repeat(fill), gap),
        Span::styled(right, style),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Key-hint bar: the most important keys of the focused frame (D-016).
fn render_fkeys(frame: &mut Frame, app: &App, area: Rect, hits: &mut Hits) {
    hits.fkeys.clear();
    let number = app.theme.get(Role::FkeyNumber);
    let label = app.theme.get(Role::FkeyLabel);
    frame.render_widget(Block::new().style(label), area);
    let mut spans = Vec::new();
    let mut x = area.x;
    for hint in app.key_hints() {
        let key = hint.key_text();
        let text = format!(" {} ", hint.label);
        let width = (key.chars().count() + text.chars().count()) as u16;
        // entries that do not fit are left out from the right
        if x + width > area.right() {
            break;
        }
        hits.fkeys.push((hint.key, Rect::new(x, area.y, width, 1)));
        spans.push(Span::styled(key, number));
        spans.push(Span::styled(text, label));
        x += width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
