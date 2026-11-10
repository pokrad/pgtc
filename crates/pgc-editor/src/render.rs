//! Drawing the editor: line number gutter, highlighted text, selection. Only the
//! visible lines are read and lexed.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use unicode_width::UnicodeWidthChar;

use crate::editor::Editor;
use crate::lexer::TokenKind;

/// Colours from the colour scheme's `[editor]` roles (D-014, D-038).
#[derive(Debug, Clone, Copy, Default)]
pub struct EditorStyles {
    pub text: Style,
    pub keyword: Style,
    pub string: Style,
    pub number: Style,
    pub comment: Style,
    pub quoted_ident: Style,
    pub operator: Style,
    pub selection: Style,
    pub line_number: Style,
}

impl EditorStyles {
    fn token(&self, kind: TokenKind) -> Style {
        match kind {
            TokenKind::Text => self.text,
            TokenKind::Keyword => self.keyword,
            TokenKind::String => self.string,
            TokenKind::Number => self.number,
            TokenKind::Comment => self.comment,
            TokenKind::QuotedIdent => self.quoted_ident,
            TokenKind::Operator => self.operator,
        }
    }
}

/// Width of the line number gutter (digits + one space), 0 when off.
pub fn gutter_width(editor: &Editor, line_numbers: bool) -> u16 {
    if !line_numbers {
        return 0;
    }
    let digits = editor.len_lines().to_string().len().max(3);
    digits as u16 + 1
}

/// Draws `editor` into `area`. Returns the screen position of the cursor.
pub fn render(
    editor: &Editor,
    area: Rect,
    buf: &mut Buffer,
    styles: &EditorStyles,
    line_numbers: bool,
) -> Option<Position> {
    let gutter = gutter_width(editor, line_numbers).min(area.width);
    let text_x = area.x + gutter;
    let text_w = area.width - gutter;
    buf.set_style(area, styles.text);

    let selection = editor.selection();
    let tab = editor.tab_width();
    let lines = editor.highlighted_lines(editor.top, area.height as usize);
    let mut cursor = None;
    let (cur_line, cur_col) = editor.cursor_line_col();

    for (row, (text, tokens)) in lines.iter().enumerate() {
        let line_no = editor.top + row;
        let y = area.y + row as u16;
        if gutter > 0 {
            let label = format!("{:>w$} ", line_no + 1, w = gutter as usize - 1);
            buf.set_string(area.x, y, label, styles.line_number);
        }
        let line_start = editor.line_to_char(line_no);
        let mut col = 0usize; // display column
        let mut char_idx = line_start;
        for tok in tokens {
            let style = styles.token(tok.kind);
            for c in text[tok.start..tok.end].chars() {
                let w = if c == '\t' {
                    tab - col % tab
                } else {
                    c.width().unwrap_or(0)
                };
                let selected = selection.is_some_and(|(a, b)| char_idx >= a && char_idx < b);
                let style = if selected { styles.selection } else { style };
                if w > 0 && col + w > editor.left && col < editor.left + text_w as usize {
                    let x = text_x + (col.saturating_sub(editor.left)) as u16;
                    if c == '\t' || col < editor.left {
                        // tabs, and wide chars cut at the left edge, become spaces
                        let visible =
                            (col + w).min(editor.left + text_w as usize) - col.max(editor.left);
                        for i in 0..visible {
                            buf[(x + i as u16, y)].set_char(' ').set_style(style);
                        }
                    } else if col + w <= editor.left + text_w as usize {
                        buf[(x, y)].set_char(c).set_style(style);
                    }
                }
                col += w;
                char_idx += 1;
            }
        }
        // a selected line break shows as one selected cell
        if selection.is_some_and(|(a, b)| char_idx >= a && char_idx < b)
            && col >= editor.left
            && col < editor.left + text_w as usize
        {
            let x = text_x + (col - editor.left) as u16;
            buf[(x, y)].set_style(styles.selection);
        }
        if line_no == cur_line && cur_col >= editor.left && cur_col < editor.left + text_w as usize
        {
            cursor = Some(Position::new(text_x + (cur_col - editor.left) as u16, y));
        }
    }
    if lines.is_empty() && cur_line == 0 {
        cursor = Some(Position::new(text_x, area.y));
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn draws_gutter_tokens_and_cursor() {
        let mut e = Editor::new("select 'x'\n\tfrom t", false, 4);
        e.set_view(5, 20);
        let styles = EditorStyles {
            keyword: Style::default().fg(Color::Yellow),
            string: Style::default().fg(Color::Green),
            ..EditorStyles::default()
        };
        let area = Rect::new(0, 0, 24, 5);
        let mut buf = Buffer::empty(area);
        let cursor = render(&e, area, &mut buf, &styles, true);
        let row = |y: u16| {
            (0..24)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        };
        assert!(row(0).starts_with("  1 select 'x'"), "{:?}", row(0));
        assert!(row(1).starts_with("  2     from t"), "{:?}", row(1));
        assert_eq!(buf[(4, 0)].fg, Color::Yellow);
        assert_eq!(buf[(11, 0)].fg, Color::Green);
        assert_eq!(cursor, Some(Position::new(4, 0)));
    }
}
