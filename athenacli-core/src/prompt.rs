//! Prompt rendering: template substitution (Python `get_prompt`), the
//! too-long fallback (Python `get_message`), and the reedline `Prompt`
//! implementation with the bottom-toolbar approximation in the right prompt
//! (reedline has no bottom toolbar; master plan risk #4).

use std::borrow::Cow;
use std::sync::Arc;

use chrono::{DateTime, Local};
use reedline::{
    Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, PromptViMode,
};

use crate::style::LiveToggles;

/// Python `AthenaCli.MAX_LEN_PROMPT`: substituted prompts longer than this
/// fall back to [`SHORT_PROMPT`].
pub const MAX_LEN_PROMPT: usize = 45;

/// Python's fallback template in `get_message`.
pub const SHORT_PROMPT: &str = r"\r:\d> ";

/// Substitute the prompt template, mirroring Python `get_prompt` (same token
/// set and replacement order) plus the `\w` workgroup token this port already
/// supported in earlier phases.
pub fn substitute(
    template: &str,
    region: Option<&str>,
    database: &str,
    work_group: &str,
    now: &DateTime<Local>,
) -> String {
    let database = if database.is_empty() {
        "(none)"
    } else {
        database
    };
    template
        .replace("\\r", region.unwrap_or("(none)"))
        .replace("\\d", database)
        .replace("\\w", work_group)
        .replace("\\n", "\n")
        .replace("\\D", &now.format("%a %b %d %H:%M:%S %Y").to_string())
        .replace("\\m", &now.format("%M").to_string())
        .replace("\\P", &now.format("%p").to_string())
        .replace("\\R", &now.format("%H").to_string())
        .replace("\\s", &now.format("%S").to_string())
}

/// Substituted prompt with the Python `get_message` fallback: when the result
/// exceeds [`MAX_LEN_PROMPT`] characters, re-render with [`SHORT_PROMPT`].
pub fn prompt_text(
    template: &str,
    region: Option<&str>,
    database: &str,
    work_group: &str,
    now: &DateTime<Local>,
) -> String {
    let prompt = substitute(template, region, database, work_group, now);
    if prompt.chars().count() > MAX_LEN_PROMPT {
        substitute(SHORT_PROMPT, region, database, work_group, now)
    } else {
        prompt
    }
}

/// The REPL prompt. Region/database/workgroup are snapshots taken when the
/// REPL loop (re)builds it after each submission; date/time tokens are
/// substituted on every render so they stay current while typing.
pub struct AthenaPrompt {
    pub template: String,
    pub continuation: String,
    pub region: Option<String>,
    pub database: String,
    pub work_group: String,
    pub toggles: Arc<LiveToggles>,
}

impl AthenaPrompt {
    /// Left prompt as of now — also used by the REPL to echo the line into
    /// tee/pager output.
    pub fn left_text(&self) -> String {
        prompt_text(
            &self.template,
            self.region.as_deref(),
            &self.database,
            &self.work_group,
            &Local::now(),
        )
    }
}

impl Prompt for AthenaPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(self.left_text())
    }

    /// Bottom-toolbar approximation (Python `clitoolbar.py`): the toggle
    /// states, refreshed every render.
    fn render_prompt_right(&self) -> Cow<'_, str> {
        let on_off = |on: bool| if on { "ON" } else { "OFF" };
        Cow::Owned(format!(
            "[F2] Smart: {}  [F3] Multiline: {}  [F4] {}",
            on_off(self.toggles.smart_completion()),
            on_off(self.toggles.multi_line()),
            if self.toggles.vi_mode() {
                "Vi"
            } else {
                "Emacs"
            },
        ))
    }

    /// Vi sub-mode marker (Python toolbar's `Vi-mode (I/N)`), fixed-width so
    /// the input text does not shift between insert and normal.
    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<'_, str> {
        match edit_mode {
            PromptEditMode::Vi(PromptViMode::Insert) => Cow::Borrowed("(I) "),
            PromptEditMode::Vi(PromptViMode::Normal) => Cow::Borrowed("(N) "),
            _ => Cow::Borrowed(""),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.continuation)
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 10, h, m, s).unwrap()
    }

    #[test]
    fn substitutes_connection_tokens() {
        let now = at(9, 5, 7);
        assert_eq!(
            substitute(r"\d@\r> ", Some("us-east-1"), "sales", "primary", &now),
            "sales@us-east-1> "
        );
        assert_eq!(
            substitute(r"\r:\w> ", None, "", "primary", &now),
            "(none):primary> "
        );
    }

    #[test]
    fn substitutes_date_tokens_like_python_strftime() {
        let now = at(14, 5, 7);
        assert_eq!(
            substitute(r"\D", None, "", "", &now),
            "Wed Jun 10 14:05:07 2026"
        );
        assert_eq!(
            substitute(r"\R:\m:\s \P", None, "", "", &now),
            "14:05:07 PM"
        );
        assert_eq!(substitute(r"a\nb", None, "", "", &now), "a\nb");
    }

    #[test]
    fn long_prompt_falls_back_to_short_form() {
        let now = at(9, 0, 0);
        let long_db = "a".repeat(50);
        let text = prompt_text(r"\d> ", Some("us-east-1"), &long_db, "", &now);
        assert_eq!(text, format!("us-east-1:{long_db}> "));

        // \D alone stays within the limit, no fallback.
        assert_eq!(
            prompt_text(r"\D> ", Some("r"), "db", "", &now),
            "Wed Jun 10 09:00:00 2026> "
        );
    }
}
