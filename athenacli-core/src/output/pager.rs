//! Output routing, the port of Python `AthenaCli.output()`: print to stdout
//! when the result fits the terminal, otherwise pipe it through `$PAGER`
//! (external process, like `click.echo_via_pager`). Every content line is
//! also written to the tee/once files; the status line is not.

use std::io::Write;

use crate::special::Session;

/// Python `get_reserved_space`: rows kept free for the completion menu.
fn reserved_space(height: u16) -> usize {
    ((height as f64 * 0.45).round() as usize).min(8)
}

/// Python `get_output_margin`.
fn output_margin(prompt: &str, timing: bool, status: Option<&str>, height: u16) -> usize {
    let mut margin = reserved_space(height) + prompt.matches('\n').count() + 1;
    if timing {
        margin += 1;
    }
    if let Some(s) = status {
        margin += 1 + s.matches('\n').count();
    }
    margin
}

/// Route `content` (a rendered table, possibly with a title line) to stdout
/// or the pager, write tee/once copies, then print `status` to stdout.
pub fn output(session: &mut Session, prompt: &str, content: &str, status: Option<&str>) {
    if !content.is_empty() {
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let margin = output_margin(prompt, session.timing, status, rows);

        let mut fits = true;
        let mut via_pager = false;
        let mut buf: Vec<&str> = Vec::new();

        for (i, line) in content.split('\n').enumerate() {
            session.write_tee(line);
            session.write_once(line);

            if fits || via_pager {
                buf.push(line);
                if line.chars().count() > cols as usize
                    || i + 1 > (rows as usize).saturating_sub(margin)
                {
                    fits = false;
                    if session.pager_enabled {
                        via_pager = true;
                    }
                    if !via_pager {
                        for l in buf.drain(..) {
                            println!("{l}");
                        }
                    }
                }
            } else {
                println!("{line}");
            }
        }

        if !buf.is_empty() {
            if via_pager {
                page(&buf.join("\n"));
            } else {
                for l in buf {
                    println!("{l}");
                }
            }
        }
    }

    if let Some(s) = status {
        if !s.is_empty() {
            println!("{s}");
        }
    }
}

/// Pipe `text` through `$PAGER` (default `less -R`), blocking until it exits.
fn page(text: &str) {
    let pager = std::env::var("PAGER")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "less -R".to_string());

    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(&pager)
        .stdin(std::process::Stdio::piped())
        .spawn();

    match child {
        Ok(mut child) => {
            if let Some(stdin) = child.stdin.as_mut() {
                // Ignore BrokenPipe: the user may quit the pager early.
                let _ = stdin.write_all(text.as_bytes());
                let _ = stdin.write_all(b"\n");
            }
            let _ = child.wait();
        }
        Err(_) => {
            // Pager unavailable: degrade to plain stdout.
            println!("{text}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_space_caps_at_8() {
        assert_eq!(reserved_space(24), 8);
        assert_eq!(reserved_space(10), 5); // round(10*0.45) = 5
        assert_eq!(reserved_space(100), 8);
    }

    #[test]
    fn margin_counts_prompt_timing_and_status() {
        // base: reserved(24)=8 + 0 prompt newlines + 1 = 9
        assert_eq!(output_margin("> ", false, None, 24), 9);
        assert_eq!(output_margin("> ", true, None, 24), 10);
        assert_eq!(output_margin("> ", false, Some("a\nb"), 24), 11);
        assert_eq!(output_margin("a\n> ", false, None, 24), 10);
    }
}
