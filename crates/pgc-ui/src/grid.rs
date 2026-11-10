//! Result grid (D-019, D-041): one result set, drawn cell by cell for the visible part
//! only. NULL as `[null]`, line breaks / tabs as `↵` / `→`, widths capped at 40,
//! numbers right-aligned, a row-number column, a cell cursor.

use pgc_db::{Column, Value};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

/// Widest a column gets (longer values are cut with `…`, D-041).
pub const MAX_COL_WIDTH: usize = 40;
const NULL: &str = "[null]";

/// Type OIDs shown right-aligned.
pub fn is_numeric(oid: u32) -> bool {
    // int2, int4, int8, oid, float4, float8, money, numeric
    matches!(oid, 21 | 23 | 20 | 26 | 700 | 701 | 790 | 1700)
}

/// A value for display: line breaks and tabs become visible symbols.
pub fn display(value: &str) -> String {
    value
        .replace("\r\n", "↵")
        .replace(['\n', '\r'], "↵")
        .replace('\t', "→")
}

#[derive(Debug, Clone)]
pub struct ResultSet {
    pub columns: Vec<Column>,
    widths: Vec<usize>,
    pub rows: Vec<Vec<Value>>,
    /// Paused: more rows are available on the server (D-019).
    pub more: bool,
    /// A fetch request is on its way.
    pub fetching: bool,
    /// Cell cursor.
    pub row: usize,
    pub col: usize,
    /// First visible row / column.
    pub top: usize,
    pub left: usize,
    /// Ctrl+A: copy takes the whole result set.
    pub all_selected: bool,
    /// Rows in the whole result after an export took the rest from the stream
    /// (D-042); the grid keeps only the rows it had loaded.
    pub exported_total: Option<u64>,
    /// Label of a psql-style command's result (D-054).
    pub title: Option<String>,
}

impl ResultSet {
    pub fn new(columns: Vec<Column>) -> Self {
        let widths = columns
            .iter()
            .map(|c| c.name.width().clamp(4, MAX_COL_WIDTH))
            .collect();
        Self {
            columns,
            widths,
            rows: Vec::new(),
            more: false,
            fetching: false,
            row: 0,
            col: 0,
            top: 0,
            left: 0,
            all_selected: false,
            exported_total: None,
            title: None,
        }
    }

    pub fn push_rows(&mut self, rows: Vec<Vec<Value>>) {
        for row in &rows {
            for (w, v) in self.widths.iter_mut().zip(row) {
                let len = match v {
                    Some(s) => display(s).width(),
                    None => NULL.len(),
                };
                *w = (*w).max(len.min(MAX_COL_WIDTH));
            }
        }
        self.rows.extend(rows);
    }

    /// Sub-tab label, e.g. `1: 120 rows` / `2: 1,000+ rows` (D-019).
    pub fn label(&self, number: usize) -> String {
        if let Some(total) = self.exported_total {
            return format!(
                "{number}: {} of {} rows (rest exported)",
                thousands(self.rows.len()),
                thousands(total as usize)
            );
        }
        let plus = if self.more { "+" } else { "" };
        let n = self.rows.len();
        let word = if n == 1 && !self.more { "row" } else { "rows" };
        match &self.title {
            Some(t) => format!("{number}: {t} · {}{plus} {word}", thousands(n)),
            None => format!("{number}: {}{plus} {word}", thousands(n)),
        }
    }

    fn row_number_width(&self) -> usize {
        thousands(self.rows.len()).len().max(3) + 1
    }

    /// Moves the cell cursor; `page` = visible rows.
    pub fn move_cursor(&mut self, drow: isize, dcol: isize, page: usize) {
        let _ = page;
        self.all_selected = false;
        if !self.rows.is_empty() {
            let max = self.rows.len() as isize - 1;
            self.row = (self.row as isize).saturating_add(drow).clamp(0, max) as usize;
        }
        if !self.columns.is_empty() {
            let max = self.columns.len() as isize - 1;
            self.col = (self.col as isize).saturating_add(dcol).clamp(0, max) as usize;
        }
    }

    /// Keeps the cursor cell visible in a grid of `height` data rows and `width` cells.
    pub fn ensure_visible(&mut self, height: usize, width: usize) {
        if self.row < self.top {
            self.top = self.row;
        } else if height > 0 && self.row >= self.top + height {
            self.top = self.row + 1 - height;
        }
        if self.col < self.left {
            self.left = self.col;
        }
        // scroll right until the cursor column fits
        let avail = width.saturating_sub(self.row_number_width());
        while self.left < self.col {
            let used: usize = self.widths[self.left..=self.col]
                .iter()
                .map(|w| w + 1)
                .sum();
            if used <= avail {
                break;
            }
            self.left += 1;
        }
    }

    /// The cursor cell (full value, unescaped), or the whole set after Ctrl+A as
    /// tab-separated text with a header row (D-041).
    pub fn copy_text(&self) -> Option<String> {
        if self.all_selected {
            let mut out = self
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join("\t");
            out.push('\n');
            for row in &self.rows {
                let line: Vec<&str> = row.iter().map(|v| v.as_deref().unwrap_or("")).collect();
                out.push_str(&line.join("\t"));
                out.push('\n');
            }
            return Some(out);
        }
        let v = self.rows.get(self.row)?.get(self.col)?;
        Some(v.as_deref().unwrap_or("").to_owned())
    }
}

/// `1234567` → `1,234,567`
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Grid colours (theme roles, D-014).
#[derive(Debug, Clone, Copy, Default)]
pub struct GridStyles {
    pub text: Style,
    pub header: Style,
    pub null: Style,
    pub cursor: Style,
    pub row_number: Style,
    /// Vertical lines between columns.
    pub separator: Style,
    /// Column lines as `|` (ASCII frames, D-051).
    pub ascii: bool,
}

fn fit(text: &str, width: usize, right: bool) -> String {
    let w = text.width();
    if w > width {
        let mut out = String::new();
        let mut used = 0;
        for c in text.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if used + cw + 1 > width {
                break;
            }
            out.push(c);
            used += cw;
        }
        out.push('…');
        used += 1;
        out.push_str(&" ".repeat(width.saturating_sub(used)));
        out
    } else if right {
        format!("{}{text}", " ".repeat(width - w))
    } else {
        format!("{text}{}", " ".repeat(width - w))
    }
}

/// Area of a visible cell, by (row, column).
pub type CellArea = ((usize, usize), Rect);

/// Draws the visible part of `rs` into `area` (header row + data rows). Returns the
/// areas of the visible data cells (for mouse clicks).
pub fn render(
    rs: &ResultSet,
    area: Rect,
    buf: &mut Buffer,
    styles: &GridStyles,
    focused: bool,
) -> Vec<CellArea> {
    let mut cells = Vec::new();
    buf.set_style(area, styles.text);
    if area.height == 0 {
        return cells;
    }
    let rn = rs.row_number_width();
    let rows_visible = area.height as usize - 1;
    let right = area.x + area.width;
    // header; a vertical line ends the row-number column and every column
    let line = if styles.ascii { "|" } else { "│" };
    buf.set_stringn(area.x, area.y, " ".repeat(rn - 1), rn - 1, styles.header);
    buf.set_stringn(area.x + rn as u16 - 1, area.y, line, 1, styles.header);
    let mut x = area.x + rn as u16;
    for (c, col) in rs.columns.iter().enumerate().skip(rs.left) {
        if x >= right {
            break;
        }
        let w = rs.widths[c];
        let text = fit(&col.name, w, false);
        let avail = (right - x) as usize;
        buf.set_stringn(x, area.y, &text, avail, styles.header);
        if (x as usize + w) < right as usize {
            buf.set_stringn(x + w as u16, area.y, line, 1, styles.header);
        }
        x = x.saturating_add(w as u16 + 1);
    }
    // rows
    for (i, row) in rs.rows.iter().enumerate().skip(rs.top).take(rows_visible) {
        let y = area.y + 1 + (i - rs.top) as u16;
        let label = format!("{:>w$}", thousands(i + 1), w = rn - 1);
        buf.set_stringn(area.x, y, &label, rn - 1, styles.row_number);
        buf.set_stringn(area.x + rn as u16 - 1, y, line, 1, styles.separator);
        let mut x = area.x + rn as u16;
        for (c, col) in rs.columns.iter().enumerate().skip(rs.left) {
            if x >= right {
                break;
            }
            let w = rs.widths[c];
            let (text, mut style) = match row.get(c).and_then(|v| v.as_deref()) {
                Some(v) => (fit(&display(v), w, is_numeric(col.type_oid)), styles.text),
                None => (fit(NULL, w, false), styles.null),
            };
            let cursor = focused && (rs.all_selected || (i == rs.row && c == rs.col));
            if cursor {
                style = styles.cursor;
            }
            let avail = (right - x) as usize;
            buf.set_stringn(x, y, &text, avail, style);
            if (x as usize + w) < right as usize {
                buf.set_stringn(x + w as u16, y, line, 1, styles.separator);
            }
            cells.push(((i, c), Rect::new(x, y, (w + 1).min(avail) as u16, 1)));
            x = x.saturating_add(w as u16 + 1);
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rs() -> ResultSet {
        let mut rs = ResultSet::new(vec![
            Column {
                name: "id".into(),
                type_oid: 23,
            },
            Column {
                name: "note".into(),
                type_oid: 25,
            },
        ]);
        rs.push_rows(vec![
            vec![Some("1".into()), Some("a\nb\tc".into())],
            vec![Some("22".into()), None],
            vec![Some("3".into()), Some("x".repeat(100).into())],
        ]);
        rs
    }

    #[test]
    fn widths_labels_and_copy() {
        let mut r = rs();
        assert_eq!(r.widths, vec![4, MAX_COL_WIDTH]);
        assert_eq!(r.label(1), "1: 3 rows");
        r.more = true;
        assert_eq!(r.label(1), "1: 3+ rows");
        r.move_cursor(0, 1, 10);
        assert_eq!(
            r.copy_text().as_deref(),
            Some("a\nb\tc"),
            "copied unchanged"
        );
        r.move_cursor(1, 0, 10);
        assert_eq!(r.copy_text().as_deref(), Some(""));
        r.all_selected = true;
        let all = r.copy_text().unwrap();
        assert!(all.starts_with("id\tnote\n1\ta\nb\tc\n"));
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(display("a\r\nb\tc"), "a↵b→c");
    }

    #[test]
    fn draws_header_rows_null_and_alignment() {
        let r = rs();
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        render(&r, area, &mut buf, &GridStyles::default(), true);
        let line = |y: u16| {
            (0..30)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        };
        assert!(line(0).starts_with("   │id  │note"), "{:?}", line(0));
        assert!(line(1).starts_with("  1│   1│a↵b→c"), "{:?}", line(1));
        assert!(line(2).starts_with("  2│  22│[null]"), "{:?}", line(2));
        assert!(line(3).contains("xxx"), "{:?}", line(3));
    }

    #[test]
    fn cursor_scrolls_into_view() {
        let mut r = rs();
        r.move_cursor(2, 1, 1);
        r.ensure_visible(1, 20);
        assert_eq!(r.top, 2);
        assert_eq!(
            r.left, 1,
            "column 2 does not fit next to column 1 in 20 cells"
        );
    }
}
