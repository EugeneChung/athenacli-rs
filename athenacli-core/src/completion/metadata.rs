//! Completion metadata cache and the queries that fill it. `Querier` clones the
//! Athena client plus connection context so the background refresher can build a
//! fresh `Metadata` off the REPL thread.

use std::collections::HashMap;

use aws_sdk_athena::Client;

use crate::athena;

/// Schema names available for completion. Swapped atomically by the refresher.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub databases: Vec<String>,
    /// Tables in the current database.
    pub tables: Vec<String>,
    /// Lower-cased table name -> column names.
    pub columns: HashMap<String, Vec<String>>,
}

/// Cheap-to-clone handle that runs the metadata queries against Athena.
#[derive(Clone)]
pub struct Querier {
    client: Client,
    database: String,
    catalog: String,
    s3_staging_dir: Option<String>,
    work_group: Option<String>,
}

impl Querier {
    pub fn new(
        client: Client,
        database: String,
        catalog: String,
        s3_staging_dir: Option<String>,
        work_group: Option<String>,
    ) -> Self {
        Self {
            client,
            database,
            catalog,
            s3_staging_dir,
            work_group,
        }
    }

    async fn run(&self, sql: &str) -> anyhow::Result<athena::QueryRun> {
        athena::run_query(
            &self.client,
            sql,
            &self.database,
            &self.catalog,
            self.s3_staging_dir.as_deref(),
            self.work_group.as_deref(),
            false, // background refresher: never draw on the user's terminal
        )
        .await
    }

    pub async fn databases(&self) -> anyhow::Result<Vec<String>> {
        Ok(first_column(&self.run("SHOW DATABASES").await?))
    }

    pub async fn tables(&self) -> anyhow::Result<Vec<String>> {
        Ok(first_column(&self.run("SHOW TABLES").await?))
    }

    pub async fn table_columns(&self) -> anyhow::Result<HashMap<String, Vec<String>>> {
        let sql = format!(
            "SELECT table_name, column_name FROM information_schema.columns \
             WHERE table_schema = '{}' ORDER BY table_name, ordinal_position",
            self.database.replace('\'', "''")
        );
        let run = self.run(&sql).await?;
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for row in &run.rows {
            if let (Some(Some(table)), Some(Some(column))) = (row.first(), row.get(1)) {
                map.entry(table.to_ascii_lowercase())
                    .or_default()
                    .push(column.clone());
            }
        }
        Ok(map)
    }

    /// Build the full cache. Each query degrades to empty on failure so a
    /// missing permission never kills completion entirely.
    pub async fn build_metadata(&self) -> Metadata {
        let databases = self.databases().await.unwrap_or_default();
        let tables = self.tables().await.unwrap_or_default();
        let columns = if self.database.is_empty() {
            HashMap::new()
        } else {
            self.table_columns().await.unwrap_or_default()
        };
        Metadata {
            databases,
            tables,
            columns,
        }
    }
}

/// Non-null values of each row's first column.
fn first_column(run: &athena::QueryRun) -> Vec<String> {
    run.rows
        .iter()
        .filter_map(|r| r.first().cloned().flatten())
        .collect()
}
