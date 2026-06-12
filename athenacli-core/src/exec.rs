//! Synchronous execution wrapper. Holds the Athena client plus a tokio
//! `Handle`, and drives async query execution from the synchronous REPL via
//! `Handle::block_on` (master plan: async SDK <-> sync reedline bridge).

use aws_config::SdkConfig;
use aws_sdk_athena::Client;
use tokio::runtime::Handle;

use crate::athena::{self, QueryRun};

/// One executed statement's output.
#[derive(Debug, Clone)]
pub struct ResultSet {
    pub run: QueryRun,
    /// `\G` was appended -> render vertically.
    pub expanded: bool,
}

impl ResultSet {
    pub fn status(&self) -> String {
        self.run.status()
    }
}

pub struct SqlExecute {
    client: Client,
    sdk_config: SdkConfig,
    handle: Handle,
    pub database: String,
    pub catalog: String,
    pub region: Option<String>,
    s3_staging_dir: Option<String>,
    work_group: Option<String>,
}

impl SqlExecute {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: Client,
        sdk_config: SdkConfig,
        handle: Handle,
        database_arg: &str,
        s3_staging_dir: Option<String>,
        work_group: Option<String>,
        region: Option<String>,
    ) -> Self {
        let (catalog, database) = split_catalog_database(database_arg);
        Self {
            client,
            sdk_config,
            handle,
            database,
            catalog,
            region,
            s3_staging_dir,
            work_group,
        }
    }

    /// Switch the session to `catalog.database` (the `use` special command).
    pub fn set_database(&mut self, database_arg: &str) {
        let (catalog, database) = split_catalog_database(database_arg);
        self.catalog = catalog;
        self.database = database;
    }

    /// The loaded AWS config, for building sibling service clients (S3).
    pub fn sdk_config(&self) -> &SdkConfig {
        &self.sdk_config
    }

    /// Effective Athena workgroup; `primary` when none is configured, matching
    /// Athena's own default.
    pub fn work_group(&self) -> &str {
        self.work_group
            .as_deref()
            .unwrap_or(athena::DEFAULT_WORK_GROUP)
    }

    /// Tokio handle, for spawning the background completion refresher.
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// A cloneable querier for completion metadata, sharing this connection.
    pub fn querier(&self) -> crate::completion::metadata::Querier {
        crate::completion::metadata::Querier::new(
            self.client.clone(),
            self.database.clone(),
            self.catalog.clone(),
            self.s3_staging_dir.clone(),
            self.work_group.clone(),
        )
    }

    /// AWS console URL for a query execution. `None` when the region is unknown.
    pub fn console_url(&self, query_execution_id: &str) -> Option<String> {
        self.region
            .as_deref()
            .map(|region| athena::console_url(region, query_execution_id))
    }

    /// Run one SQL statement (already stripped of `;`/`\G`) through the
    /// async bridge.
    pub fn run_sql(&self, sql: &str) -> anyhow::Result<QueryRun> {
        self.handle.block_on(athena::run_query(
            &self.client,
            sql,
            &self.database,
            &self.catalog,
            self.s3_staging_dir.as_deref(),
            self.work_group.as_deref(),
            true, // user-facing: show the live progress spinner
        ))
    }

    /// Split, strip trailing `;`, detect `\G`, and execute each statement.
    pub fn run(&self, statement: &str) -> anyhow::Result<Vec<ResultSet>> {
        let statement = statement.trim();
        if statement.is_empty() {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        for raw in split_statements(statement) {
            // Python: sql.rstrip(';'), then strip a trailing `\G`.
            let trimmed = raw.trim_end_matches(';').trim();
            let (sql, expanded) = match trimmed.strip_suffix("\\G") {
                Some(rest) => (rest.trim(), true),
                None => (trimmed, false),
            };
            if sql.is_empty() {
                continue;
            }
            let run = self.run_sql(sql)?;
            out.push(ResultSet { run, expanded });
        }
        Ok(out)
    }
}

/// Split `catalog.database`, mirroring Python `SQLExecute.__init__`.
/// Defaults the catalog to `AwsDataCatalog`.
pub fn split_catalog_database(arg: &str) -> (String, String) {
    if let Some((catalog, database)) = arg.split_once('.') {
        let catalog = if catalog.is_empty() {
            "AwsDataCatalog"
        } else {
            catalog
        };
        (catalog.to_string(), database.to_string())
    } else {
        ("AwsDataCatalog".to_string(), arg.to_string())
    }
}

/// Split a string into statements on top-level `;`, ignoring semicolons inside
/// string/identifier literals and comments. Replaces Python `sqlparse.split`.
pub fn split_statements(sql: &str) -> Vec<String> {
    #[derive(PartialEq)]
    enum State {
        Normal,
        Single,
        Double,
        Tick,
        LineComment,
        BlockComment,
    }

    let mut state = State::Normal;
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut chars = sql.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        match state {
            State::Normal => match c {
                '\'' => state = State::Single,
                '"' => state = State::Double,
                '`' => state = State::Tick,
                '-' if matches!(chars.peek(), Some((_, '-'))) => {
                    chars.next();
                    state = State::LineComment;
                }
                '/' if matches!(chars.peek(), Some((_, '*'))) => {
                    chars.next();
                    state = State::BlockComment;
                }
                ';' => {
                    let stmt = sql[start..i].trim();
                    if !stmt.is_empty() {
                        out.push(stmt.to_string());
                    }
                    start = i + 1; // ';' is one byte
                }
                _ => {}
            },
            State::Single => {
                if c == '\'' {
                    if matches!(chars.peek(), Some((_, '\''))) {
                        chars.next(); // escaped ''
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::Double => {
                if c == '"' {
                    if matches!(chars.peek(), Some((_, '"'))) {
                        chars.next();
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::Tick => {
                if c == '`' {
                    state = State::Normal;
                }
            }
            State::LineComment => {
                if c == '\n' {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                if c == '*' && matches!(chars.peek(), Some((_, '/'))) {
                    chars.next();
                    state = State::Normal;
                }
            }
        }
    }

    let tail = sql[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_database_split() {
        assert_eq!(
            split_catalog_database("mycat.mydb"),
            ("mycat".into(), "mydb".into())
        );
        assert_eq!(
            split_catalog_database("mydb"),
            ("AwsDataCatalog".into(), "mydb".into())
        );
        assert_eq!(
            split_catalog_database("default"),
            ("AwsDataCatalog".into(), "default".into())
        );
        // split on first '.' only
        assert_eq!(
            split_catalog_database("cat.sch.ema"),
            ("cat".into(), "sch.ema".into())
        );
        // empty catalog part defaults
        assert_eq!(
            split_catalog_database(".db"),
            ("AwsDataCatalog".into(), "db".into())
        );
    }

    #[test]
    fn splits_simple_statements() {
        assert_eq!(split_statements("SELECT 1;"), vec!["SELECT 1"]);
        assert_eq!(
            split_statements("SELECT 1; SELECT 2"),
            vec!["SELECT 1", "SELECT 2"]
        );
        assert_eq!(split_statements("SELECT 1"), vec!["SELECT 1"]);
    }

    #[test]
    fn ignores_semicolons_in_strings() {
        assert_eq!(split_statements("SELECT ';'"), vec!["SELECT ';'"]);
        assert_eq!(
            split_statements("SELECT 'a;b'; SELECT 2"),
            vec!["SELECT 'a;b'", "SELECT 2"]
        );
        // escaped single quote inside a string
        assert_eq!(
            split_statements("SELECT 'it''s; ok'"),
            vec!["SELECT 'it''s; ok'"]
        );
    }

    #[test]
    fn ignores_semicolons_in_comments() {
        assert_eq!(
            split_statements("SELECT 1 -- a;b\n; SELECT 2"),
            vec!["SELECT 1 -- a;b", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT /* ;;; */ 1"),
            vec!["SELECT /* ;;; */ 1"]
        );
    }

    #[test]
    fn drops_empty_trailing_statement() {
        assert_eq!(split_statements("SELECT 1;   ;  "), vec!["SELECT 1"]);
        assert_eq!(split_statements("   "), Vec::<String>::new());
    }
}
