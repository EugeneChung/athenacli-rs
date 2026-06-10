//! `\dt` / `\l`, mirroring Python `special/dbcommands.py`.

use super::{Emit, Flow, Invocation, Sink, SpecialCtx, SpecialResult};

/// `\dt [table]`: `SHOW TABLES`, or `SHOW COLUMNS FROM <table>` with an arg.
pub fn list_tables(
    ctx: &mut SpecialCtx,
    inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    let query = if inv.arg.is_empty() {
        "SHOW TABLES".to_string()
    } else {
        format!("SHOW COLUMNS FROM {}", inv.arg)
    };
    let run = ctx.exec.run_sql(&query)?;
    sink(
        ctx.session,
        Emit::Special(SpecialResult::table(run.headers, run.rows)),
    )?;
    Ok(Flow::Continue)
}

/// `\l`: `SHOW DATABASES`.
pub fn list_databases(
    ctx: &mut SpecialCtx,
    _inv: &Invocation,
    sink: &mut Sink,
) -> anyhow::Result<Flow> {
    let run = ctx.exec.run_sql("SHOW DATABASES")?;
    sink(
        ctx.session,
        Emit::Special(SpecialResult::table(run.headers, run.rows)),
    )?;
    Ok(Flow::Continue)
}
