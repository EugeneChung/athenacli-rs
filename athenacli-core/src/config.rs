//! Configuration: serde-backed TOML, mirroring the defaults of the Python
//! `athenacli` INI file (`athenaclirc`).
//!
//! INI -> TOML migration notes (master plan risk #6):
//!   - `[aws_profile default]`            -> `[aws_profile.default]`
//!   - `[main]` / `[colors]` sections     -> same table names
//!   - INI bare `True`/`False`            -> TOML real `true`/`false`
//!   - INI bare / single-quoted strings   -> TOML double-quoted strings
//!   - `[favorite_queries]`               -> `[favorite_queries]`

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub main: MainConfig,
    pub aws_profile: HashMap<String, AwsProfile>,
    pub colors: HashMap<String, String>,
    pub favorite_queries: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MainConfig {
    pub log_file: String,
    pub history_file: String,
    pub log_level: String,
    pub multi_line: bool,
    pub destructive_warning: String,
    pub key_bindings: String,
    pub prompt: String,
    pub prompt_continuation: String,
    pub timing: bool,
    pub table_format: String,
    pub syntax_style: String,
    pub enable_pager: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AwsProfile {
    pub aws_access_key_id: Option<String>,
    pub aws_secret_access_key: Option<String>,
    pub aws_session_token: Option<String>,
    pub region: Option<String>,
    pub s3_staging_dir: Option<String>,
    pub work_group: Option<String>,
    pub role_arn: Option<String>,
}

impl Default for MainConfig {
    fn default() -> Self {
        // Values copied verbatim from the Python `athenaclirc` `[main]` section.
        Self {
            log_file: "~/.athenacli/app.log".to_string(),
            history_file: "~/.athenacli/history".to_string(),
            log_level: "INFO".to_string(),
            multi_line: true,
            destructive_warning: "true".to_string(),
            key_bindings: "emacs".to_string(),
            prompt: r"\r:\d> ".to_string(),
            prompt_continuation: "-> ".to_string(),
            // Diverges from Python (default on): client wall-clock timing is a
            // dev-facing detail, not useful to most users.
            timing: false,
            table_format: "ascii".to_string(),
            syntax_style: "default".to_string(),
            enable_pager: true,
        }
    }
}

impl Default for AwsProfile {
    fn default() -> Self {
        // Empty strings (not None) so the generated template shows the keys,
        // matching the Python `athenaclirc`. Empty values are treated as unset
        // during credential resolution (see `auth.rs`).
        Self {
            aws_access_key_id: Some(String::new()),
            aws_secret_access_key: Some(String::new()),
            aws_session_token: Some(String::new()),
            region: Some(String::new()),
            s3_staging_dir: Some(String::new()),
            work_group: Some(String::new()),
            role_arn: Some(String::new()),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut aws_profile = HashMap::new();
        aws_profile.insert("default".to_string(), AwsProfile::default());
        Self {
            main: MainConfig::default(),
            aws_profile,
            colors: default_colors(),
            favorite_queries: HashMap::new(),
        }
    }
}

/// The `[colors]` keys this port renders, with their built-in values, so the
/// generated config advertises what can be themed (prompt_toolkit class names
/// from the Python template that have no reedline counterpart are dropped).
/// Values use the prompt_toolkit style-string format (`style::parse_style`).
fn default_colors() -> HashMap<String, String> {
    [
        ("completion-menu.completion", "bg:#008888 #ffffff"),
        ("completion-menu.completion.current", "bg:#ffffff #000000"),
        ("auto-suggestion", "#666666 italic"),
        ("sql.keyword", "green bold"),
        ("sql.string", "#ba2121"),
        ("sql.number", "#666666"),
        ("sql.comment", "#408080 italic"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&text)?;
        Ok(cfg)
    }

    /// Returns the profile section if present (after looking up `aws_profile.<name>`).
    pub fn profile(&self, name: &str) -> Option<&AwsProfile> {
        self.aws_profile.get(name)
    }

    pub fn to_toml(&self) -> anyhow::Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }
}

/// Default config path, mirroring Python's hardcoded `~/.athenacli/athenaclirc`.
pub fn default_config_path() -> PathBuf {
    home_dir().join(".athenacli").join("athenaclirc")
}

/// Expand a leading `~` to the user's home directory.
pub fn expand(path: &str) -> String {
    shellexpand::tilde(path).into_owned()
}

/// Write a fresh default config to `path`, creating parent directories.
pub fn write_default(path: &Path) -> anyhow::Result<()> {
    save(&Config::default(), path)
}

/// Persist `cfg` to `path` (favorite queries write-back), creating parent
/// directories. Re-serializes the whole file; TOML comments are not kept.
pub fn save(cfg: &Config, path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, cfg.to_toml()?)?;
    Ok(())
}

fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips_through_toml() {
        let cfg = Config::default();
        let text = cfg.to_toml().expect("serialize");
        let parsed: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // Only `[main] timing` provided; everything else must default.
        let parsed: Config = toml::from_str("[main]\ntiming = false\n").expect("parse");
        assert!(!parsed.main.timing);
        assert_eq!(parsed.main.table_format, "ascii");
        assert_eq!(parsed.main.prompt, r"\r:\d> ");
        // aws_profile default section is supplied by Config::default().
        assert!(parsed.aws_profile.contains_key("default"));
    }

    #[test]
    fn defaults_match_python_athenaclirc() {
        let m = MainConfig::default();
        assert_eq!(m.log_file, "~/.athenacli/app.log");
        assert_eq!(m.history_file, "~/.athenacli/history");
        assert_eq!(m.log_level, "INFO");
        assert!(m.multi_line);
        // Intentional divergence from Python's default-on (see MainConfig::default).
        assert!(!m.timing);
        assert!(m.enable_pager);
        assert_eq!(m.key_bindings, "emacs");
        // Python athenaclirc ships `\r:\d> ` (region:database).
        assert_eq!(m.prompt, r"\r:\d> ");
        assert_eq!(m.prompt_continuation, "-> ");
    }
}
