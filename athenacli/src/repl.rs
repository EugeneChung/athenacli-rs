//! Synchronous reedline REPL: history, `;`-terminated multiline,
//! Ctrl-C/Ctrl-D, special commands, pager/tee/once output, destructive
//! confirmation, external editor round-trip.

use std::borrow::Cow;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use arc_swap::ArcSwap;
use athenacli_core::completion::completer::{AthenaCompleter, Casing};
use athenacli_core::completion::metadata::Metadata;
use athenacli_core::completion::refresher::{self, Refresher};
use athenacli_core::config::{self, Config};
use athenacli_core::exec::SqlExecute;
use athenacli_core::output::pager;
use athenacli_core::special::{self, Aborted, Emit, Flow, Session, SpecialCtx};
use athenacli_core::{athena, cancel, output};
use inquire::Confirm;
use reedline::{
    default_emacs_keybindings, ColumnarMenu, EditCommand, Emacs, FileBackedHistory, KeyCode,
    KeyModifiers, MenuBuilder, Prompt, PromptEditMode, PromptHistorySearch,
    PromptHistorySearchStatus, Reedline, ReedlineEvent, ReedlineMenu, Signal, ValidationResult,
    Validator,
};

const COMPLETION_MENU: &str = "completion_menu";

const HISTORY_CAPACITY: usize = 2000;

pub fn run(exec: &mut SqlExecute, cfg: &Config, config_path: &Path) -> Result<()> {
    let mut session = Session::from_config(cfg);
    let mut config = cfg.clone();
    let region = exec.region.clone();

    // SIGINT -> cancel flag, the Rust KeyboardInterrupt. While reedline reads,
    // Ctrl-C arrives as a key event instead, so this only fires mid-execution.
    exec.handle().spawn(async {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                break;
            }
            cancel::request();
        }
    });

    let history_path = PathBuf::from(config::expand(&cfg.main.history_file));
    if let Some(dir) = history_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    // Background metadata refresher feeds the completer lock-free via ArcSwap.
    let mut refresher = Refresher::new(exec.handle(), exec.querier());
    refresher.refresh();
    let mut line_editor = build_editor(
        &history_path,
        refresher.metadata(),
        exec.database.clone(),
        session.multi_line,
    )?;

    loop {
        let prompt = AthenaPrompt {
            left: substitute_prompt(&session.prompt_template, exec),
            continuation: session.prompt_continuation.clone(),
        };
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                let text = buffer.trim().to_string();
                if text.is_empty() {
                    continue;
                }

                // `\e`: edit in $EDITOR, then seed the next prompt with the
                // result (Python re-prompts with default=sql; repeated `\e`
                // round-trips emerge naturally).
                if special::io::editor_command(&text) {
                    let filename = special::io::get_filename(&text);
                    let mut query = special::io::get_editor_query(&text);
                    if query.trim().is_empty() {
                        query = session.last_query.clone().unwrap_or_default();
                    }
                    match special::io::open_external_editor(filename.as_deref(), &query) {
                        Ok((sql, None)) => {
                            line_editor.run_edit_commands(&[
                                EditCommand::Clear,
                                EditCommand::InsertString(sql),
                            ]);
                        }
                        Ok((_, Some(message))) => eprintln!("{message}"),
                        Err(err) => eprintln!("{err}"),
                    }
                    continue;
                }

                // Destructive confirmation on the whole line (Python
                // `one_iteration`); `watch`/`read` confirm their inner
                // statements through ctx.confirm.
                if session.destructive_warning {
                    match confirm_destructive(&text) {
                        Some(false) => {
                            println!("Wise choice!");
                            continue;
                        }
                        Some(true) => println!("Your call!"),
                        None => {}
                    }
                }

                session.write_tee(&format!("{}{}", prompt.left, text));
                cancel::reset();
                let db_before = exec.database.clone();

                let warn = session.destructive_warning;
                let mut confirm = move |q: &str| {
                    if warn {
                        confirm_destructive(q)
                    } else {
                        None
                    }
                };

                let prompt_left = prompt.left.clone();
                let region = region.clone();
                let mut result_count = 0usize;
                let mut started = Instant::now();
                let mut sink = |session: &mut Session, emit: Emit| -> Result<()> {
                    match emit {
                        Emit::Sql(rs) => {
                            if result_count > 0 {
                                println!();
                            }
                            if let Some(region) = &region {
                                println!(
                                    "Athena URL: {}",
                                    athena::console_url(region, &rs.run.query_execution_id)
                                );
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
                                    return Err(Aborted.into());
                                }
                            }
                            let rendered = output::render(
                                &rs.run.headers,
                                &rs.run.rows,
                                &session.table_format,
                                rs.expanded,
                            );
                            let status = rs.status();
                            pager::output(session, &prompt_left, &rendered, Some(&status));
                            if session.timing {
                                println!("Time: {:.3}s", rs.run.elapsed_ms as f64 / 1000.0);
                            }
                        }
                        Emit::Special(sr) => {
                            if result_count > 0 {
                                println!();
                            }
                            let mut content = String::new();
                            if let Some(title) = &sr.title {
                                content.push_str(title);
                            }
                            let rendered =
                                output::render(&sr.headers, &sr.rows, &session.table_format, false);
                            if !rendered.is_empty() {
                                if !content.is_empty() {
                                    content.push('\n');
                                }
                                content.push_str(&rendered);
                            }
                            pager::output(session, &prompt_left, &content, sr.status.as_deref());
                            if session.timing {
                                println!("Time: {:.3}s", started.elapsed().as_secs_f64());
                            }
                        }
                        Emit::ClearScreen => special::io::clear_screen(),
                    }
                    result_count += 1;
                    started = Instant::now();
                    Ok(())
                };

                let flow = {
                    let mut ctx = SpecialCtx {
                        exec,
                        session: &mut session,
                        config: &mut config,
                        config_path,
                        confirm: &mut confirm,
                    };
                    special::run_line(&mut ctx, &text, &mut sink)
                };

                match flow {
                    Ok(Flow::Exit) => break,
                    Ok(Flow::Continue) => {}
                    // Ctrl-C / declined row threshold: already reported.
                    Err(e) if e.is::<cancel::Cancelled>() => println!(),
                    Err(e) if e.is::<Aborted>() => {}
                    Err(e) => eprintln!("{e}"),
                }
                session.unset_once_if_written();
                session.last_query = Some(text.clone());

                if exec.database != db_before {
                    // `use`: the completer/refresher capture the database, so
                    // rebuild them around the new one.
                    refresher = Refresher::new(exec.handle(), exec.querier());
                    refresher.refresh();
                    line_editor = build_editor(
                        &history_path,
                        refresher.metadata(),
                        exec.database.clone(),
                        session.multi_line,
                    )?;
                } else if refresher::need_refresh(&text) {
                    // DDL/USE can change schema -> refresh completion metadata.
                    refresher.refresh();
                }
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
    session.close_tee();
    Ok(())
}

fn build_editor(
    history_path: &Path,
    metadata: Arc<ArcSwap<Metadata>>,
    database: String,
    multi_line: bool,
) -> Result<Reedline> {
    let history = Box::new(FileBackedHistory::with_file(
        HISTORY_CAPACITY,
        history_path.to_path_buf(),
    )?);
    let completer = AthenaCompleter::new(metadata, database, Casing::Auto);

    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );

    let mut line_editor = Reedline::create()
        .with_history(history)
        .with_completer(Box::new(completer))
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(
            ColumnarMenu::default().with_name(COMPLETION_MENU),
        )))
        .with_edit_mode(Box::new(Emacs::new(keybindings)));
    if multi_line {
        line_editor = line_editor.with_validator(Box::new(SqlValidator));
    }
    Ok(line_editor)
}

/// Python `confirm_destructive_query`: `None` when not destructive or stdin
/// is not a TTY; otherwise the user's choice.
pub fn confirm_destructive(query: &str) -> Option<bool> {
    if !special::is_destructive(query) || !std::io::stdin().is_terminal() {
        return None;
    }
    eprintln!("You're about to run a destructive command.");
    Some(
        Confirm::new("Do you want to proceed?")
            .with_default(false)
            .prompt()
            .unwrap_or(false),
    )
}

/// Substitute the prompt placeholders (`\r` region, `\d` database,
/// `\w` workgroup). Date/time codes are deferred to Phase 4.
fn substitute_prompt(template: &str, exec: &SqlExecute) -> String {
    let region = exec.region.as_deref().unwrap_or("(none)");
    let database = if exec.database.is_empty() {
        "(none)"
    } else {
        &exec.database
    };
    template
        .replace("\\r", region)
        .replace("\\d", database)
        .replace("\\w", exec.work_group())
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
    /// Python `clibuffer._multiline_exception`, verbatim.
    fn validate(&self, line: &str) -> ValidationResult {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('\\') // special command
            || trimmed.ends_with(';')
            || trimmed.ends_with("\\g")
            || trimmed.ends_with("\\G")
            || matches!(trimmed, "exit" | "quit" | ":q")
        {
            ValidationResult::Complete
        } else {
            ValidationResult::Incomplete
        }
    }
}
