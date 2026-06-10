//! Special (backslash/name) commands: registry, parsing, and dispatch.
//! Mirrors Python `packages/special/main.py` plus the per-statement dispatch
//! hook from `sqlexecute.run`. The registry is a fixed table built once —
//! no decorator magic (master plan decision).

pub mod db;
pub mod download;
pub mod favorites;
pub mod io;

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use crate::config::Config;
use crate::exec::{split_statements, ResultSet, SqlExecute};

/// Python `NO_QUERY` / `PARSED_QUERY` / `RAW_QUERY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgType {
    NoQuery,
    Parsed,
    Raw,
}

/// What the REPL should do after a dispatched line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// `exit` / `quit` / `\q` (Python raises `EOFError`).
    Exit,
}

/// A special command's table-shaped output, the Rust equivalent of Python's
/// `(title, rows, headers, status)` tuple.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecialResult {
    pub title: Option<String>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub status: Option<String>,
}

impl SpecialResult {
    pub fn message(status: impl Into<String>) -> Self {
        Self {
            status: Some(status.into()),
            ..Self::default()
        }
    }

    pub fn table(headers: Vec<String>, rows: Vec<Vec<Option<String>>>) -> Self {
        Self {
            headers,
            rows,
            ..Self::default()
        }
    }
}

/// One unit of output pushed to the caller as it is produced (Python handlers
/// are generators; `watch` yields forever).
#[derive(Debug)]
pub enum Emit {
    /// Regular SQL result: console URL + status line + timing in the REPL.
    Sql(ResultSet),
    /// Special command output: optional title + table + status only.
    Special(SpecialResult),
    /// `watch -c`: clear the screen before the next round.
    ClearScreen,
}

/// Output callback. Receives the session explicitly so handlers (which hold
/// `&mut Session` through their ctx) can hand it to the sink per call.
pub type Sink<'s> = dyn FnMut(&mut Session, Emit) -> anyhow::Result<()> + 's;

/// Marker error: the user declined the "more than N rows" confirmation.
/// The REPL prints nothing for it (the sink already echoed `Aborted!`).
#[derive(Debug, thiserror::Error)]
#[error("aborted")]
pub struct Aborted;

/// Tri-state destructive confirmation, Python `confirm_destructive_query`:
/// `None` = not destructive (or no TTY), `Some(true)` = proceed,
/// `Some(false)` = abort.
pub type Confirm<'c> = dyn FnMut(&str) -> Option<bool> + 'c;

/// Per-session mutable state, replacing Python's module globals
/// (`iocommands.py`) and `AthenaCli` fields.
pub struct Session {
    pub timing: bool,
    pub table_format: String,
    pub prompt_template: String,
    pub prompt_continuation: String,
    pub multi_line: bool,
    pub pager_enabled: bool,
    pub destructive_warning: bool,
    pub last_query: Option<String>,
    pub last_output_location: Option<String>,
    tee: Option<std::fs::File>,
    once: Option<OnceFile>,
    written_to_once: bool,
}

struct OnceFile {
    path: std::path::PathBuf,
    overwrite: bool,
    handle: Option<std::fs::File>,
}

impl Session {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            timing: cfg.main.timing,
            table_format: cfg.main.table_format.clone(),
            prompt_template: cfg.main.prompt.clone(),
            prompt_continuation: cfg.main.prompt_continuation.clone(),
            multi_line: cfg.main.multi_line,
            pager_enabled: cfg.main.enable_pager,
            destructive_warning: matches!(
                cfg.main.destructive_warning.to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            ),
            last_query: None,
            last_output_location: None,
            tee: None,
            once: None,
            written_to_once: false,
        }
    }

    pub fn set_tee(&mut self, path: &Path, overwrite: bool) -> anyhow::Result<()> {
        let file = open_for(path, overwrite)
            .map_err(|e| anyhow::anyhow!("Cannot write to file '{}': {e}", path.display()))?;
        self.tee = Some(file);
        Ok(())
    }

    pub fn close_tee(&mut self) {
        self.tee = None;
    }

    pub fn set_once(&mut self, path: std::path::PathBuf, overwrite: bool) {
        self.once = Some(OnceFile {
            path,
            overwrite,
            handle: None,
        });
    }

    /// Write one output line to the tee file, if armed (Python `write_tee`).
    pub fn write_tee(&mut self, line: &str) {
        use std::io::Write;
        if let Some(f) = &mut self.tee {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }

    /// Write one output line to the once file, if armed (Python `write_once`).
    pub fn write_once(&mut self, line: &str) {
        use std::io::Write;
        if line.is_empty() {
            return;
        }
        let Some(once) = &mut self.once else { return };
        if once.handle.is_none() {
            match open_for(&once.path, once.overwrite) {
                Ok(f) => once.handle = Some(f),
                Err(e) => {
                    let path = once.path.display().to_string();
                    self.once = None;
                    eprintln!("Cannot write to file '{path}': {e}");
                    return;
                }
            }
        }
        if let Some(f) = &mut once.handle {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
            self.written_to_once = true;
        }
    }

    /// Python `unset_once_if_written`: disarm `\once` after a result landed.
    pub fn unset_once_if_written(&mut self) {
        if self.written_to_once {
            self.once = None;
            self.written_to_once = false;
        }
    }
}

fn open_for(path: &Path, overwrite: bool) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(overwrite)
        .append(!overwrite)
        .open(path)
}

/// Everything a handler may need, replacing Python's globals + `cur`.
pub struct SpecialCtx<'a> {
    pub exec: &'a mut SqlExecute,
    pub session: &'a mut Session,
    pub config: &'a mut Config,
    pub config_path: &'a Path,
    pub confirm: &'a mut Confirm<'a>,
}

pub struct Invocation {
    pub arg: String,
    pub verbose: bool,
    /// The full statement, for `RawQuery` commands.
    pub raw: String,
}

type Handler = fn(&mut SpecialCtx, &Invocation, &mut Sink) -> anyhow::Result<Flow>;

pub struct SpecialCommand {
    handler: Handler,
    pub command: &'static str,
    pub shortcut: &'static str,
    pub description: &'static str,
    pub arg_type: ArgType,
    pub hidden: bool,
    pub case_sensitive: bool,
}

/// Split `sql` into `(command, verbose, arg)`, Python `parse_special_command`.
pub fn parse_special_command(sql: &str) -> (String, bool, String) {
    let (command, arg) = match sql.split_once(' ') {
        Some((c, a)) => (c, a),
        None => (sql, ""),
    };
    let verbose = command.contains('+');
    let command = command.trim().replace('+', "");
    (command, verbose, arg.trim().to_string())
}

struct Entry {
    key: &'static str,
    cmd: SpecialCommand,
}

#[allow(clippy::too_many_arguments)]
fn entry(
    key: &'static str,
    handler: Handler,
    command: &'static str,
    shortcut: &'static str,
    description: &'static str,
    arg_type: ArgType,
    hidden: bool,
    case_sensitive: bool,
) -> Entry {
    Entry {
        key,
        cmd: SpecialCommand {
            handler,
            command,
            shortcut,
            description,
            arg_type,
            hidden,
            case_sensitive,
        },
    }
}

fn registry() -> &'static HashMap<&'static str, SpecialCommand> {
    static REGISTRY: OnceLock<HashMap<&'static str, SpecialCommand>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        use ArgType::*;
        // (key, handler, command, shortcut, description, arg_type, hidden, case_sensitive)
        // Keys for case-insensitive commands are stored lowercase, like Python.
        // Alias entries repeat the handler with hidden=true.
        let entries = vec![
            entry(
                "help",
                cmd_help,
                "help",
                "\\?",
                "Show this help.",
                NoQuery,
                false,
                false,
            ),
            entry(
                "\\?",
                cmd_help,
                "help",
                "\\?",
                "Show this help.",
                NoQuery,
                true,
                false,
            ),
            entry(
                "?",
                cmd_help,
                "help",
                "\\?",
                "Show this help.",
                NoQuery,
                true,
                false,
            ),
            entry(
                "exit", cmd_quit, "exit", "\\q", "Exit.", NoQuery, false, false,
            ),
            entry(
                "\\q", cmd_quit, "exit", "\\q", "Exit.", NoQuery, true, false,
            ),
            entry(
                "quit", cmd_quit, "quit", "\\q", "Quit.", NoQuery, false, false,
            ),
            entry(
                "\\e",
                cmd_stub,
                "\\e",
                "\\e",
                "Edit command with editor (uses $EDITOR).",
                NoQuery,
                false,
                true,
            ),
            entry(
                "\\G",
                cmd_stub,
                "\\G",
                "\\G",
                "Display current query results vertically.",
                NoQuery,
                false,
                true,
            ),
            entry(
                "use",
                cmd_use,
                "use",
                "\\u",
                "Change to a new database.",
                Parsed,
                false,
                false,
            ),
            entry(
                "\\u",
                cmd_use,
                "use",
                "\\u",
                "Change to a new database.",
                Parsed,
                true,
                false,
            ),
            entry(
                "prompt",
                cmd_prompt,
                "prompt",
                "\\R",
                "Change prompt format.",
                Parsed,
                false,
                true,
            ),
            entry(
                "\\R",
                cmd_prompt,
                "prompt",
                "\\R",
                "Change prompt format.",
                Parsed,
                true,
                true,
            ),
            entry(
                "tableformat",
                cmd_tableformat,
                "tableformat",
                "\\T",
                "Change the table format used to output results.",
                Parsed,
                false,
                true,
            ),
            entry(
                "\\T",
                cmd_tableformat,
                "tableformat",
                "\\T",
                "Change the table format used to output results.",
                Parsed,
                true,
                true,
            ),
            entry(
                "\\dt",
                db::list_tables,
                "\\dt",
                "\\dt [table]",
                "List or describe tables.",
                Parsed,
                false,
                true,
            ),
            entry(
                "\\l",
                db::list_databases,
                "\\l",
                "\\l",
                "List databases.",
                Raw,
                false,
                true,
            ),
            entry(
                "pager",
                io::set_pager,
                "pager",
                "\\P [command]",
                "Set PAGER. Print the query results via PAGER.",
                Parsed,
                false,
                true,
            ),
            entry(
                "\\P",
                io::set_pager,
                "pager",
                "\\P [command]",
                "Set PAGER. Print the query results via PAGER.",
                Parsed,
                true,
                true,
            ),
            entry(
                "nopager",
                io::disable_pager,
                "nopager",
                "\\n",
                "Disable pager, print to stdout.",
                NoQuery,
                false,
                true,
            ),
            entry(
                "\\n",
                io::disable_pager,
                "nopager",
                "\\n",
                "Disable pager, print to stdout.",
                NoQuery,
                true,
                true,
            ),
            entry(
                "\\timing",
                io::toggle_timing,
                "\\timing",
                "\\t",
                "Toggle timing of commands.",
                NoQuery,
                false,
                true,
            ),
            entry(
                "\\t",
                io::toggle_timing,
                "\\timing",
                "\\t",
                "Toggle timing of commands.",
                NoQuery,
                true,
                true,
            ),
            entry(
                "tee",
                io::set_tee,
                "tee",
                "tee [-o] filename",
                "Append all results to an output file (overwrite using -o).",
                Parsed,
                false,
                false,
            ),
            entry(
                "notee",
                io::no_tee,
                "notee",
                "notee",
                "Stop writing results to an output file.",
                Parsed,
                false,
                false,
            ),
            entry(
                "\\once",
                io::set_once,
                "\\once",
                "\\o [-o] filename",
                "Append next result to an output file (overwrite using -o).",
                Parsed,
                false,
                false,
            ),
            entry(
                "\\o",
                io::set_once,
                "\\once",
                "\\o [-o] filename",
                "Append next result to an output file (overwrite using -o).",
                Parsed,
                true,
                false,
            ),
            entry(
                "\\f",
                favorites::execute_favorite,
                "\\f",
                "\\f [name [args..]]",
                "List or execute favorite queries.",
                Parsed,
                false,
                true,
            ),
            entry(
                "\\fs",
                favorites::save_favorite,
                "\\fs",
                "\\fs name query",
                "Save a favorite query.",
                Parsed,
                false,
                false,
            ),
            entry(
                "\\fd",
                favorites::delete_favorite,
                "\\fd",
                "\\fd [name]",
                "Delete a favorite query.",
                Parsed,
                false,
                false,
            ),
            entry(
                "read",
                io::read_file,
                "read",
                "read [filename]",
                "Read and execute query from a file.",
                Parsed,
                false,
                false,
            ),
            entry(
                "system",
                io::system_command,
                "system",
                "system [command]",
                "Execute a system shell command.",
                Parsed,
                false,
                false,
            ),
            entry(
                "watch",
                io::watch_query,
                "watch",
                "watch [seconds] [-c] query",
                "Executes the query every [seconds] seconds (by default 5).",
                Parsed,
                false,
                false,
            ),
            entry(
                "download",
                download::download,
                "download",
                "download",
                "Download results from last query.",
                NoQuery,
                false,
                false,
            ),
        ];
        entries.into_iter().map(|e| (e.key, e.cmd)).collect()
    })
}

/// Try to run `sql` as a special command. `Ok(None)` means "not a special
/// command" (Python `CommandNotFound`) — run it as regular SQL.
pub fn execute(ctx: &mut SpecialCtx, sql: &str, sink: &mut Sink) -> anyhow::Result<Option<Flow>> {
    let (command, verbose, arg) = parse_special_command(sql);
    let lower = command.to_lowercase();

    let reg = registry();
    let cmd = match reg.get(command.as_str()) {
        Some(c) => c,
        None => match reg.get(lower.as_str()) {
            // A case-sensitive command matched only after lowercasing — reject.
            Some(c) if c.case_sensitive => return Ok(None),
            Some(c) => c,
            None => return Ok(None),
        },
    };

    // Python special-cases `help <SQL KEYWORD>`.
    if lower == "help" && !arg.is_empty() {
        return show_keyword_help(ctx, &arg, sink).map(Some);
    }

    let inv = Invocation {
        arg: if cmd.arg_type == ArgType::NoQuery {
            String::new()
        } else {
            arg
        },
        verbose,
        raw: sql.to_string(),
    };
    (cmd.handler)(ctx, &inv, sink).map(Some)
}

/// Per-statement dispatch, the hook Python keeps in `sqlexecute.run`:
/// split the line, try special first, fall back to regular SQL.
pub fn run_line(ctx: &mut SpecialCtx, line: &str, sink: &mut Sink) -> anyhow::Result<Flow> {
    for raw in split_statements(line.trim()) {
        let trimmed = raw.trim_end_matches(';').trim();
        let (sql, expanded) = match trimmed.strip_suffix("\\G") {
            Some(rest) => (rest.trim(), true),
            None => (trimmed, false),
        };
        if sql.is_empty() {
            continue;
        }
        match execute(ctx, sql, sink)? {
            Some(Flow::Exit) => return Ok(Flow::Exit),
            Some(Flow::Continue) => {}
            None => {
                let run = ctx.exec.run_sql(sql)?;
                ctx.session.last_output_location = run.output_location.clone();
                sink(ctx.session, Emit::Sql(ResultSet { run, expanded }))?;
            }
        }
    }
    Ok(Flow::Continue)
}

fn cmd_help(ctx: &mut SpecialCtx, _inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    let mut keys: Vec<_> = registry().iter().filter(|(_, c)| !c.hidden).collect();
    keys.sort_by_key(|(k, _)| *k);
    let rows = keys
        .into_iter()
        .map(|(_, c)| {
            vec![
                Some(c.command.to_string()),
                Some(c.shortcut.to_string()),
                Some(c.description.to_string()),
            ]
        })
        .collect();
    sink(
        ctx.session,
        Emit::Special(SpecialResult::table(
            vec!["Command".into(), "Shortcut".into(), "Description".into()],
            rows,
        )),
    )?;
    Ok(Flow::Continue)
}

fn show_keyword_help(ctx: &mut SpecialCtx, arg: &str, sink: &mut Sink) -> anyhow::Result<Flow> {
    let keyword = arg.trim_matches('"').trim_matches('\'');
    let run = ctx.exec.run_sql(&format!("help '{keyword}'"))?;
    if run.has_result_set && !run.rows.is_empty() {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::table(run.headers, run.rows)),
        )?;
    } else {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message(format!(
                "No help found for {keyword}."
            ))),
        )?;
    }
    Ok(Flow::Continue)
}

fn cmd_quit(_ctx: &mut SpecialCtx, _inv: &Invocation, _sink: &mut Sink) -> anyhow::Result<Flow> {
    Ok(Flow::Exit)
}

/// `\e` / `\G` reach dispatch only outside their REPL handling (Python's stub
/// raises `NotImplementedError`, echoed as this message).
fn cmd_stub(ctx: &mut SpecialCtx, _inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    sink(
        ctx.session,
        Emit::Special(SpecialResult::message("Not Yet Implemented.")),
    )?;
    Ok(Flow::Continue)
}

fn cmd_use(ctx: &mut SpecialCtx, inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    if !inv.arg.is_empty() {
        ctx.exec.set_database(&inv.arg);
    }
    sink(
        ctx.session,
        Emit::Special(SpecialResult::message(format!(
            "You are now connected to database \"{}\"",
            ctx.exec.database
        ))),
    )?;
    Ok(Flow::Continue)
}

fn cmd_prompt(ctx: &mut SpecialCtx, inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    if inv.arg.is_empty() {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message("Missing required argument, format.")),
        )?;
        return Ok(Flow::Continue);
    }
    ctx.session.prompt_template = inv.arg.clone();
    sink(
        ctx.session,
        Emit::Special(SpecialResult::message(format!(
            "Changed prompt format to {}",
            inv.arg
        ))),
    )?;
    Ok(Flow::Continue)
}

const TABLE_FORMATS: [&str; 3] = ["ascii", "csv", "vertical"];

fn cmd_tableformat(
    ctx: &mut SpecialCtx,
    inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    let arg = inv.arg.to_lowercase();
    if TABLE_FORMATS.contains(&arg.as_str()) {
        ctx.session.table_format = arg.clone();
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message(format!(
                "Changed table format to {arg}"
            ))),
        )?;
    } else {
        let mut msg = format!("Table format {} not recognized. Allowed formats:", inv.arg);
        for f in TABLE_FORMATS {
            msg.push_str("\n\t");
            msg.push_str(f);
        }
        sink(ctx.session, Emit::Special(SpecialResult::message(msg)))?;
    }
    Ok(Flow::Continue)
}

/// Python `parseutils.is_destructive`: any statement starting with one of the
/// destructive keywords (comments skipped).
pub fn is_destructive(queries: &str) -> bool {
    const KEYWORDS: [&str; 4] = ["drop", "shutdown", "delete", "truncate"];
    split_statements(queries).iter().any(|q| {
        first_word(q)
            .map(|w| KEYWORDS.contains(&w.to_lowercase().as_str()))
            .unwrap_or(false)
    })
}

/// First word of a statement, skipping whitespace and `--` / `/* */` comments.
fn first_word(sql: &str) -> Option<String> {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map(|(_, r)| r).unwrap_or("");
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map(|(_, r)| r).unwrap_or("");
        } else {
            break;
        }
    }
    let word: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ';' && *c != '(')
        .collect();
    (!word.is_empty()).then_some(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_verbose_and_arg() {
        assert_eq!(
            parse_special_command("\\dt+ foo"),
            ("\\dt".into(), true, "foo".into())
        );
        assert_eq!(
            parse_special_command("\\dt foo"),
            ("\\dt".into(), false, "foo".into())
        );
        assert_eq!(
            parse_special_command("\\l"),
            ("\\l".into(), false, "".into())
        );
        assert_eq!(
            parse_special_command("tee  -o /tmp/out.txt"),
            ("tee".into(), false, "-o /tmp/out.txt".into())
        );
    }

    #[test]
    fn registry_contains_expected_commands() {
        let reg = registry();
        for key in [
            "help",
            "exit",
            "quit",
            "use",
            "prompt",
            "tableformat",
            "\\dt",
            "\\l",
            "pager",
            "nopager",
            "\\timing",
            "tee",
            "notee",
            "\\once",
            "\\f",
            "\\fs",
            "\\fd",
            "read",
            "system",
            "watch",
            "download",
            "\\e",
            "\\G",
        ] {
            assert!(reg.contains_key(key), "missing {key}");
        }
        // Aliases are hidden.
        assert!(reg["\\q"].hidden);
        assert!(reg["\\u"].hidden);
        assert!(!reg["help"].hidden);
    }

    #[test]
    fn case_sensitive_lookup_rules() {
        let reg = registry();
        // `\dt` is case-sensitive: an upper-cased form must not resolve.
        assert!(reg.get("\\DT").is_none());
        // case-insensitive commands are stored lowercase.
        assert!(reg.get("tee").is_some());
        assert!(reg.get("TEE").is_none()); // resolved via lowercasing in execute()
    }

    #[test]
    fn destructive_detection() {
        assert!(is_destructive("drop table foo"));
        assert!(is_destructive("DROP TABLE foo"));
        assert!(is_destructive("select 1; truncate table foo"));
        assert!(is_destructive("-- comment\ndelete from t"));
        assert!(is_destructive("/* hm */ shutdown"));
        assert!(!is_destructive("select * from drop_log"));
        assert!(!is_destructive("show tables"));
        assert!(!is_destructive(""));
    }

    #[test]
    fn first_word_skips_comments() {
        assert_eq!(first_word("  select 1"), Some("select".into()));
        assert_eq!(first_word("-- x\n drop t"), Some("drop".into()));
        assert_eq!(first_word("/* x */ drop t"), Some("drop".into()));
        assert_eq!(first_word("delete(1)"), Some("delete".into()));
        assert_eq!(first_word("   "), None);
    }
}
