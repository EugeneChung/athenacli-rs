//! Synchronous reedline REPL: history, `;`-terminated multiline,
//! Ctrl-C/Ctrl-D, special commands, pager/tee/once output, destructive
//! confirmation, external editor round-trip.

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
use athenacli_core::prompt::AthenaPrompt;
use athenacli_core::special::{self, Aborted, Emit, Flow, Session, SpecialCtx};
use athenacli_core::style::highlight::SqlHighlighter;
use athenacli_core::style::keybindings::{
    self, COMPLETION_MENU, TOGGLE_EDIT_MODE, TOGGLE_MULTI_LINE, TOGGLE_SMART_COMPLETION,
};
use athenacli_core::style::{LiveToggles, Theme};
use athenacli_core::{athena, cancel, output};
use inquire::Confirm;
use reedline::{
    ColumnarMenu, DefaultHinter, EditCommand, FileBackedHistory, MenuBuilder, Reedline,
    ReedlineMenu, Signal, ValidationResult, Validator,
};

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

    // F2/F3/F4 runtime state, shared with the completer/validator/prompt.
    let toggles = Arc::new(LiveToggles::new(
        true,
        cfg.main.multi_line,
        cfg.main.key_bindings.eq_ignore_ascii_case("vi"),
    ));
    let theme = Theme::from_colors(&cfg.colors);

    // Background metadata refresher feeds the completer lock-free via ArcSwap.
    let mut refresher = Refresher::new(exec.handle(), exec.querier());
    refresher.refresh();
    let mut line_editor = build_editor(
        &history_path,
        refresher.metadata(),
        exec.database.clone(),
        &toggles,
        &theme,
    )?;

    loop {
        let prompt = AthenaPrompt {
            template: session.prompt_template.clone(),
            continuation: session.prompt_continuation.clone(),
            region: exec.region.clone(),
            database: exec.database.clone(),
            work_group: exec.work_group().to_string(),
            toggles: toggles.clone(),
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

                let prompt_left = prompt.left_text();
                session.write_tee(&format!("{prompt_left}{text}"));
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
                        &toggles,
                        &theme,
                    )?;
                } else if refresher::need_refresh(&text) {
                    // DDL/USE can change schema -> refresh completion metadata.
                    refresher.refresh();
                }
            }
            // F2/F3/F4: reedline suspends the typed buffer, we apply the
            // toggle, and the next read_line restores it (Python flips these
            // inside `key_bindings.py`; reedline keybindings cannot reach
            // editor state, hence the host round-trip).
            Ok(Signal::HostCommand(cmd)) => {
                match cmd.as_str() {
                    TOGGLE_SMART_COMPLETION => {
                        toggles.toggle_smart_completion();
                    }
                    TOGGLE_MULTI_LINE => {
                        toggles.toggle_multi_line();
                    }
                    TOGGLE_EDIT_MODE => {
                        // Only the edit mode is swapped; buffer, history, and
                        // menu survive the rebuild.
                        let vi = toggles.toggle_vi_mode();
                        line_editor = line_editor.with_edit_mode(keybindings::edit_mode(vi));
                    }
                    _ => {}
                }
                continue;
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
    toggles: &Arc<LiveToggles>,
    theme: &Theme,
) -> Result<Reedline> {
    let history = Box::new(FileBackedHistory::with_file(
        HISTORY_CAPACITY,
        history_path.to_path_buf(),
    )?);
    let completer = AthenaCompleter::new(metadata, database, Casing::Auto, toggles.clone());
    let menu = ColumnarMenu::default()
        .with_name(COMPLETION_MENU)
        .with_text_style(theme.menu_text)
        .with_match_text_style(theme.menu_text)
        .with_selected_text_style(theme.menu_selected_text)
        .with_selected_match_text_style(theme.menu_selected_text);

    let line_editor = Reedline::create()
        .with_history(history)
        .with_completer(Box::new(completer))
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(menu)))
        .with_highlighter(Box::new(SqlHighlighter::new(theme)))
        // Python's AutoSuggestFromHistory: gray inline hint, Right to accept.
        .with_hinter(Box::new(DefaultHinter::default().with_style(theme.hint)))
        // The validator stays attached; F3 flips behavior through the shared
        // toggles (a single-line session validates everything as complete).
        .with_validator(Box::new(SqlValidator {
            toggles: toggles.clone(),
        }))
        .with_edit_mode(keybindings::edit_mode(toggles.vi_mode()));
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

struct SqlValidator {
    toggles: Arc<LiveToggles>,
}

impl Validator for SqlValidator {
    /// Python `clibuffer._multiline_exception`, verbatim — gated on the F3
    /// multiline toggle (off means every entry submits immediately).
    fn validate(&self, line: &str) -> ValidationResult {
        let trimmed = line.trim();
        if !self.toggles.multi_line()
            || trimmed.is_empty()
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
