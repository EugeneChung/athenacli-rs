//! Synchronous reedline REPL (Phase 1 minimal): history, `;`-terminated
//! multiline, Ctrl-C/Ctrl-D, render + status + timing.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use anyhow::Result;
use athenacli_core::config::{self, Config};
use athenacli_core::exec::SqlExecute;
use athenacli_core::output;
use inquire::Confirm;
use reedline::{
    FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus,
    Reedline, Signal, ValidationResult, Validator,
};

const HISTORY_CAPACITY: usize = 2000;

pub fn run(exec: &SqlExecute, cfg: &Config) -> Result<()> {
    let history_path = config::expand(&cfg.main.history_file);
    if let Some(dir) = Path::new(&history_path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let history = Box::new(FileBackedHistory::with_file(
        HISTORY_CAPACITY,
        PathBuf::from(history_path),
    )?);

    let mut line_editor = Reedline::create().with_history(history);
    if cfg.main.multi_line {
        line_editor = line_editor.with_validator(Box::new(SqlValidator));
    }

    let prompt = AthenaPrompt {
        left: substitute_prompt(&cfg.main.prompt, exec),
        continuation: cfg.main.prompt_continuation.clone(),
    };

    loop {
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                let text = buffer.trim();
                if text.is_empty() {
                    continue;
                }
                if matches!(text, "exit" | "quit" | "\\q") {
                    break;
                }
                run_line(exec, cfg, &buffer);
            }
            Ok(Signal::CtrlC) => continue,
            Ok(Signal::CtrlD) => break,
            Ok(_) => continue,
            Err(err) => {
                eprintln!("{err}");
                break;
            }
        }
    }
    Ok(())
}

fn run_line(exec: &SqlExecute, cfg: &Config, line: &str) {
    match exec.run(line) {
        Ok(results) => {
            for (i, rs) in results.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                if rs.run.rows.len() > output::ROW_THRESHOLD {
                    eprintln!(
                        "The result set has more than {} rows.",
                        output::ROW_THRESHOLD
                    );
                    let proceed = Confirm::new("Do you want to continue?")
                        .with_default(true)
                        .prompt()
                        .unwrap_or(false);
                    if !proceed {
                        eprintln!("Aborted!");
                        break;
                    }
                }
                let rendered = output::render(
                    &rs.run.headers,
                    &rs.run.rows,
                    &cfg.main.table_format,
                    rs.expanded,
                );
                if !rendered.is_empty() {
                    println!("{rendered}");
                }
                println!("{}", rs.status());
                if cfg.main.timing {
                    println!("Time: {:.3}s", rs.run.elapsed_ms as f64 / 1000.0);
                }
            }
        }
        Err(err) => eprintln!("{err}"),
    }
}

/// Substitute the Phase 1 prompt placeholders (`\r` region, `\d` database).
/// Date/time codes are deferred to Phase 4.
fn substitute_prompt(template: &str, exec: &SqlExecute) -> String {
    let region = exec.region.as_deref().unwrap_or("(none)");
    let database = if exec.database.is_empty() {
        "(none)"
    } else {
        &exec.database
    };
    template.replace("\\r", region).replace("\\d", database)
}

struct AthenaPrompt {
    left: String,
    continuation: String,
}

impl Prompt for AthenaPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.left)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
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

struct SqlValidator;

impl Validator for SqlValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.ends_with(';')
            || trimmed.ends_with("\\G")
            || matches!(trimmed, "exit" | "quit" | "\\q")
        {
            ValidationResult::Complete
        } else {
            ValidationResult::Incomplete
        }
    }
}
