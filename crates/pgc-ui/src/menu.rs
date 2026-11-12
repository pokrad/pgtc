//! Menu bar (D-014): `F9`, arrows, highlighted hotkey letters, mouse clicks.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::hotkey::HotText;
use crate::keymap::Command;
use crate::theme::{Role, Theme};

pub enum MenuEntry {
    Item { label: HotText, cmd: Command },
    Separator,
}

pub struct MenuDef {
    pub title: HotText,
    pub entries: Vec<MenuEntry>,
}

pub fn item(label: &str, cmd: Command) -> MenuEntry {
    MenuEntry::Item {
        label: HotText::new(label),
        cmd,
    }
}

/// All menus of D-014.
pub fn menu_bar() -> Vec<MenuDef> {
    use Command as C;
    use MenuEntry::Separator;
    vec![
        MenuDef {
            title: HotText::new("&File"),
            entries: vec![
                item("&New connection…", C::NewConnection),
                item("&Connect", C::Connect),
                item("&Disconnect", C::Disconnect),
                Separator,
                item("New &query", C::NewQuery),
                item("&Open SQL file…", C::OpenFile),
                item("&Save SQL file…", C::SaveFile),
                Separator,
                item("&Export database definition…", C::ExportDefinition),
                item("&Load database definition…", C::LoadDefinition),
                Separator,
                item("Q&uit", C::Quit),
            ],
        },
        MenuDef {
            title: HotText::new("&View"),
            entries: vec![
                item("&Refresh object tree", C::RefreshTree),
                item("&Edit in new query tab", C::EditInQueryTab),
                item("&Find object…", C::FindObject),
                Separator,
                item("&Next tab", C::NextTab),
                item("&Previous tab", C::PrevTab),
                item("&Close tab", C::CloseTab),
            ],
        },
        MenuDef {
            title: HotText::new("&Query"),
            entries: vec![
                item("&Execute", C::Execute),
                item("&Cancel", C::CancelQuery),
                item("&Fetch all", C::FetchAll),
                item("E&xport result…", C::ExportResult),
                Separator,
                item("Co&mmit", C::Commit),
                item("&Rollback", C::Rollback),
                Separator,
                item("&Auto commit", C::ToggleAutoCommit),
                item("Auto roll&back on error", C::ToggleAutoRollback),
                item("&PL/pgSQL mode", C::TogglePlpgsql),
                Separator,
                item("Show/hide res&ults", C::ToggleResults),
                item("Results / Messa&ges", C::ShowMessages),
                item("&Next result set", C::NextResult),
                item("Pre&vious result set", C::PrevResult),
            ],
        },
        MenuDef {
            title: HotText::new("&Tools"),
            entries: vec![
                item("&Generate SQL from developer template…", C::NewFromTemplate),
                item("ps&ql command…", C::Psql),
            ],
        },
        MenuDef {
            title: HotText::new("&Settings"),
            entries: vec![
                item("&Settings…", C::Settings),
                Separator,
                item("&Run setup", C::RunSetup),
                item("Install &templates", C::InstallTemplates),
                item("Show templates f&older", C::ShowTemplatesFolder),
            ],
        },
        MenuDef {
            title: HotText::new("&Help"),
            entries: vec![
                item("&Keyboard shortcuts", C::KeyboardShortcuts),
                item("Third-party &licences", C::ThirdPartyLicences),
                item("&About pgtc", C::About),
            ],
        },
    ]
}

/// Open menu and highlighted item (`None` when all items are disabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuState {
    pub menu: usize,
    pub item: Option<usize>,
}

pub enum MenuEvent {
    None,
    Close,
    Run(Command),
}

fn entry_cmd(e: &MenuEntry) -> Option<Command> {
    match e {
        MenuEntry::Item { cmd, .. } => Some(*cmd),
        MenuEntry::Separator => None,
    }
}

impl MenuState {
    pub fn open(menus: &[MenuDef], menu: usize, enabled: &dyn Fn(Command) -> bool) -> Self {
        let item = menus[menu]
            .entries
            .iter()
            .position(|e| entry_cmd(e).is_some_and(enabled));
        Self { menu, item }
    }

    fn move_item(&mut self, menus: &[MenuDef], delta: isize, enabled: &dyn Fn(Command) -> bool) {
        self.item = move_in(&menus[self.menu].entries, self.item, delta, enabled);
    }

    pub fn handle_key(
        &mut self,
        menus: &[MenuDef],
        cmd: Option<Command>,
        ev: &KeyEvent,
        enabled: &dyn Fn(Command) -> bool,
    ) -> MenuEvent {
        if let KeyCode::Char(c) = ev.code
            && !ev.modifiers.contains(KeyModifiers::CONTROL)
        {
            let c = c.to_lowercase().next().unwrap_or(c);
            if ev.modifiers.contains(KeyModifiers::ALT) {
                if let Some(m) = menus.iter().position(|m| m.title.key() == Some(c)) {
                    *self = Self::open(menus, m, enabled);
                }
                return MenuEvent::None;
            }
            return hotkey_in(&menus[self.menu].entries, c, enabled)
                .map_or(MenuEvent::None, MenuEvent::Run);
        }
        let n = menus.len();
        match cmd {
            Some(Command::Left) => *self = Self::open(menus, (self.menu + n - 1) % n, enabled),
            Some(Command::Right) => *self = Self::open(menus, (self.menu + 1) % n, enabled),
            Some(Command::Up | Command::PrevField) => self.move_item(menus, -1, enabled),
            Some(Command::Down | Command::NextField) => self.move_item(menus, 1, enabled),
            Some(Command::Home) => {
                self.item = None;
                self.move_item(menus, 1, enabled);
            }
            Some(Command::End) => {
                self.item = None;
                self.move_item(menus, -1, enabled);
            }
            Some(Command::Accept | Command::Toggle) => {
                if let Some(cmd) = self
                    .item
                    .and_then(|i| entry_cmd(&menus[self.menu].entries[i]))
                {
                    return MenuEvent::Run(cmd);
                }
            }
            Some(Command::Cancel | Command::Menu | Command::Quit) => return MenuEvent::Close,
            _ => {}
        }
        MenuEvent::None
    }
}

/// Mouse areas produced by [`render_bar`] / [`render_dropdown`].
#[derive(Debug, Default, Clone)]
pub struct MenuHits {
    pub titles: Vec<Rect>,
    pub dropdown: Option<Rect>,
    /// (entry index, area)
    pub items: Vec<(usize, Rect)>,
}

/// Draws the menu bar in `area` (one row).
pub fn render_bar(
    frame: &mut Frame,
    area: Rect,
    menus: &[MenuDef],
    open: Option<MenuState>,
    theme: &Theme,
    hits: &mut MenuHits,
) {
    let normal = theme.get(Role::MenuNormal);
    frame.render_widget(Block::new().style(normal), area);
    hits.titles.clear();
    let mut x = area.x + 1;
    let mut spans = vec![Span::styled(" ", normal)];
    for (i, m) in menus.iter().enumerate() {
        let selected = open.is_some_and(|o| o.menu == i);
        let (st, hot) = if selected {
            (
                theme.get(Role::MenuSelected),
                theme.get(Role::MenuSelectedHotkey),
            )
        } else {
            (normal, theme.get(Role::MenuHotkey))
        };
        let width = m.title.width() as u16 + 4;
        spans.push(Span::styled("  ", st));
        spans.extend(m.title.spans(st, hot));
        spans.push(Span::styled("  ", st));
        hits.titles.push(Rect::new(x, area.y, width, 1));
        x += width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// How each menu item looks right now (asked from the app when drawing).
pub struct ItemState<'a> {
    pub enabled: &'a dyn Fn(Command) -> bool,
    pub shortcut: &'a dyn Fn(Command) -> Option<String>,
    /// `Some(on)` for on/off items, drawn as `[x]` / `[ ]`.
    pub checked: &'a dyn Fn(Command) -> Option<bool>,
}

/// Draws the pull-down of the open menu below the bar.
pub fn render_dropdown(
    frame: &mut Frame,
    menus: &[MenuDef],
    state: MenuState,
    theme: &Theme,
    items: &ItemState<'_>,
    hits: &mut MenuHits,
) {
    let x = hits.titles.get(state.menu).map_or(0, |r| r.x);
    let y = frame.area().y + 1;
    let entries = &menus[state.menu].entries;
    let (area, item_areas) = render_entries(frame, entries, state.item, (x, y), theme, items);
    hits.dropdown = Some(area);
    hits.items = item_areas;
}

/// Draws a box with menu entries whose top-left corner is near `anchor`.
/// Returns the box and the area of every item.
pub fn render_entries(
    frame: &mut Frame,
    entries: &[MenuEntry],
    selected: Option<usize>,
    anchor: (u16, u16),
    theme: &Theme,
    items: &ItemState<'_>,
) -> (Rect, Vec<(usize, Rect)>) {
    let (enabled, shortcut, checked) = (items.enabled, items.shortcut, items.checked);
    let screen = frame.area();
    let shortcuts: Vec<Option<String>> = entries
        .iter()
        .map(|e| entry_cmd(e).and_then(shortcut))
        .collect();
    let label_w = entries
        .iter()
        .map(|e| match e {
            MenuEntry::Item { label, cmd } => {
                label.width() + if checked(*cmd).is_some() { 4 } else { 0 }
            }
            MenuEntry::Separator => 0,
        })
        .max()
        .unwrap_or(0);
    let key_w = shortcuts
        .iter()
        .flatten()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0);
    let inner_w = label_w + if key_w > 0 { key_w + 3 } else { 0 } + 2;
    let width = (inner_w as u16 + 2).min(screen.width);
    let height = (entries.len() as u16 + 2).min(screen.height);
    let x = anchor.0.min(screen.right().saturating_sub(width));
    let y = anchor.1.min(screen.bottom().saturating_sub(height));
    let area = Rect::new(x, y, width, height);

    let normal = theme.get(Role::MenuNormal);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::bordered()
            .border_set(theme.plain_border())
            .style(normal)
            .border_style(normal),
        area,
    );
    let mut item_areas = Vec::new();

    let inner_w = area.width.saturating_sub(2) as usize;
    for (i, e) in entries.iter().enumerate() {
        let y = area.y + 1 + i as u16;
        if y >= area.bottom().saturating_sub(1) {
            break;
        }
        let row = Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1);
        match e {
            MenuEntry::Separator => {
                let line = theme.hline().repeat(inner_w);
                frame.render_widget(Paragraph::new(line).style(normal), row);
            }
            MenuEntry::Item { label, cmd } => {
                let on = enabled(*cmd);
                let (st, hot) = match (on, selected == Some(i)) {
                    (false, _) => (theme.get(Role::MenuDisabled), theme.get(Role::MenuDisabled)),
                    (true, true) => (
                        theme.get(Role::MenuSelected),
                        theme.get(Role::MenuSelectedHotkey),
                    ),
                    (true, false) => (normal, theme.get(Role::MenuHotkey)),
                };
                let key = shortcuts[i].clone().unwrap_or_default();
                let mut spans = vec![Span::styled(" ", st)];
                // On/off items show their state as text (D-005: not only an icon).
                let mark = match checked(*cmd) {
                    Some(true) => "[x] ",
                    Some(false) => "[ ] ",
                    None => "",
                };
                spans.push(Span::styled(mark, st));
                spans.extend(label.spans(st, hot));
                let used = 1 + mark.len() + label.width() + key.chars().count() + 1;
                spans.push(Span::styled(" ".repeat(inner_w.saturating_sub(used)), st));
                spans.push(Span::styled(format!("{key} "), st));
                frame.render_widget(Paragraph::new(Line::from(spans)), row);
                item_areas.push((i, row));
            }
        }
    }
    (area, item_areas)
}

/// A context menu (D-009, D-035) shown at `anchor`.
pub struct Popup {
    pub entries: Vec<MenuEntry>,
    pub item: Option<usize>,
    pub anchor: (u16, u16),
}

impl Popup {
    pub fn new(
        entries: Vec<MenuEntry>,
        anchor: (u16, u16),
        enabled: &dyn Fn(Command) -> bool,
    ) -> Self {
        let item = move_in(&entries, None, 1, enabled);
        Self {
            entries,
            item,
            anchor,
        }
    }

    pub fn handle_key(
        &mut self,
        cmd: Option<Command>,
        ev: &KeyEvent,
        enabled: &dyn Fn(Command) -> bool,
    ) -> MenuEvent {
        if let KeyCode::Char(c) = ev.code
            && !ev.modifiers.contains(KeyModifiers::CONTROL)
        {
            return hotkey_in(&self.entries, c, enabled).map_or(MenuEvent::None, MenuEvent::Run);
        }
        match cmd {
            Some(Command::Up | Command::PrevField) => {
                self.item = move_in(&self.entries, self.item, -1, enabled);
            }
            Some(Command::Down | Command::NextField) => {
                self.item = move_in(&self.entries, self.item, 1, enabled);
            }
            Some(Command::Home) => self.item = move_in(&self.entries, None, 1, enabled),
            Some(Command::End) => self.item = move_in(&self.entries, None, -1, enabled),
            Some(Command::Accept | Command::Toggle) => {
                if let Some(cmd) = self.item.and_then(|i| entry_cmd(&self.entries[i])) {
                    return MenuEvent::Run(cmd);
                }
            }
            Some(Command::Cancel | Command::Menu | Command::Quit | Command::ContextMenu) => {
                return MenuEvent::Close;
            }
            _ => {}
        }
        MenuEvent::None
    }
}

/// Next enabled item from `item` in direction `delta` (wrapping); `None` = start at
/// the beginning (`delta > 0`) or the end.
fn move_in(
    entries: &[MenuEntry],
    item: Option<usize>,
    delta: isize,
    enabled: &dyn Fn(Command) -> bool,
) -> Option<usize> {
    let n = entries.len() as isize;
    let mut i = item.map_or(if delta > 0 { -1 } else { n }, |i| i as isize);
    for _ in 0..n {
        i = (i + delta).rem_euclid(n);
        if entry_cmd(&entries[i as usize]).is_some_and(enabled) {
            return Some(i as usize);
        }
    }
    item
}

/// Command of the enabled item with hotkey `c`.
fn hotkey_in(entries: &[MenuEntry], c: char, enabled: &dyn Fn(Command) -> bool) -> Option<Command> {
    let c = c.to_lowercase().next().unwrap_or(c);
    entries.iter().find_map(|e| match e {
        MenuEntry::Item { label, cmd } if label.key() == Some(c) && enabled(*cmd) => Some(*cmd),
        _ => None,
    })
}

/// Command of entry `i` of menu `m`, if it is an item.
pub fn entry_command(menus: &[MenuDef], m: usize, i: usize) -> Option<Command> {
    menus.get(m)?.entries.get(i).and_then(entry_cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_are_unique_per_menu() {
        let menus = menu_bar();
        let mut titles: Vec<char> = menus.iter().filter_map(|m| m.title.key()).collect();
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), menus.len());
        for m in &menus {
            let mut keys: Vec<char> = m
                .entries
                .iter()
                .filter_map(|e| match e {
                    MenuEntry::Item { label, .. } => {
                        Some(label.key().expect("every item has a hotkey"))
                    }
                    MenuEntry::Separator => None,
                })
                .collect();
            let n = keys.len();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), n, "duplicate hotkey in {}", m.title.text);
        }
    }

    #[test]
    fn navigation_skips_disabled_items() {
        let menus = menu_bar();
        let enabled = |c: Command| matches!(c, Command::NewQuery | Command::Quit);
        let mut s = MenuState::open(&menus, 0, &enabled);
        assert_eq!(
            entry_command(&menus, 0, s.item.unwrap()),
            Some(Command::NewQuery)
        );
        s.move_item(&menus, 1, &enabled);
        assert_eq!(
            entry_command(&menus, 0, s.item.unwrap()),
            Some(Command::Quit)
        );
        s.move_item(&menus, 1, &enabled);
        assert_eq!(
            entry_command(&menus, 0, s.item.unwrap()),
            Some(Command::NewQuery)
        );
        let tools = MenuState::open(&menus, 2, &enabled);
        assert_eq!(tools.item, None);
    }
}
