//! Labels with a highlighted hotkey letter, written as `"&File"` (`&&` = literal `&`).

use ratatui::style::Style;
use ratatui::text::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotText {
    pub text: String,
    /// Char index of the hotkey letter.
    pub hot: Option<usize>,
}

impl HotText {
    pub fn new(source: &str) -> Self {
        let mut text = String::new();
        let mut hot = None;
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '&' {
                match chars.next() {
                    Some('&') => text.push('&'),
                    Some(h) => {
                        hot.get_or_insert(text.chars().count());
                        text.push(h);
                    }
                    None => {}
                }
            } else {
                text.push(c);
            }
        }
        Self { text, hot }
    }

    /// Lower-case hotkey letter.
    pub fn key(&self) -> Option<char> {
        let i = self.hot?;
        self.text
            .chars()
            .nth(i)
            .map(|c| c.to_lowercase().next().unwrap_or(c))
    }

    pub fn width(&self) -> usize {
        self.text.chars().count()
    }

    /// Spans with the hotkey letter in `hot` style.
    pub fn spans(&self, normal: Style, hot: Style) -> Vec<Span<'static>> {
        let Some(i) = self.hot else {
            return vec![Span::styled(self.text.clone(), normal)];
        };
        let before: String = self.text.chars().take(i).collect();
        let letter: String = self.text.chars().skip(i).take(1).collect();
        let after: String = self.text.chars().skip(i + 1).collect();
        vec![
            Span::styled(before, normal),
            Span::styled(letter, hot),
            Span::styled(after, normal),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hotkey_and_escapes() {
        let h = HotText::new("Com&pare && sync");
        assert_eq!(h.text, "Compare & sync");
        assert_eq!(h.key(), Some('p'));
        assert_eq!(HotText::new("&File").key(), Some('f'));
        assert_eq!(HotText::new("Plain").key(), None);
    }
}
