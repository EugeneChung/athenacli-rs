# athenacli-rs

A Rust port of [athenacli](https://github.com/dbcli/athenacli) — an interactive
terminal client for Amazon Athena with auto-completion, syntax highlighting,
and the dbcli-style special commands. Single static binary, no Python runtime.

This README is a quick tour; the complete documentation — every CLI flag,
config key, key binding, and special command — is in the
[user manual](docs/manual.md).

## Features

- Interactive REPL with multiline editing, persistent history, and
  fish-style autosuggestions from history (Right arrow to accept)
- Context-aware completion (keywords, databases, tables, columns) refreshed
  in the background
- SQL syntax highlighting while typing, themeable via `[colors]`
- Prompt templating (`\d` database, `\r` region, date/time tokens) with the
  classic `region:database>` default
- Special commands (`\dt`, `\l`, `\f`, `tee`, `watch`, `download`, … — run
  `help` inside the REPL), favorite queries, pager/tee output
- Destructive-query confirmation, Ctrl-C cancels the running Athena query
  server-side
- One-shot mode: `athenacli -e "select 1"` (CSV by default)

## Install

Requires Rust (see `rust-toolchain.toml`; rustup picks it automatically):

```sh
cargo install --path athenacli
```

## Usage

```sh
athenacli                          # connect with the default AWS profile
athenacli my_database              # pick a database (catalog.database works too)
athenacli -e "show databases"      # one-shot execution
athenacli --profile prod -r us-east-1 --s3-staging-dir s3://bucket/prefix/
```

Credentials resolve from CLI flags, then the `[aws_profile.<name>]` config
section, then the standard AWS chain (env vars, `~/.aws`, instance roles).

## Key bindings

| Key | Action |
| --- | --- |
| Tab / Ctrl-Space | Open the completion menu / next candidate |
| F2 | Toggle smart completion (off = prefix-match every known name) |
| F3 | Toggle multiline mode |
| F4 | Toggle Vi / Emacs editing (Vi shows an `(I)`/`(N)` marker) |
| Ctrl-R | Reverse history search |
| Ctrl-C / Ctrl-D | Cancel input or running query / exit |

The current toggle states are shown in the right-side prompt (the Python
version's bottom toolbar).

## Configuration

The config lives at `~/.athenacli/athenaclirc` (override with
`--athenaclirc`) and is **TOML** — a fresh default file is generated on first
run. Main knobs:

```toml
[main]
multi_line = true
destructive_warning = "true"
key_bindings = "emacs"          # or "vi"
prompt = '\r:\d> '              # \d database, \r region, \w workgroup,
                                # \n newline, \D full date, \R hour, \m min,
                                # \s sec, \P AM/PM
prompt_continuation = "-> "
timing = false
table_format = "ascii"
enable_pager = true

[aws_profile.default]
region = "us-east-1"
s3_staging_dir = "s3://your-bucket/athena-results/"
work_group = ""

[colors]
# prompt_toolkit-style strings: '#rrggbb', 'bg:#rrggbb', names, bold/italic
"sql.keyword" = "green bold"
"completion-menu.completion" = "bg:#008888 #ffffff"

[favorite_queries]
top = "select * from $1 limit 10"   # run with: \f top my_table
```

### Migrating from the Python athenaclirc (INI)

The Python version used INI; the keys are the same but the syntax differs:

| INI (Python) | TOML (this port) |
| --- | --- |
| `[aws_profile default]` | `[aws_profile.default]` |
| `multi_line = True` | `multi_line = true` |
| `prompt = '\r:\d> '` (bare/single-quoted) | `prompt = '\r:\d> '` (quotes required) |
| `destructive_warning = True` | `destructive_warning = "true"` |
| `[favorite_queries]` | unchanged |
| `[colors]` pygments/ptk class names | only the keys shown above are rendered |

Move your old file aside, run `athenacli` once to generate the TOML template,
then copy your values in.

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

The workspace has two crates: `athenacli-core` (config, auth, query
lifecycle, completion, output — testable without a TTY) and `athenacli` (the
binary: CLI args, REPL wiring). Implementation plans live in `.plan/`.
