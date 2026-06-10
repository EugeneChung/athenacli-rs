//! SQL syntax highlighting while typing. Replaces Python `lexer.py` (a
//! MySqlLexer extended with `repair`/`offset`): the scanner's keyword set
//! already folds those in, and `highlight_spans` classifies the whole line.

use nu_ansi_term::Style;
use reedline::{Highlighter, StyledText};

use crate::parse::scanner::{highlight_spans, HighlightKind};

use super::Theme;

pub struct SqlHighlighter {
    keyword: Style,
    string: Style,
    number: Style,
    comment: Style,
}

impl SqlHighlighter {
    pub fn new(theme: &Theme) -> Self {
        Self {
            keyword: theme.sql_keyword,
            string: theme.sql_string,
            number: theme.sql_number,
            comment: theme.sql_comment,
        }
    }
}

impl Highlighter for SqlHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        for (range, kind) in highlight_spans(line) {
            let style = match kind {
                HighlightKind::Keyword => self.keyword,
                HighlightKind::Str => self.string,
                HighlightKind::Num => self.number,
                HighlightKind::Comment => self.comment,
                HighlightKind::Default => Style::default(),
            };
            styled.push((style, line[range].to_string()));
        }
        styled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_preserves_text_and_styles_keywords() {
        let theme = Theme::default();
        let hl = SqlHighlighter::new(&theme);
        let line = "SELECT a FROM t WHERE x = 'v' -- done";
        let styled = hl.highlight(line, 0);

        let rebuilt: String = styled.buffer.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(rebuilt, line);

        // Spans may carry surrounding whitespace (comment gaps), so match by
        // containment.
        let style_of = |text: &str| {
            styled
                .buffer
                .iter()
                .find(|(_, s)| s.contains(text))
                .map(|(st, _)| *st)
                .unwrap()
        };
        assert_eq!(style_of("SELECT"), theme.sql_keyword);
        assert_eq!(style_of("'v'"), theme.sql_string);
        assert_eq!(style_of("-- done"), theme.sql_comment);
        assert_eq!(style_of("a"), Style::default());
    }
}
