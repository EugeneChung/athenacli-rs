# athenacli-rs User Manual

athenacli-rs is an interactive terminal client for Amazon Athena — a Rust port
of the Python [athenacli](https://github.com/dbcli/athenacli). It ships as a
single static binary with no Python runtime, and keeps the dbcli workflow:
context-aware completion, syntax highlighting, special commands, favorite
queries, pager/tee output.

This manual covers everything the tool does. For a quick tour, see the
[README](../README.md).

## Contents

- [Installation](#installation)
- [First run](#first-run)
- [Command-line reference](#command-line-reference)
- [Connection and credentials](#connection-and-credentials)
- [Configuration reference](#configuration-reference)
- [Using the REPL](#using-the-repl)
- [Running queries](#running-queries)
- [Output, pager, tee](#output-pager-tee)
- [Special commands](#special-commands)
- [Favorite queries](#favorite-queries)
- [One-shot mode (-e)](#one-shot-mode--e)
- [Files](#files)
- [Migrating from the Python athenacli](#migrating-from-the-python-athenacli)
- [Differences from the Python version](#differences-from-the-python-version)

## Installation

Requires the Rust toolchain pinned in `rust-toolchain.toml` (rustup picks it
up automatically):

```sh
cargo install --path athenacli
```

This builds a release binary into `~/.cargo/bin/athenacli`.

## First run

If the default config file `~/.athenacli/athenaclirc` does not exist,
athenacli prints a welcome message, generates a default config there, and
exits with status 1:

```
Welcome to athenacli!

It seems this is your first time to run athenacli,
we generated a default config file for you
    /Users/you/.athenacli/athenaclirc
Please change it accordingly, and run athenacli again.
```

Fill in at least `region` and `s3_staging_dir` under `[aws_profile.default]`
(or rely on your `~/.aws` setup and a workgroup that defines an output
location), then run `athenacli` again.

This first-run flow only applies to the default location; with
`--athenaclirc <path>` a missing file is not generated and built-in defaults
are used.

## Command-line reference

```
athenacli [OPTIONS] [DATABASE]
```

| Option | Description |
| --- | --- |
| `[DATABASE]` | Database to connect to, default `default`. The `catalog.database` form selects a catalog other than the default `AwsDataCatalog`. |
| `-e, --execute <QUERY>` | Execute and quit ([one-shot mode](#one-shot-mode--e)). The argument is a query string, a path to a SQL file, or `-` for stdin. |
| `-r, --region <REGION>` | AWS region. |
| `--aws-access-key-id <KEY>` | AWS access key id. |
| `--aws-secret-access-key <KEY>` | AWS secret access key. |
| `--aws-session-token <TOKEN>` | AWS session token. |
| `--s3-staging-dir <S3URI>` | S3 location where Athena stores query results, e.g. `s3://bucket/prefix/`. |
| `--work_group <NAME>` | Athena workgroup (note the underscore, kept from the Python CLI). |
| `--profile <NAME>` | Profile name, default `default`. Also read from the `AWS_PROFILE` environment variable. Selects both the `[aws_profile.<name>]` config section and the AWS shared-config profile. |
| `--athenaclirc <PATH>` | Use a config file other than `~/.athenacli/athenaclirc`. |
| `--table-format <FMT>` | Output format for `-e` mode only, default `csv`. One of `ascii`, `csv`, `vertical`. |
| `-V, --version` | Print version. |
| `-h, --help` | Print help. |

Examples:

```sh
athenacli                                  # default profile, default database
athenacli my_db                            # pick a database
athenacli AwsDataCatalog.my_db             # catalog.database
athenacli --profile prod -r us-east-1 --s3-staging-dir s3://bucket/results/
athenacli -e "show databases"              # one-shot, CSV output
athenacli -e queries.sql                   # one-shot from a file
echo "select 1" | athenacli -e -           # one-shot from stdin
```

## Connection and credentials

Each connection setting is resolved field by field, first match wins:

1. CLI flag
2. `[aws_profile.<profile>]` section in the config file (empty strings count
   as unset)
3. The standard AWS chain (environment variables, `~/.aws/credentials` /
   `~/.aws/config` for the same profile name, SSO, instance roles, …)

The region additionally falls back to the SDK's default region chain when
neither the flag nor the config provides one.

`role_arn` is config-only (no CLI flag): when set in the profile section,
athenacli assumes that IAM role (STS session name `athenacli`) on top of the
resolved base credentials.

The S3 staging directory may be omitted if the chosen workgroup defines a
result output location; Athena requires one of the two.

## Configuration reference

The config file is TOML, default location `~/.athenacli/athenaclirc`.
Every key is optional — missing keys fall back to the defaults below.

### `[main]`

| Key | Default | Meaning |
| --- | --- | --- |
| `log_file` | `"~/.athenacli/app.log"` | Where the application log is written. |
| `log_level` | `"INFO"` | One of `CRITICAL`/`ERROR`, `WARNING`, `INFO`, `DEBUG`, or `NONE` to disable logging. |
| `history_file` | `"~/.athenacli/history"` | REPL history file (capacity 2000 entries). |
| `multi_line` | `true` | Start with multiline editing on: statements run only when terminated with `;` (toggle at runtime with F3). |
| `destructive_warning` | `"true"` | Ask before running `DROP`/`DELETE`/`TRUNCATE`/`SHUTDOWN`. A string: `"true"`, `"1"`, `"yes"`, `"on"` enable it (case-insensitive); anything else disables. |
| `key_bindings` | `"emacs"` | Initial editing mode, `"emacs"` or `"vi"` (toggle at runtime with F4). |
| `prompt` | `'\r:\d> '` | Prompt template, see [Prompt templates](#prompt-templates). |
| `prompt_continuation` | `"-> "` | Prefix shown on continuation lines of a multiline statement. |
| `timing` | `false` | Print client-side wall-clock time after each result. |
| `table_format` | `"ascii"` | REPL output format: `ascii`, `csv`, or `vertical`. |
| `syntax_style` | `"default"` | Accepted for compatibility with the Python config; not used. Colors come from `[colors]`. |
| `enable_pager` | `true` | Allow long results to go through the pager. |

### `[aws_profile.<name>]`

One table per profile; `--profile` picks which one is read. All keys are
optional and empty strings mean "not set".

| Key | Meaning |
| --- | --- |
| `aws_access_key_id` | Static access key. |
| `aws_secret_access_key` | Static secret key. |
| `aws_session_token` | Session token for temporary credentials. |
| `region` | AWS region. |
| `s3_staging_dir` | S3 location for query results. |
| `work_group` | Athena workgroup. |
| `role_arn` | IAM role to assume after the base credentials resolve. |

### `[colors]`

Values use the prompt_toolkit style-string format: a foreground color
(`green`, `#ba2121`), an optional background (`bg:#008888`), and the
modifiers `bold`/`italic`, space-separated. Only these keys are rendered:

| Key | Default | Styles |
| --- | --- | --- |
| `sql.keyword` | `"green bold"` | SQL keywords while typing. |
| `sql.string` | `"#ba2121"` | String literals. |
| `sql.number` | `"#666666"` | Numeric literals. |
| `sql.comment` | `"#408080 italic"` | `--` and `/* */` comments. |
| `completion-menu.completion` | `"bg:#008888 #ffffff"` | Completion menu entries. |
| `completion-menu.completion.current` | `"bg:#ffffff #000000"` | The selected menu entry. |
| `auto-suggestion` | `"#666666 italic"` | The gray inline history suggestion. |

### `[favorite_queries]`

Saved queries, `name = "query"`. Managed with `\fs` / `\fd` from inside the
REPL (which rewrite this file), or edited by hand. See
[Favorite queries](#favorite-queries).

## Using the REPL

### Entering statements

With multiline on (the default), a statement is submitted when it ends with
`;` (or `\g` / `\G`); until then Enter inserts a new line prefixed with
`prompt_continuation`. These inputs submit immediately regardless:

- special commands (anything starting with `\`)
- `exit`, `quit`, `:q`

With multiline off (F3), every Enter submits.

A single input line may contain several statements separated by `;`; they run
in sequence, each with its own result and status line.

### Key bindings

| Key | Action |
| --- | --- |
| Tab / Ctrl-Space | Open the completion menu; press again for the next candidate. |
| Right arrow | Accept the gray history autosuggestion. |
| Ctrl-R | Reverse history search. |
| F2 | Toggle smart completion. |
| F3 | Toggle multiline mode. |
| F4 | Toggle Vi / Emacs editing mode. |
| Ctrl-C | While editing: discard the current input. While a query runs: cancel it. |
| Ctrl-D | Exit (same as `exit`, `quit`, `\q`). |

Everything else follows the reedline defaults for the active mode (Emacs
bindings, or Vi insert/normal modes). In Vi mode the prompt shows an `(I)` /
`(N)` marker for the current sub-mode. F2/F3/F4 preserve whatever you have
typed so far.

The right-hand side of the screen shows the live toggle state — the
equivalent of the Python version's bottom toolbar:

```
[F2] Smart: ON  [F3] Multiline: ON  [F4] Emacs
```

### Completion

Completion metadata (databases, tables, columns) is fetched in the background
at startup, after `use`, and after schema-changing statements (`CREATE`,
`DROP`, `ALTER`, …), so the menu never blocks on the network.

- **Smart completion** (default): suggestions depend on the cursor context —
  tables after `FROM`, columns of the tables in scope after `SELECT`/`WHERE`,
  databases after `USE`, keywords elsewhere.
- **Dumb completion** (F2 off): plain prefix match over every known keyword
  and name.

History powers fish-style autosuggestions: the rest of a previous matching
line appears in gray, Right arrow accepts it.

### Prompt templates

The `prompt` config key (and the `prompt` / `\R` special command) accept a
template with these tokens:

| Token | Replaced with |
| --- | --- |
| `\d` | Current database (`(none)` when empty). |
| `\r` | AWS region (`(none)` when unknown). |
| `\w` | Workgroup. |
| `\n` | Newline. |
| `\D` | Full date, e.g. `Wed Jun 10 14:05:07 2026`. |
| `\R` | Hour, 24-hour clock. |
| `\m` | Minutes. |
| `\s` | Seconds. |
| `\P` | `AM` / `PM`. |

Date/time tokens refresh on every keystroke. If the substituted prompt
exceeds 45 characters, it falls back to the short form `\r:\d> `.

```
ap-northeast-2:default> prompt \d@\r>
Changed prompt format to \d@\r>
default@ap-northeast-2>
```

The runtime change is not saved; set `prompt` in the config to keep it.

## Running queries

For each SQL statement athenacli prints, in order:

1. `Athena URL: …` — a link to the query execution in the AWS console.
2. The result table (when the statement returns one).
3. A status line:

```
25 rows in set
Execution time: 1340 ms, Data scanned: 1.2 MB, Approximate cost: $0.00
```

`Query OK` replaces the row count for statements without rows. Execution
time is Athena's engine time; the approximate cost assumes $5 per TB
scanned. With `timing` on (or after `\timing`), a client-side
`Time: 1.402s` line follows.

### Vertical output

Append `\G` to a statement to render the result one column per line —
useful for wide rows:

```
ap-northeast-2:default> select * from elb_logs limit 1\G
```

### Large results

When a result has more than 1000 rows, athenacli asks
`Do you want to continue?` (default yes) before printing.

### Destructive statements

With `destructive_warning` on, statements starting with `DROP`, `DELETE`,
`TRUNCATE`, or `SHUTDOWN` (leading comments are skipped) prompt:

```
You're about to run a destructive command.
Do you want to proceed? (y/N)
```

Declining prints `Wise choice!` and skips the input; the check also applies
to each statement run via `read` and `watch`. The prompt only appears on an
interactive terminal.

### Cancelling

Ctrl-C while a query is running stops it server-side (Athena
`StopQueryExecution`), so you stop paying for it; the REPL returns to the
prompt. Ctrl-C while editing just clears the input.

## Output, pager, tee

### Table formats

`ascii` (bordered table), `csv`, and `vertical` (one column per line, like
`\G`). The REPL uses `[main] table_format`; one-shot mode uses
`--table-format` (default `csv`). Switch at runtime with
`tableformat <name>` / `\T <name>`.

### Pager

When a result is taller than the free terminal space (or any line is wider
than the terminal), the output goes through your pager instead of scrolling
the screen; smaller results print directly. In the pager (`less`), use
`q` to return to the prompt.

- The pager command is `$PAGER`, falling back to `less -R`.
- `pager <command>` / `\P <command>` sets `$PAGER` for the session and
  re-enables paging; `pager` with no argument just re-enables it.
- `nopager` / `\n` disables paging (everything prints to stdout).
- `enable_pager = false` in the config disables it from the start.
- `watch` disables the pager while it runs.

### tee — copy results to a file

`tee [-o] <file>` appends every following prompt line, query, and result
table (status lines excluded) to the file; `-o` truncates it first.
`notee` stops. `\once [-o] <file>` (alias `\o`) does the same for the next
result only.

```
ap-northeast-2:default> tee -o /tmp/session.log
ap-northeast-2:default> select 1;
…
ap-northeast-2:default> notee
```

## Special commands

Run `help` (or `\?`) inside the REPL to list these. Type them in lowercase
as shown — most word commands accept any case, but the lowercase form always
works. Special commands also work in `-e` one-shot mode.

| Command | Shortcut | Description |
| --- | --- | --- |
| `help` | `\?` | List special commands. `help <keyword>` forwards `help '<keyword>'` to Athena. |
| `exit`, `quit` | `\q` | Leave athenacli (Ctrl-D works too). |
| `use <db>` | `\u <db>` | Switch database (`use catalog.db` switches the catalog too); completion metadata reloads for it. |
| `\l` | | List databases (`SHOW DATABASES`). |
| `\dt [table]` | | List tables (`SHOW TABLES`), or with an argument show that table's columns (`SHOW COLUMNS FROM …`). |
| `prompt <template>` | `\R <template>` | Change the prompt for this session. |
| `tableformat <fmt>` | `\T <fmt>` | Change the result format: `ascii`, `csv`, `vertical`. |
| `\timing` | `\t` | Toggle the client-side `Time:` line. |
| `pager [command]` | `\P [command]` | Set `$PAGER` and/or re-enable paging. |
| `nopager` | `\n` | Disable paging. |
| `tee [-o] <file>` | | Append results to a file (`-o` overwrites). `notee` stops. |
| `\once [-o] <file>` | `\o` | Write only the next result to a file. |
| `\f [name [args…]]` | | List or run favorite queries. |
| `\fs <name> <query>` | | Save a favorite query. |
| `\fd <name>` | | Delete a favorite query. |
| `\e [file]` | | Compose the input in `$EDITOR`, see below. |
| `read <file>` | | Execute the statements in a SQL file. |
| `system <command>` | | Run a shell-less system command; `system cd <dir>` changes athenacli's working directory. |
| `watch [sec] [-c] <query>` | | Re-run a query every `sec` seconds (default 5) until Ctrl-C; `-c` clears the screen between rounds. |
| `download` | | Download the last query's result file from S3 to `/tmp/`. |

Details worth knowing:

- **`\e` (external editor)** — opens `$VISUAL`, else `$EDITOR`, else `vi`.
  `\e` alone edits the last submitted input; `SELECT … \e` edits that text;
  `\e <file>` edits the file. In the temp-file case, keep your query above
  the `# Type your query above this line.` marker. After the editor exits,
  the text is placed on the prompt — press Enter to run it (or `\e` again to
  keep editing).
- **`system`** — the command is split on spaces and run without a shell, so
  pipes, globs, and quoting do not apply. stderr is shown when the command
  produces any.
- **`watch`** — example: `watch 10 -c select count(*) from events;` reruns
  every 10 seconds with a screen clear. Each round is a full (billed) Athena
  query.
- **`download`** — Athena stores every result as an object in the staging
  S3 location; this fetches the one from the most recent query, e.g.
  `Saved 5882 bytes to /tmp/<query-id>.csv`.

## Favorite queries

Favorites are named queries persisted in the `[favorite_queries]` config
section:

```
ap-northeast-2:default> \fs top select * from $1 limit $2
Saved.
ap-northeast-2:default> \f top elb_logs 10
> select * from elb_logs limit 10
…
ap-northeast-2:default> \f
+------+--------------------------------+
| Name | Query                          |
+------+--------------------------------+
| top  | select * from $1 limit $2      |
+------+--------------------------------+
ap-northeast-2:default> \fd top
top: Deleted
```

`$1`…`$N` are positional parameters substituted from the `\f` arguments
(shell-style quoting supported: `\f top "my table" 10`). Supplying too many
or too few arguments is an error. A favorite may contain several
`;`-separated statements.

Note: `\fs` / `\fd` rewrite the config file, so comments you added there by
hand are lost.

## One-shot mode (-e)

`athenacli -e <arg>` runs without the REPL and exits 0 on success, 1 on
error. The argument is:

- `-` — read the query from stdin,
- an existing file path — read the query from that file,
- anything else — the query text itself.

Output is the Athena console URL plus the result in `--table-format`
(default `csv`, so it pipes cleanly); no status line, no timing. Multiple
`;`-separated statements run in order. Special commands work here too, and
destructive statements still ask for confirmation when run from a terminal.

```sh
athenacli -e "select elb_name, count(*) c from elb_logs group by 1" > out.csv
```

## Files

| Path | Purpose |
| --- | --- |
| `~/.athenacli/athenaclirc` | Config (TOML). Generated on first run; `--athenaclirc` overrides. |
| `~/.athenacli/history` | REPL history (`history_file`). |
| `~/.athenacli/app.log` | Application log (`log_file`, `log_level`). |

## Migrating from the Python athenacli

The keys are the same but the file is TOML instead of INI:

| INI (Python) | TOML (this port) |
| --- | --- |
| `[aws_profile default]` | `[aws_profile.default]` |
| `multi_line = True` | `multi_line = true` |
| `prompt = \r:\d> ` (bare) | `prompt = '\r:\d> '` (quotes required; single quotes keep backslashes literal) |
| `destructive_warning = True` | `destructive_warning = "true"` (a string) |
| `[favorite_queries]` | unchanged |
| `[colors]` pygments/prompt_toolkit class names | only the keys listed in [`[colors]`](#colors) are rendered |

Move your old INI aside, run `athenacli` once to generate the TOML template,
and copy your values in.

## Differences from the Python version

- **Config format** — TOML instead of INI (see above).
- **`timing` defaults to off** (Python: on).
- **Bottom toolbar** — the line editor has no bottom toolbar; the toggle
  states show in the right-side prompt instead.
- **`download`** — uses the AWS SDK directly instead of shelling out to
  `aws s3 cp`.
- **Completion menu styling** is simpler; only the `[colors]` keys listed
  above take effect.
- **`--table-format`** applies to `-e` mode only, as in Python, but the
  supported formats are `ascii`/`csv`/`vertical` (no tabulate variants).
