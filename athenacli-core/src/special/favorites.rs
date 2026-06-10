//! Favorite queries (`\f` / `\fs` / `\fd`), mirroring Python
//! `favoritequeries.py` + the `iocommands.py` handlers. Persisted in the
//! `[favorite_queries]` table of the athenaclirc TOML.

use crate::config;
use crate::exec::split_statements;

use super::{Emit, Flow, Invocation, Sink, SpecialCtx, SpecialResult};

pub const USAGE: &str = r#"
Favorite Queries are a way to save frequently used queries
with a short name.
Examples:

    # Save a new favorite query.
    > \fs simple select * from abc where a is not Null;

    # List all favorite queries.
    > \f
    +--------+---------------------------------------+
    | Name   | Query                                 |
    +--------+---------------------------------------+
    | simple | SELECT * FROM abc where a is not NULL |
    +--------+---------------------------------------+

    # Run a favorite query.
    > \f simple

    # Delete a favorite query.
    > \fd simple
    simple: Deleted
"#;

/// `\f [name [args..]]`: list favorites, or run one (with `$1..$N` substitution).
pub fn execute_favorite(
    ctx: &mut SpecialCtx,
    inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    if inv.arg.is_empty() {
        return list_favorites(ctx, sink);
    }

    let (name, arg_str) = match inv.arg.split_once(' ') {
        Some((n, rest)) => (n, rest),
        None => (inv.arg.as_str(), ""),
    };
    let args = shlex::split(arg_str).unwrap_or_default();

    let Some(query) = ctx.config.favorite_queries.get(name).cloned() else {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message(format!("No favorite query: {name}"))),
        )?;
        return Ok(Flow::Continue);
    };

    let query = match subst_favorite_query_args(&query, &args) {
        Ok(q) => q,
        Err(message) => {
            sink(ctx.session, Emit::Special(SpecialResult::message(message)))?;
            return Ok(Flow::Continue);
        }
    };

    for sql in split_statements(&query) {
        let sql = sql.trim_end_matches(';').trim().to_string();
        let title = format!("> {sql}");
        let run = ctx.exec.run_sql(&sql)?;
        sink(
            ctx.session,
            Emit::Special(SpecialResult {
                title: Some(title),
                headers: run.headers,
                rows: run.rows,
                status: None,
            }),
        )?;
    }
    Ok(Flow::Continue)
}

fn list_favorites(ctx: &mut SpecialCtx, sink: &mut Sink) -> anyhow::Result<Flow> {
    let mut names: Vec<_> = ctx.config.favorite_queries.keys().cloned().collect();
    names.sort();
    let rows: Vec<Vec<Option<String>>> = names
        .iter()
        .map(|n| vec![Some(n.clone()), ctx.config.favorite_queries.get(n).cloned()])
        .collect();
    let status = if rows.is_empty() {
        Some(format!("\nNo favorite queries found.{USAGE}"))
    } else {
        None
    };
    sink(
        ctx.session,
        Emit::Special(SpecialResult {
            title: None,
            headers: vec!["Name".into(), "Query".into()],
            rows,
            status,
        }),
    )?;
    Ok(Flow::Continue)
}

/// Replace positional parameters `$1..$N`; errors mirror Python's messages.
pub fn subst_favorite_query_args(query: &str, args: &[String]) -> Result<String, String> {
    let mut query = query.to_string();
    for (idx, val) in args.iter().enumerate() {
        let var = format!("${}", idx + 1);
        if !query.contains(&var) {
            return Err(format!(
                "query does not have substitution parameter {var}:\n  {query}"
            ));
        }
        query = query.replace(&var, val);
    }
    if let Some(pos) = query.find('$') {
        let tail: String = query[pos + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !tail.is_empty() {
            return Err(format!(
                "missing substitution for ${tail} in query:\n  {query}"
            ));
        }
    }
    Ok(query)
}

/// `\fs name query`: save and persist.
pub fn save_favorite(
    ctx: &mut SpecialCtx,
    inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    let usage = format!("Syntax: \\fs name query.\n{USAGE}");
    if inv.arg.is_empty() {
        sink(ctx.session, Emit::Special(SpecialResult::message(usage)))?;
        return Ok(Flow::Continue);
    }
    let (name, query) = match inv.arg.split_once(' ') {
        Some((n, q)) if !n.is_empty() && !q.trim().is_empty() => (n, q.trim()),
        _ => {
            sink(
                ctx.session,
                Emit::Special(SpecialResult::message(format!(
                    "{usage}Err: Both name and query are required."
                ))),
            )?;
            return Ok(Flow::Continue);
        }
    };
    ctx.config
        .favorite_queries
        .insert(name.to_string(), query.to_string());
    config::save(ctx.config, ctx.config_path)?;
    sink(ctx.session, Emit::Special(SpecialResult::message("Saved.")))?;
    Ok(Flow::Continue)
}

/// `\fd name`: delete and persist.
pub fn delete_favorite(
    ctx: &mut SpecialCtx,
    inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    if inv.arg.is_empty() {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message(format!(
                "Syntax: \\fd name.\n{USAGE}"
            ))),
        )?;
        return Ok(Flow::Continue);
    }
    let status = if ctx.config.favorite_queries.remove(&inv.arg).is_some() {
        config::save(ctx.config, ctx.config_path)?;
        format!("{}: Deleted", inv.arg)
    } else {
        format!("{}: Not Found.", inv.arg)
    };
    sink(ctx.session, Emit::Special(SpecialResult::message(status)))?;
    Ok(Flow::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subst_replaces_positional_args() {
        let q = subst_favorite_query_args(
            "select * from t where a=$1 and b=$2",
            &["1".into(), "x".into()],
        )
        .unwrap();
        assert_eq!(q, "select * from t where a=1 and b=x");
    }

    #[test]
    fn subst_complains_about_extra_args() {
        let err = subst_favorite_query_args("select 1", &["x".into()]).unwrap_err();
        assert!(err.contains("does not have substitution parameter $1"));
    }

    #[test]
    fn subst_complains_about_missing_args() {
        let err = subst_favorite_query_args("select $1, $2", &["x".into()]).unwrap_err();
        assert!(err.contains("missing substitution for $2"));
    }

    #[test]
    fn subst_no_args_passthrough() {
        assert_eq!(
            subst_favorite_query_args("select 1", &[]).unwrap(),
            "select 1"
        );
    }
}
