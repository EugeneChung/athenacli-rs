//! Styling and editor-state plumbing for the REPL: the `[colors]`-driven
//! theme, the SQL syntax highlighter, keybindings, and the F2/F3/F4 runtime
//! toggles. Approximates Python `clistyle.py` + `key_bindings.py` on top of
//! reedline (master plan risk #4: not a 1:1 prompt_toolkit reproduction).

pub mod highlight;
pub mod keybindings;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use nu_ansi_term::{Color, Style};

/// Editor state the F2/F3/F4 keybindings flip at runtime, shared between the
/// REPL loop, the completer, the validator, and the prompt (Python mutates
/// `cli.*` attributes from `key_bindings.py`; we share atomics instead).
#[derive(Debug)]
pub struct LiveToggles {
    smart_completion: AtomicBool,
    multi_line: AtomicBool,
    vi_mode: AtomicBool,
}

impl LiveToggles {
    pub fn new(smart_completion: bool, multi_line: bool, vi_mode: bool) -> Self {
        Self {
            smart_completion: AtomicBool::new(smart_completion),
            multi_line: AtomicBool::new(multi_line),
            vi_mode: AtomicBool::new(vi_mode),
        }
    }

    pub fn smart_completion(&self) -> bool {
        self.smart_completion.load(Ordering::Relaxed)
    }

    pub fn multi_line(&self) -> bool {
        self.multi_line.load(Ordering::Relaxed)
    }

    pub fn vi_mode(&self) -> bool {
        self.vi_mode.load(Ordering::Relaxed)
    }

    /// Flip and return the new value.
    pub fn toggle_smart_completion(&self) -> bool {
        !self.smart_completion.fetch_xor(true, Ordering::Relaxed)
    }

    pub fn toggle_multi_line(&self) -> bool {
        !self.multi_line.fetch_xor(true, Ordering::Relaxed)
    }

    pub fn toggle_vi_mode(&self) -> bool {
        !self.vi_mode.fetch_xor(true, Ordering::Relaxed)
    }
}

/// Resolved styles for everything we render. Defaults approximate the Pygments
/// `default` style (SQL tokens) and the Python `athenaclirc` `[colors]` section
/// (completion menu, autosuggest hint); any key present in `[colors]`
/// overrides its entry.
#[derive(Debug, Clone)]
pub struct Theme {
    pub sql_keyword: Style,
    pub sql_string: Style,
    pub sql_number: Style,
    pub sql_comment: Style,
    pub menu_text: Style,
    pub menu_selected_text: Style,
    pub hint: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            sql_keyword: Style::new().fg(Color::Green).bold(),
            sql_string: Style::new().fg(Color::Rgb(0xba, 0x21, 0x21)),
            sql_number: Style::new().fg(Color::Rgb(0x66, 0x66, 0x66)),
            sql_comment: Style::new().fg(Color::Rgb(0x40, 0x80, 0x80)).italic(),
            menu_text: parse_style("bg:#008888 #ffffff"),
            menu_selected_text: parse_style("bg:#ffffff #000000"),
            hint: Style::new().fg(Color::DarkGray).italic(),
        }
    }
}

impl Theme {
    /// Build from the config `[colors]` table. Unknown keys are ignored (the
    /// Python config carries prompt_toolkit class names we do not render).
    pub fn from_colors(colors: &HashMap<String, String>) -> Self {
        let mut theme = Theme::default();
        let apply = |key: &str, slot: &mut Style| {
            if let Some(value) = colors.get(key) {
                *slot = parse_style(value);
            }
        };
        apply("sql.keyword", &mut theme.sql_keyword);
        apply("sql.string", &mut theme.sql_string);
        apply("sql.number", &mut theme.sql_number);
        apply("sql.comment", &mut theme.sql_comment);
        apply("completion-menu.completion", &mut theme.menu_text);
        apply(
            "completion-menu.completion.current",
            &mut theme.menu_selected_text,
        );
        apply("auto-suggestion", &mut theme.hint);
        theme
    }
}

/// Parse a prompt_toolkit-flavored style string (the `[colors]` value format),
/// e.g. `'bg:#008888 #ffffff bold'`. Supported: `#rrggbb` foreground,
/// `bg:#rrggbb` background, basic ANSI color names, `bold`/`italic`/
/// `underline`. Anything else (e.g. `noinherit`, `nobold`) is ignored.
pub fn parse_style(value: &str) -> Style {
    let mut style = Style::new();
    for word in value.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        match lower.as_str() {
            "bold" => style = style.bold(),
            "italic" => style = style.italic(),
            "underline" => style = style.underline(),
            _ => {
                if let Some(bg) = lower.strip_prefix("bg:") {
                    if let Some(color) = parse_color(bg) {
                        style = style.on(color);
                    }
                } else if let Some(color) = parse_color(&lower) {
                    style = style.fg(color);
                }
            }
        }
    }
    style
}

fn parse_color(word: &str) -> Option<Color> {
    if let Some(hex) = word.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    match word {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" | "purple" => Some(Color::Purple),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "grey" | "darkgray" | "darkgrey" => Some(Color::DarkGray),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_style_ptk_format() {
        let s = parse_style("bg:#008888 #ffffff bold");
        assert_eq!(s.background, Some(Color::Rgb(0, 0x88, 0x88)));
        assert_eq!(s.foreground, Some(Color::Rgb(0xff, 0xff, 0xff)));
        assert!(s.is_bold);
    }

    #[test]
    fn parse_style_ignores_unknown_words() {
        let s = parse_style("noinherit nobold #00ff5f");
        assert_eq!(s.foreground, Some(Color::Rgb(0, 0xff, 0x5f)));
        assert!(!s.is_bold);
    }

    #[test]
    fn colors_table_overrides_defaults() {
        let mut colors = HashMap::new();
        colors.insert("sql.keyword".to_string(), "blue".to_string());
        let theme = Theme::from_colors(&colors);
        assert_eq!(theme.sql_keyword.foreground, Some(Color::Blue));
        // untouched keys keep their defaults
        assert_eq!(
            theme.menu_selected_text.background,
            Some(Color::Rgb(0xff, 0xff, 0xff))
        );
    }

    #[test]
    fn toggles_flip_and_report_new_value() {
        let t = LiveToggles::new(true, true, false);
        assert!(!t.toggle_smart_completion());
        assert!(!t.smart_completion());
        assert!(t.toggle_vi_mode());
        assert!(t.vi_mode());
    }
}
