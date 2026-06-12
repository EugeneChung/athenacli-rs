//! Athena query lifecycle: StartQueryExecution -> poll GetQueryExecution ->
//! paginate GetQueryResults. Mirrors what `pyathena`'s DB-API cursor hides,
//! including the "first data row is the header" gotcha (master plan risk #2).

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use aws_sdk_athena::types::{
    QueryExecutionContext, QueryExecutionState, ResultConfiguration, StatementType,
};
use aws_sdk_athena::Client;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};

const POLL_INTERVAL: Duration = Duration::from_millis(200); // pyathena poll_interval=0.2

/// Athena's implicit workgroup when none is configured.
pub const DEFAULT_WORK_GROUP: &str = "primary";

/// AWS console deep link to a query execution's history entry in the query editor.
pub fn console_url(region: &str, query_execution_id: &str) -> String {
    format!(
        "https://{region}.console.aws.amazon.com/athena/home\
         ?region={region}#/query-editor/history/{query_execution_id}"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Select,
    Show,
    Describe,
    Explain,
    Use,
    Ddl,
    Other,
}

impl StatementKind {
    pub fn from_sql(sql: &str) -> Self {
        let first = sql
            .trim_start()
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match first.as_str() {
            "select" | "with" | "values" => StatementKind::Select,
            "show" => StatementKind::Show,
            "describe" | "desc" => StatementKind::Describe,
            "explain" => StatementKind::Explain,
            "use" => StatementKind::Use,
            "create" | "drop" | "alter" | "msck" | "insert" | "update" | "delete" => {
                StatementKind::Ddl
            }
            _ => StatementKind::Other,
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueryRun {
    pub query_execution_id: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub output_location: Option<String>,
    pub scanned_bytes: i64,
    pub engine_ms: i64,
    pub elapsed_ms: u128,
    pub kind: StatementKind,
    pub has_result_set: bool,
}

impl QueryRun {
    /// Status line, byte-for-byte matching Python `format_utils.format_status`.
    pub fn status(&self) -> String {
        format_status(self.rows.len(), self.engine_ms, self.scanned_bytes)
    }
}

/// True iff the first data row duplicates the column names (the Athena header
/// row that `pyathena` strips). Conservative: only skips when every cell of the
/// first row equals the corresponding metadata column name.
pub fn should_skip_header(first_row: &[Option<String>], column_names: &[String]) -> bool {
    if column_names.is_empty() || first_row.len() != column_names.len() {
        return false;
    }
    first_row
        .iter()
        .zip(column_names)
        .all(|(cell, name)| cell.as_deref() == Some(name.as_str()))
}

/// Human-readable byte size, matching Python `format_utils.humanize_size`.
pub fn humanize_size(mut num: f64) -> String {
    const SUFFIXES: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut idx = 0;
    while num >= 1024.0 && idx < SUFFIXES.len() - 1 {
        num /= 1024.0;
        idx += 1;
    }
    let formatted = format!("{num:.2}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    format!("{} {}", trimmed, SUFFIXES[idx])
}

/// Status line, matching Python `format_utils.format_status`
/// (`rows_status` + `statistics`).
pub fn format_status(rows_length: usize, engine_ms: i64, scanned_bytes: i64) -> String {
    let rows = if rows_length > 0 {
        format!(
            "{} row{} in set",
            rows_length,
            if rows_length == 1 { "" } else { "s" }
        )
    } else {
        "Query OK".to_string()
    };
    // Most regions are $5 per TB.
    let approx_cost = scanned_bytes as f64 / 1024f64.powi(4) * 5.0;
    format!(
        "{}\nExecution time: {} ms, Data scanned: {}, Approximate cost: ${:.2}",
        rows,
        engine_ms,
        humanize_size(scanned_bytes as f64),
        approx_cost
    )
}

/// Spinner shown while a query runs: Athena state label plus live elapsed
/// time. `ProgressBar` draws to stderr and hides itself when stderr is not a
/// terminal, so piped/scripted runs stay clean.
fn progress_spinner() -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}... {secs}")
            .expect("static template is valid")
            .with_key(
                "secs",
                |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                    let _ = write!(w, "{:.1}s", state.elapsed().as_secs_f64());
                },
            ),
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// Run a single SQL statement end to end.
///
/// `show_progress` renders a live `Queued/Running... 1.2s` spinner on stderr
/// while the query executes; the background metadata refresher passes `false`.
pub async fn run_query(
    client: &Client,
    sql: &str,
    database: &str,
    catalog: &str,
    s3_staging_dir: Option<&str>,
    work_group: Option<&str>,
    show_progress: bool,
) -> anyhow::Result<QueryRun> {
    let started = Instant::now();
    let kind = StatementKind::from_sql(sql);
    let spinner = show_progress.then(progress_spinner);
    if let Some(pb) = &spinner {
        pb.set_message("Submitting");
    }

    // Whole lifecycle inside one block so the spinner is cleared on every
    // exit path (success, Athena failure, Ctrl-C) before anything prints.
    let run = async {
        let start = client
            .start_query_execution()
            .query_string(sql)
            .query_execution_context(
                QueryExecutionContext::builder()
                    .database(database)
                    .catalog(catalog)
                    .build(),
            )
            .set_result_configuration(
                s3_staging_dir
                    .map(|loc| ResultConfiguration::builder().output_location(loc).build()),
            )
            .set_work_group(work_group.map(str::to_string))
            .send()
            .await
            .context("StartQueryExecution failed")?;

        let query_id = start
            .query_execution_id()
            .ok_or_else(|| anyhow!("StartQueryExecution returned no QueryExecutionId"))?
            .to_string();

        // Poll until terminal state.
        let (scanned_bytes, engine_ms, output_location, statement_type) = loop {
            let resp = client
                .get_query_execution()
                .query_execution_id(&query_id)
                .send()
                .await
                .context("GetQueryExecution failed")?;
            let qe = resp
                .query_execution()
                .ok_or_else(|| anyhow!("GetQueryExecution returned no QueryExecution"))?;
            let state = qe.status().and_then(|s| s.state()).cloned();

            match state {
                Some(QueryExecutionState::Succeeded) => {
                    let stats = qe.statistics();
                    let scanned = stats.and_then(|s| s.data_scanned_in_bytes()).unwrap_or(0);
                    let engine = stats
                        .and_then(|s| s.engine_execution_time_in_millis())
                        .unwrap_or(0);
                    let loc = qe
                        .result_configuration()
                        .and_then(|r| r.output_location())
                        .map(str::to_string);
                    let stype = qe.statement_type().cloned();
                    break (scanned, engine, loc, stype);
                }
                Some(QueryExecutionState::Failed) | Some(QueryExecutionState::Cancelled) => {
                    let reason = qe
                        .status()
                        .and_then(|s| s.state_change_reason())
                        .unwrap_or("query failed without a reason");
                    return Err(anyhow!("{reason}"));
                }
                other => {
                    // Ctrl-C in the REPL: stop the query server-side and bail.
                    if crate::cancel::requested() {
                        let _ = client
                            .stop_query_execution()
                            .query_execution_id(&query_id)
                            .send()
                            .await;
                        return Err(crate::cancel::Cancelled.into());
                    }
                    if let Some(pb) = &spinner {
                        pb.set_message(match other {
                            Some(QueryExecutionState::Queued) => "Queued",
                            Some(QueryExecutionState::Running) => "Running",
                            _ => "Waiting",
                        });
                    }
                    tokio::time::sleep(POLL_INTERVAL).await
                }
            }
        };

        if let Some(pb) = &spinner {
            pb.set_message("Fetching results");
        }

        // DDL statements (CREATE/DROP/ALTER) produce no result set.
        let fetch_rows = !matches!(statement_type, Some(StatementType::Ddl));

        let mut headers: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();

        if fetch_rows {
            let mut next_token: Option<String> = None;
            let mut first_page = true;
            loop {
                let mut req = client.get_query_results().query_execution_id(&query_id);
                if let Some(token) = &next_token {
                    req = req.next_token(token);
                }
                let resp = req.send().await.context("GetQueryResults failed")?;
                let result_set = resp.result_set();

                if first_page {
                    if let Some(meta) = result_set.and_then(|rs| rs.result_set_metadata()) {
                        headers = meta
                            .column_info()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                    }
                }

                let page_rows: Vec<Vec<Option<String>>> = result_set
                    .map(|rs| {
                        rs.rows()
                            .iter()
                            .map(|row| {
                                row.data()
                                    .iter()
                                    .map(|d| d.var_char_value().map(str::to_string))
                                    .collect()
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let mut iter = page_rows.into_iter();
                if first_page {
                    if let Some(first) = iter.next() {
                        if !should_skip_header(&first, &headers) {
                            rows.push(first);
                        }
                    }
                }
                rows.extend(iter);

                first_page = false;
                next_token = resp.next_token().map(str::to_string);
                if next_token.is_none() {
                    break;
                }
            }
        }

        let has_result_set = !headers.is_empty();

        Ok(QueryRun {
            query_execution_id: query_id,
            headers,
            rows,
            output_location,
            scanned_bytes,
            engine_ms,
            elapsed_ms: started.elapsed().as_millis(),
            kind,
            has_result_set,
        })
    }
    .await;

    if let Some(pb) = &spinner {
        pb.finish_and_clear();
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(vals: &[&str]) -> Vec<Option<String>> {
        vals.iter().map(|v| Some(v.to_string())).collect()
    }

    #[test]
    fn skips_header_when_first_row_equals_column_names() {
        let names = vec!["id".to_string(), "name".to_string()];
        let first = cells(&["id", "name"]);
        assert!(should_skip_header(&first, &names));
    }

    #[test]
    fn keeps_first_row_when_it_differs() {
        let names = vec!["id".to_string(), "name".to_string()];
        let first = cells(&["1", "alice"]);
        assert!(!should_skip_header(&first, &names));
    }

    #[test]
    fn does_not_skip_on_length_mismatch_or_empty() {
        assert!(!should_skip_header(
            &cells(&["id"]),
            &["id".into(), "x".into()]
        ));
        assert!(!should_skip_header(&cells(&["id"]), &[]));
    }

    #[test]
    fn humanize_matches_python() {
        assert_eq!(humanize_size(0.0), "0 B");
        assert_eq!(humanize_size(1023.0), "1023 B");
        assert_eq!(humanize_size(1024.0), "1 KB");
        assert_eq!(humanize_size(1536.0), "1.5 KB");
        assert_eq!(humanize_size(1024.0 * 1024.0), "1 MB");
        assert_eq!(humanize_size(1024f64.powi(4)), "1 TB");
    }

    #[test]
    fn status_singular_plural_and_query_ok() {
        assert!(format_status(1, 12, 0).starts_with("1 row in set\n"));
        assert!(format_status(2, 12, 0).starts_with("2 rows in set\n"));
        assert!(format_status(0, 12, 0).starts_with("Query OK\n"));
    }

    #[test]
    fn status_full_line_format() {
        // 1 TB scanned => $5.00, "1 TB".
        let s = format_status(3, 1500, 1024i64.pow(4));
        assert_eq!(
            s,
            "3 rows in set\nExecution time: 1500 ms, Data scanned: 1 TB, Approximate cost: $5.00"
        );
    }

    #[test]
    fn console_url_format() {
        assert_eq!(
            console_url("us-east-1", "abc-123"),
            "https://us-east-1.console.aws.amazon.com/athena/home?region=us-east-1#/query-editor/history/abc-123"
        );
    }

    #[test]
    fn statement_kind_detection() {
        assert_eq!(StatementKind::from_sql("SELECT 1"), StatementKind::Select);
        assert_eq!(
            StatementKind::from_sql("  show databases"),
            StatementKind::Show
        );
        assert_eq!(
            StatementKind::from_sql("CREATE TABLE t(x int)"),
            StatementKind::Ddl
        );
        assert_eq!(
            StatementKind::from_sql("WITH a AS (..) SELECT"),
            StatementKind::Select
        );
    }
}
