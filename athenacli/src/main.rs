mod cli;
mod repl;

use std::io::Read;
use std::path::Path;

use anyhow::Context;
use athenacli_core::auth::{self, CliCreds};
use athenacli_core::config::{self, Config};
use athenacli_core::exec::SqlExecute;
use athenacli_core::output;
use clap::Parser;
use tracing::Level;

fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();

    let default_path = config::default_config_path();
    // First-run welcome only applies to the default config location, matching
    // Python's `athenaclirc == ATHENACLIRC` check.
    let (config_path, welcome_eligible) = match &args.athenaclirc {
        Some(p) => (p.clone(), false),
        None => (default_path, true),
    };

    if welcome_eligible && !config_path.exists() {
        print_welcome(&config_path);
        config::write_default(&config_path)?;
        std::process::exit(1);
    }

    let cfg = if config_path.exists() {
        Config::load(&config_path)
            .with_context(|| format!("failed to read config {}", config_path.display()))?
    } else {
        Config::default()
    };

    init_logging(&cfg);

    // main owns the multi-threaded runtime; the REPL stays synchronous and
    // drives async calls via Handle::block_on (master plan bridge).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let cli_creds = CliCreds {
        access_key_id: args.aws_access_key_id.clone(),
        secret_access_key: args.aws_secret_access_key.clone(),
        session_token: args.aws_session_token.clone(),
        region: args.region.clone(),
        s3_staging_dir: args.s3_staging_dir.clone(),
        work_group: args.work_group.clone(),
    };
    let spec = auth::resolve(&cli_creds, &args.profile, cfg.profile(&args.profile));

    let (client, resolved_region) = runtime
        .block_on(auth::build_client(&spec))
        .context("failed to build Athena client")?;

    let exec = SqlExecute::new(
        client,
        runtime.handle().clone(),
        &args.database,
        spec.s3_staging_dir.clone(),
        spec.work_group.clone(),
        resolved_region,
    );

    if let Some(execute) = &args.execute {
        let query = read_execute_arg(execute)?;
        match run_oneshot(&exec, &query, &args.table_format) {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
    }

    repl::run(&exec, &cfg)
}

/// `-e` argument: `-` reads stdin, an existing path reads the file, otherwise
/// the argument itself is the query (mirrors Python `cli`).
fn read_execute_arg(arg: &str) -> anyhow::Result<String> {
    if arg == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        if buf.trim().is_empty() {
            anyhow::bail!("No query to execute on stdin");
        }
        Ok(buf)
    } else if Path::new(arg).exists() {
        std::fs::read_to_string(arg).with_context(|| format!("failed to read {arg}"))
    } else {
        Ok(arg.to_string())
    }
}

/// `-e` output: Athena console URL then result tables (no status line / timing,
/// matching Python `run_query` plus the REPL's URL line).
fn run_oneshot(exec: &SqlExecute, query: &str, table_format: &str) -> anyhow::Result<()> {
    for rs in exec.run(query)? {
        if let Some(url) = exec.console_url(&rs.run.query_execution_id) {
            println!("Athena URL: {url}");
        }
        let rendered = output::render(&rs.run.headers, &rs.run.rows, table_format, rs.expanded);
        if !rendered.is_empty() {
            println!("{rendered}");
        }
    }
    Ok(())
}

fn print_welcome(path: &Path) {
    println!(
        "
        Welcome to athenacli!

        It seems this is your first time to run athenacli,
        we generated a default config file for you
            {}
        Please change it accordingly, and run athenacli again.
        ",
        path.display()
    );
}

fn init_logging(cfg: &Config) {
    let level = match cfg.main.log_level.to_uppercase().as_str() {
        "NONE" => return,
        "CRITICAL" | "ERROR" => Level::ERROR,
        "WARNING" => Level::WARN,
        "DEBUG" => Level::DEBUG,
        _ => Level::INFO,
    };

    let path = config::expand(&cfg.main.log_file);
    if let Some(dir) = Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(level)
            .with_writer(move || file.try_clone().expect("clone log file handle"))
            .try_init();
    }
}
