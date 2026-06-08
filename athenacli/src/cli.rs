use std::path::PathBuf;

use clap::Parser;

/// A Athena terminal client with auto-completion and syntax highlighting.
///
/// Examples:
///   - athenacli
///   - athenacli my_database
#[derive(Debug, Parser)]
#[command(name = "athenacli", version, verbatim_doc_comment)]
pub struct Cli {
    /// Execute a command (or a file) and quit.
    #[arg(short = 'e', long = "execute")]
    pub execute: Option<String>,

    /// AWS region.
    #[arg(short = 'r', long = "region")]
    pub region: Option<String>,

    /// AWS access key id.
    #[arg(long = "aws-access-key-id")]
    pub aws_access_key_id: Option<String>,

    /// AWS secret access key.
    #[arg(long = "aws-secret-access-key")]
    pub aws_secret_access_key: Option<String>,

    /// AWS session token.
    #[arg(long = "aws-session-token")]
    pub aws_session_token: Option<String>,

    /// Amazon S3 staging directory where query results are stored.
    #[arg(long = "s3-staging-dir")]
    pub s3_staging_dir: Option<String>,

    /// Amazon Athena workgroup in which query is run, default is primary.
    #[arg(long = "work_group")]
    pub work_group: Option<String>,

    /// Location of athenaclirc file.
    #[arg(long = "athenaclirc")]
    pub athenaclirc: Option<PathBuf>,

    /// AWS profile.
    #[arg(long = "profile", env = "AWS_PROFILE", default_value = "default")]
    pub profile: String,

    /// Table format used with -e option.
    #[arg(long = "table-format", default_value = "csv")]
    pub table_format: String,

    /// catalog.database to connect to.
    #[arg(default_value = "default")]
    pub database: String,
}
