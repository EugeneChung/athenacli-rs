//! IO special commands, mirroring Python `special/iocommands.py`:
//! external editor (`\e`), pager, timing, tee/once, system, watch, read.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::cancel;
use crate::exec::split_statements;

use super::{Emit, Flow, Invocation, Sink, SpecialCtx, SpecialResult};

// ---------------------------------------------------------------- editor \e

const EDITOR_MARKER: &str = "# Type your query above this line.\n";

/// Is this an external editor command (`\e` prefix or suffix)?
pub fn editor_command(command: &str) -> bool {
    let t = command.trim();
    t.starts_with("\\e") || t.ends_with("\\e")
}

/// `\e filename` -> the filename.
pub fn get_filename(sql: &str) -> Option<String> {
    let t = sql.trim();
    if t.starts_with("\\e") {
        let (_, _, filename) = match t.split_once(' ') {
            Some((c, f)) => (c, ' ', f.trim()),
            None => return None,
        };
        return (!filename.is_empty()).then(|| filename.to_string());
    }
    None
}

/// Strip leading/trailing `\e` markers to recover the query part.
pub fn get_editor_query(sql: &str) -> String {
    let mut s = sql.trim().to_string();
    loop {
        if let Some(rest) = s.strip_prefix("\\e") {
            s = rest.to_string();
        } else if let Some(rest) = s.strip_suffix("\\e") {
            s = rest.to_string();
        } else {
            break;
        }
    }
    s
}

/// Open `$VISUAL`/`$EDITOR` (default `vi`) on `sql` (or on `filename`), wait,
/// and return the edited query. Mirrors `click.edit` + Python
/// `open_external_editor`.
pub fn open_external_editor(
    filename: Option<&str>,
    sql: &str,
) -> anyhow::Result<(String, Option<String>)> {
    let filename = filename.and_then(|f| f.trim().split(' ').next().map(str::to_string));
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    let edited = if let Some(name) = &filename {
        run_editor(&editor, name)?;
        match std::fs::read_to_string(name) {
            Ok(content) => content,
            Err(_) => {
                return Ok((
                    sql.to_string(),
                    Some(format!("Error reading file: {name}.")),
                ))
            }
        }
    } else {
        let tmp = tempfile::Builder::new()
            .prefix("athenacli_")
            .suffix(".sql")
            .tempfile()?;
        std::fs::write(tmp.path(), format!("{sql}\n\n{EDITOR_MARKER}"))?;
        run_editor(&editor, &tmp.path().to_string_lossy())?;
        let mut content = String::new();
        std::fs::File::open(tmp.path())?.read_to_string(&mut content)?;
        content
    };

    let query = edited
        .split(EDITOR_MARKER)
        .next()
        .unwrap_or("")
        .trim_end_matches('\n')
        .to_string();
    Ok((query, None))
}

fn run_editor(editor: &str, path: &str) -> anyhow::Result<()> {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} '{path}'"))
        .status()?;
    if !status.success() {
        anyhow::bail!("{editor}: Editing failed!");
    }
    Ok(())
}

// ------------------------------------------------------------------- pager

/// `pager [command]` / `\P [command]`.
pub fn set_pager(ctx: &mut SpecialCtx, inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    let msg = if !inv.arg.is_empty() {
        std::env::set_var("PAGER", &inv.arg);
        format!("PAGER set to {}.", inv.arg)
    } else if let Ok(pager) = std::env::var("PAGER") {
        format!("PAGER set to {pager}.")
    } else {
        "Pager enabled.".to_string()
    };
    ctx.session.pager_enabled = true;
    sink(ctx.session, Emit::Special(SpecialResult::message(msg)))?;
    Ok(Flow::Continue)
}

/// `nopager` / `\n`.
pub fn disable_pager(
    ctx: &mut SpecialCtx,
    _inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    ctx.session.pager_enabled = false;
    sink(
        ctx.session,
        Emit::Special(SpecialResult::message("Pager disabled.")),
    )?;
    Ok(Flow::Continue)
}

// ------------------------------------------------------------------ timing

/// `\timing` / `\t`.
pub fn toggle_timing(
    ctx: &mut SpecialCtx,
    _inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    ctx.session.timing = !ctx.session.timing;
    let msg = if ctx.session.timing {
        "Timing is on."
    } else {
        "Timing is off."
    };
    sink(ctx.session, Emit::Special(SpecialResult::message(msg)))?;
    Ok(Flow::Continue)
}

// ---------------------------------------------------------------- tee/once

/// Python `parseargfile`: `-o file` overwrites, bare `file` appends.
pub fn parseargfile(arg: &str) -> anyhow::Result<(PathBuf, bool)> {
    let (overwrite, filename) = match arg.strip_prefix("-o ") {
        Some(rest) => (true, rest),
        None => (false, arg),
    };
    if filename.is_empty() {
        anyhow::bail!("You must provide a filename.");
    }
    Ok((PathBuf::from(crate::config::expand(filename)), overwrite))
}

/// `tee [-o] filename`.
pub fn set_tee(ctx: &mut SpecialCtx, inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    let (path, overwrite) = parseargfile(&inv.arg)?;
    ctx.session.set_tee(&path, overwrite)?;
    sink(ctx.session, Emit::Special(SpecialResult::message("")))?;
    Ok(Flow::Continue)
}

/// `notee`.
pub fn no_tee(ctx: &mut SpecialCtx, _inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    ctx.session.close_tee();
    sink(ctx.session, Emit::Special(SpecialResult::message("")))?;
    Ok(Flow::Continue)
}

/// `\once [-o] filename` / `\o`.
pub fn set_once(ctx: &mut SpecialCtx, inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    let (path, overwrite) = parseargfile(&inv.arg)?;
    ctx.session.set_once(path, overwrite);
    sink(ctx.session, Emit::Special(SpecialResult::message("")))?;
    Ok(Flow::Continue)
}

// ------------------------------------------------------------------ system

/// `system [command]`, with `cd` handled in-process (Python
/// `handle_cd_command`).
pub fn system_command(
    ctx: &mut SpecialCtx,
    inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    if inv.arg.is_empty() {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message("Syntax: system [command].\n")),
        )?;
        return Ok(Flow::Continue);
    }

    let command = inv.arg.trim();
    if command.starts_with("cd") {
        let msg = match handle_cd_command(command) {
            Ok(cwd) => cwd,
            Err(e) => e,
        };
        sink(ctx.session, Emit::Special(SpecialResult::message(msg)))?;
        return Ok(Flow::Continue);
    }

    // Python: arg.split(' ') + Popen without a shell.
    let parts: Vec<&str> = command.split(' ').filter(|s| !s.is_empty()).collect();
    let msg = match std::process::Command::new(parts[0])
        .args(&parts[1..])
        .output()
    {
        Ok(out) => {
            let response = if out.stderr.is_empty() {
                out.stdout
            } else {
                out.stderr
            };
            String::from_utf8_lossy(&response).trim_end().to_string()
        }
        Err(e) => format!("OSError: {e}"),
    };
    sink(ctx.session, Emit::Special(SpecialResult::message(msg)))?;
    Ok(Flow::Continue)
}

/// `cd <dir>` inside `system`: chdir the process, return the new cwd.
pub fn handle_cd_command(arg: &str) -> Result<String, String> {
    let directory = arg
        .split_once("cd ")
        .map(|(_, d)| d.trim())
        .filter(|d| !d.is_empty())
        .ok_or_else(|| "No folder name was provided.".to_string())?;
    let expanded = crate::config::expand(directory);
    std::env::set_current_dir(&expanded).map_err(|e| e.to_string())?;
    Ok(std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default())
}

// -------------------------------------------------------------------- read

/// `read filename`: execute statements from a file (with destructive
/// confirmation per statement).
pub fn read_file(ctx: &mut SpecialCtx, inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    let filename = &inv.arg;
    if filename.is_empty() {
        return Ok(Flow::Continue);
    }
    let query = match std::fs::read_to_string(crate::config::expand(filename)) {
        Ok(q) => q,
        Err(_) => {
            sink(
                ctx.session,
                Emit::Special(SpecialResult::message(format!(
                    "Error reading file: {filename}."
                ))),
            )?;
            return Ok(Flow::Continue);
        }
    };
    for sql in split_statements(&query) {
        let sql = sql.trim_end_matches(';').trim().to_string();
        if sql.is_empty() {
            continue;
        }
        match (ctx.confirm)(&sql) {
            Some(false) => {
                sink(
                    ctx.session,
                    Emit::Special(SpecialResult::message("Wise choice!")),
                )?;
                return Ok(Flow::Continue);
            }
            Some(true) => sink(
                ctx.session,
                Emit::Special(SpecialResult::message("Your call!")),
            )?,
            None => {}
        }
        let run = ctx.exec.run_sql(&sql)?;
        sink(
            ctx.session,
            Emit::Special(SpecialResult {
                title: Some(sql),
                headers: run.headers,
                rows: run.rows,
                status: None,
            }),
        )?;
    }
    Ok(Flow::Continue)
}

// ------------------------------------------------------------------- watch

const WATCH_USAGE: &str = "Syntax: watch [seconds] [-c] query.
    * seconds: The interval at the query will be repeated, in seconds.
               By default 5.
    * -c: Clears the screen between every iteration.
";

/// Parse `watch [seconds] [-c] query` -> (seconds, clear_screen, statement).
pub fn parse_watch_args(arg: &str) -> Option<(f64, bool, String)> {
    let mut seconds = 5.0;
    let mut clear_screen = false;
    let mut rest = arg.trim();
    loop {
        if rest.is_empty() {
            return None;
        }
        let (current, tail) = match rest.split_once(' ') {
            Some((c, t)) => (c, t.trim_start()),
            None => (rest, ""),
        };
        if let Ok(s) = current.parse::<f64>() {
            seconds = s;
            rest = tail;
            continue;
        }
        if current == "-c" {
            clear_screen = true;
            rest = tail;
            continue;
        }
        let statement = if tail.is_empty() {
            current.to_string()
        } else {
            format!("{current} {tail}")
        };
        return Some((seconds, clear_screen, statement));
    }
}

/// `watch [seconds] [-c] query`: repeat until Ctrl-C. The pager is disabled
/// for the duration (Python does the same to avoid blocking each round).
pub fn watch_query(
    ctx: &mut SpecialCtx,
    inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    if inv.arg.is_empty() {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message(WATCH_USAGE)),
        )?;
        return Ok(Flow::Continue);
    }
    let Some((seconds, clear_screen, statement)) = parse_watch_args(&inv.arg) else {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message(WATCH_USAGE)),
        )?;
        return Ok(Flow::Continue);
    };

    match (ctx.confirm)(&statement) {
        Some(false) => {
            sink(
                ctx.session,
                Emit::Special(SpecialResult::message("Wise choice!")),
            )?;
            return Ok(Flow::Continue);
        }
        Some(true) => sink(
            ctx.session,
            Emit::Special(SpecialResult::message("Your call!")),
        )?,
        None => {}
    }

    let sql_list: Vec<(String, String)> = split_statements(&statement)
        .into_iter()
        .map(|sql| {
            let stripped = sql.trim_end_matches(';').trim().to_string();
            let title = format!("> {stripped}");
            (stripped, title)
        })
        .collect();

    let old_pager = ctx.session.pager_enabled;
    ctx.session.pager_enabled = false;
    cancel::reset();

    let result = (|| -> anyhow::Result<()> {
        loop {
            if clear_screen {
                sink(ctx.session, Emit::ClearScreen)?;
            }
            for (sql, title) in &sql_list {
                if cancel::requested() {
                    return Ok(());
                }
                let run = match ctx.exec.run_sql(sql) {
                    Ok(run) => run,
                    // Ctrl-C mid-query: stop watching quietly.
                    Err(e) if e.is::<cancel::Cancelled>() => return Ok(()),
                    Err(e) => return Err(e),
                };
                sink(
                    ctx.session,
                    Emit::Special(SpecialResult {
                        title: Some(title.clone()),
                        headers: run.headers,
                        rows: run.rows,
                        status: None,
                    }),
                )?;
            }
            // Sleep in slices so Ctrl-C interrupts promptly.
            let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
            while Instant::now() < deadline {
                if cancel::requested() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    })();

    ctx.session.pager_enabled = old_pager;
    result?;
    println!();
    Ok(Flow::Continue)
}

// flush stdout helper used by the REPL's ClearScreen handling
pub fn clear_screen() {
    print!("\x1b[2J\x1b[1;1H");
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_command_detection() {
        assert!(editor_command("\\e"));
        assert!(editor_command("\\e filename.sql"));
        assert!(editor_command("select * from t \\e"));
        assert!(editor_command("  \\e  "));
        assert!(!editor_command("select 1"));
    }

    #[test]
    fn editor_filename_extraction() {
        assert_eq!(get_filename("\\e /tmp/f.sql"), Some("/tmp/f.sql".into()));
        assert_eq!(get_filename("\\e"), None);
        assert_eq!(get_filename("select 1 \\e"), None);
    }

    #[test]
    fn editor_query_stripping() {
        assert_eq!(
            get_editor_query("select * from style\\e"),
            "select * from style"
        );
        assert_eq!(get_editor_query("\\eselect 1"), "select 1");
        assert_eq!(get_editor_query("\\e"), "");
        // does not eat a plain trailing 'e' (the Python regex comment case)
        assert_eq!(
            get_editor_query("select * from style"),
            "select * from style"
        );
    }

    #[test]
    fn parseargfile_modes() {
        let (path, overwrite) = parseargfile("-o /tmp/out.txt").unwrap();
        assert_eq!(path, PathBuf::from("/tmp/out.txt"));
        assert!(overwrite);

        let (path, overwrite) = parseargfile("/tmp/out.txt").unwrap();
        assert_eq!(path, PathBuf::from("/tmp/out.txt"));
        assert!(!overwrite);

        assert!(parseargfile("").is_err());
        assert!(parseargfile("-o ").is_err());
    }

    #[test]
    fn watch_arg_parsing() {
        assert_eq!(
            parse_watch_args("select 1"),
            Some((5.0, false, "select 1".into()))
        );
        assert_eq!(
            parse_watch_args("10 select 1"),
            Some((10.0, false, "select 1".into()))
        );
        assert_eq!(
            parse_watch_args("-c select 1"),
            Some((5.0, true, "select 1".into()))
        );
        assert_eq!(
            parse_watch_args("2.5 -c select 1 from t"),
            Some((2.5, true, "select 1 from t".into()))
        );
        assert_eq!(parse_watch_args("-c"), None);
        assert_eq!(parse_watch_args("3"), None);
    }

    #[test]
    fn cd_requires_directory() {
        assert!(handle_cd_command("cd").is_err());
        assert!(handle_cd_command("cd /nonexistent-dir-xyz").is_err());
    }
}
