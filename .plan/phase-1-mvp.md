# Phase 1 실행 플랜 — MVP (접속 + 실행 + 출력 + 기본 REPL)

상위 플랜: `rust-migration-plan.md` 의 "Phase 1". 이 문서는 Phase 1 을 실제로 착수 가능한 작업 단위로 쪼갠 것이다. 현재 repo 는 빈 상태(커밋 없음)이므로 여기가 첫 코드가 들어가는 지점이다.

## 목표

실제 Amazon Athena 에 붙어서 한 줄짜리 SQL 을 실행하고 결과 표를 출력하는, 최소한으로 동작하는 터미널 클라이언트. 자동완성·special 명령·스타일은 아직 없다. 핵심은 "비동기 AWS SDK 를 동기 REPL 안에서 구동하는 다리"와 "Athena 쿼리 라이프사이클(시작→폴링→결과 페이지네이션)"을 정확히 세우는 것.

## 완료 정의 (Definition of Done)

- `cargo build` 가 Rust 1.96.0 에서 경고 없이 통과. `cargo clippy -- -D warnings`, `cargo fmt --check` 통과.
- 실제 Athena 연결로 아래가 모두 동작:
  - `SELECT 1`
  - `SHOW DATABASES`, `SHOW TABLES`
  - 세미콜론으로 끝나는 멀티라인 입력
  - `-e "SELECT 1"`(단발), `-e ./query.sql`(파일), `echo "SELECT 1" | athenacli -e -`(stdin)
- 같은 쿼리를 Python `athenacli` 와 나란히 돌려서 **출력 표·헤더·상태줄(rows in set, scanned bytes, 시간)** 이 1:1 로 일치.
- 설정 파일이 없을 때 기본 파일을 생성하고 안내 메시지 후 종료(Python 동작 미러).

## 선행 조건

- Rust 1.96.0 toolchain 설치(`rustup toolchain install 1.96.0`).
- Athena 접근 가능한 AWS 자격증명과 S3 staging 디렉터리 1개(수동 검증용).
- 비교 레퍼런스로 Python `athenacli` 가 같은 머신에서 동작.

## 작업 순서

### 1. Workspace 골격
산출물: `Cargo.toml`(workspace), `rust-toolchain.toml`, `athenacli/`, `athenacli-core/`, `.gitignore`

- 루트 `Cargo.toml` 에 `[workspace] members = ["athenacli", "athenacli-core"]`, `resolver = "2"`.
- 두 crate 매니페스트에 `rust-version = "1.96"` 명시(MSRV 미달 시 빌드가 조기 실패하도록).
- `rust-toolchain.toml`: `channel = "1.96.0"`, `components = ["rustfmt", "clippy"]`.
- `Cargo.lock` 을 커밋(AWS SDK 군의 MSRV 상승을 막기 위해 버전 핀 고정 — 상위 플랜 위험 #5).
- `.gitignore`: `/target`.
- 첫 커밋: 빈 골격 + 이 플랜.

### 2. CLI 파싱 — `athenacli/src/cli.rs`
산출물: clap derive `Parser` 구조체. Python `main.py` 의 click 옵션을 1:1 미러.

| 옵션 | 타입 | 기본값 | 비고 |
|---|---|---|---|
| `-e, --execute` | `Option<String>` | — | 명령/파일경로/`-`(stdin) |
| `-r, --region` | `Option<String>` | — | |
| `--aws-access-key-id` | `Option<String>` | — | |
| `--aws-secret-access-key` | `Option<String>` | — | |
| `--aws-session-token` | `Option<String>` | — | |
| `--s3-staging-dir` | `Option<String>` | — | |
| `--work_group` | `Option<String>` | — | `#[arg(long = "work_group")]` 로 밑줄 유지 |
| `--athenaclirc` | `PathBuf` | `~/.athenacli/athenaclirc` | |
| `--profile` | `String` | env `AWS_PROFILE` 없으면 `default` | |
| `--table-format` | `String` | `csv` | `-e` 출력 포맷 |
| `database` (positional) | `String` | `default` | `catalog.database` 형식 가능 |

- positional `database` 는 `.` 가 있으면 첫 `.` 기준으로 `(catalog, database)` 분리(Python `SQLExecute.__init__` 동일 규칙). 분리 로직은 core 의 `exec.rs` 에 두고 cli 는 문자열만 전달.
- `--help` 예시 텍스트도 Python 과 비슷하게.

### 3. 설정 — `athenacli-core/src/config.rs`
산출물: serde 구조체 + 기본값 + 첫 실행 생성. 포맷은 기존 INI 가 아닌 **새 TOML**(상위 플랜 합의사항).

- 구조체:
  - `Config { main: MainConfig, aws_profile: HashMap<String, AwsProfile>, colors: HashMap<String,String>, favorite_queries: HashMap<String,String> }`
  - `MainConfig`: `log_file`, `history_file`, `log_level`, `multi_line: bool`, `destructive_warning: String`(Python 은 문자열 "all"/"off" 류), `key_bindings: String`(emacs/vi), `prompt: String`, `prompt_continuation: String`, `timing: bool`, `table_format: String`, `syntax_style: String`, `enable_pager: bool`.
  - `AwsProfile`: 위 7개 자격/리전/스테이징 필드 전부 `Option<String>` (`aws_access_key_id`, `aws_secret_access_key`, `aws_session_token`, `region`, `s3_staging_dir`, `work_group`, `role_arn`).
- 기본값: `Default` 구현으로 athenaclirc 의 [main] 기본값을 그대로 인코딩(prompt `\d@\r> `, prompt_continuation `-> `, table_format `ascii`, log_level `INFO`, multi_line `true` 등 — 실제 값은 Python `athenaclirc` 와 대조해 채운다).
- 경로 확장은 `shellexpand`(`~` 처리), 기본 위치 산정은 `directories`.
- 첫 실행: `--athenaclirc` 가 기본 경로이고 파일이 없으면 → 기본 TOML 을 그 경로에 쓰고 환영 메시지 출력 후 `exit(1)`. (Python `cli()` 의 동작을 그대로 미러.)
- INI→TOML 매핑 메모를 주석/별도 문서로 남긴다(상위 플랜 위험 #6): `[aws_profile default]` → `[aws_profile.default]`, bool/따옴표 차이.

### 4. 자격증명 해석 — `athenacli-core/src/auth.rs`
산출물: 우선순위 해석 + `aws_sdk_athena::Client` 빌드. Python `AWSConfig` 우선순위 미러.

- 우선순위(각 필드별 첫 truthy): **CLI 플래그 > `[aws_profile.<profile>]` > AWS 기본 자격증명 체인**.
- region 은 위 우선순위에서 비면 SDK 기본 체인(env/`~/.aws/config`)으로 폴백.
- 명시 키가 주어지면 정적 자격증명 provider, 아니면 profile 이름을 건 기본 provider 체인.
- `role_arn` 이 있으면 STS AssumeRole(aws-config 의 `AssumeRoleProvider`).
- 산출: `SdkConfig` → `aws_sdk_athena::Client`. 이 호출은 비동기이므로 `main` 의 런타임에서 `block_on` 으로 1회 수행.

### 5. Athena 쿼리 라이프사이클 — `athenacli-core/src/athena.rs` (이 Phase 의 핵심, 위험 #1·#2)
산출물: 시작→폴링→페이지네이션 상태머신 + 헤더행 처리.

- `start_query_execution`:
  - `query_string`, `QueryExecutionContext { database, catalog }`, `ResultConfiguration { output_location = s3_staging_dir }`, `work_group`.
  - 반환 `query_execution_id`.
- 폴링:
  - `get_query_execution(id)` 로 상태 확인, `SUCCEEDED`/`FAILED`/`CANCELLED` 까지 `200ms` sleep 반복(Python `poll_interval=0.2`).
  - `FAILED`/`CANCELLED` 면 `StateChangeReason` 추출해 에러 메시지로.
- 결과:
  - `get_query_results(id)` 를 `next_token` 으로 페이지네이션해 전체 행 수집.
  - 행 값은 전부 `Option<String>`(텍스트). NULL vs 빈문자 구분은 `ColumnInfo` 타입 참고(상위 플랜 위험 #2).
- **헤더행 gotcha(위험 #2)**: `GetQueryResults` 는 SELECT/SHOW 에서 첫 데이터 행이 컬럼명. 규칙: 첫 페이지 0번 행의 `var_char_value` 들이 `ResultSetMetadata` 의 컬럼명과 같으면 스킵. 헬퍼 `fn should_skip_header(first_row, column_info) -> bool`. 문장 종류(첫 키워드)별 휴리스틱은 Python `pyathena` 동작과 대조.
- 산출 타입:
  - `QueryRun { headers: Vec<String>, rows: Vec<Vec<Option<String>>>, output_location: Option<String>, scanned_bytes: Option<i64>, elapsed_ms: u128, statement_kind: StatementKind }`.
- 상태줄: Python `format_utils.format_status(rows_length, cursor)` 의 출력 문자열을 **정확히** 재현해야 한다 → 구현 시 `format_utils.py` 를 직접 읽어 행 수·스캔 바이트 휴먼리더블·표기 순서를 그대로 옮긴다. (이 플랜 작성 시점엔 format_utils 세부를 확정하지 않았으므로 구현 단계 첫 작업으로 둔다.)

### 6. 동기 실행 래퍼 — `athenacli-core/src/exec.rs`
산출물: `SqlExecute` (client + tokio `Handle` + 현재 database/region/catalog + config 보유).

- `run(&self, statement: &str) -> Vec<ResultSet>`:
  - `strip` 후 빈 문자열이면 빈 결과.
  - 문장 분리: Python 은 `sqlparse.split`. Rust 는 `sqlparser` 의 토크나이저로 세미콜론 경계 분리(따옴표/주석 안의 `;` 무시). 헬퍼 `split_statements(sql) -> Vec<String>`. (sqlparser 의 엄격한 파서는 미완성 SQL 에 에러나므로 split 용도로만 사용 — 상위 플랜 명시.)
  - 각 문장 끝 `\G` 감지 → 세로 출력 플래그 set 후 `\G` 제거.
  - 정규 SQL → `self.handle.block_on(self.athena.run_async(sql))` 로 비동기 구동(상위 플랜의 block_on 브리지).
  - special 명령 디스패치 훅은 Phase 3 에서 채운다(여기선 정규 SQL 만).
- `database`/`catalog` 분리(`catalog.database`)는 여기서 처리.
- 메타데이터 메서드(`databases()`/`tables()`/`table_columns()`)는 시그니처만 두고 Phase 2 에서 구현.

### 7. 표 출력 — `athenacli-core/src/output/table.rs`
산출물: comfy-table ASCII + 세로(`\G`) + 1000행 경고 임계.

- `render_table(headers, rows, expanded) -> String`.
- expanded(`\G`/vertical): `*************************** N. row ***************************` 헤더 후 `컬럼: 값` 세로 나열(Python vertical 포맷과 대조).
- NULL 표기: `None` → 빈 문자열(또는 config 의 null 표기) — Python cli_helpers 기본과 대조.
- `THRESHOLD = 1000` 상수. 1000행 초과면 출력 전에 경고 + 진행 확인(확인 UI 는 repl 에서 `inquire`; Phase 1 에선 기본 확인만).

### 8. REPL 루프 — `athenacli/src/repl.rs`
산출물: reedline 동기 루프(최소판).

- reedline 구성: `FileBackedHistory`(history_file), 기본 `Prompt`(`\d@\r> ` 치환은 Phase 4, Phase 1 은 단순 텍스트), `;` 종료 멀티라인용 `Validator`(config `multi_line` true 일 때만), Ctrl-C(현재 줄 취소·계속), Ctrl-D(종료).
- 루프: `read_line` → trim → 빈 줄 무시 → `exec.run` → `render_table` → 상태줄 출력 → `timing` 켜져 있으면 `Time: %.03fs`.
- >1000행 확인은 여기서 `inquire::Confirm`(Phase 3 에서 정교화 가능).

### 9. 부트스트랩 — `athenacli/src/main.rs`
산출물: 런타임 소유 + 배선.

- `tokio::runtime::Builder::new_multi_thread().enable_all().build()` 를 `main` 이 소유(멀티스레드 — Phase 2 백그라운드 갱신과 공존).
- 순서: cli 파싱 → config 로드/첫 실행 생성 → tracing 파일 로거 초기화(log_file, log_level) → creds 해석(`block_on`) → `Client` + `SqlExecute` 빌드.
- `-e` 분기: 값이 `-` 면 stdin, 존재하는 경로면 파일, 아니면 그 자체를 쿼리로. 실행 후 `exit(0/1)`. 출력 포맷은 `--table-format`.
- 그 외 → `run_repl(exec)`.

## 테스트

### 단위 (`athenacli-core`)
- `config`: 기본값 직렬화/역직렬화 라운드트립, 누락 필드 기본값 적용.
- 헤더행 스킵: 합성 `GetQueryResults` 행 + `ColumnInfo` 로 `should_skip_header` 검증(스킵/비스킵 양쪽).
- 문장 분리: `sqlparse.split` 케이스 일부 이식(따옴표 안 `;`, 끝 세미콜론, 멀티 문장).
- `catalog.database` 분리.
- 상태줄 포맷(format_status 이식 결과).

### 통합/수동
- `SELECT 1`, `SHOW DATABASES`, `SHOW TABLES`, 멀티라인, `-e` 3종(문자열/파일/stdin).
- Python `athenacli` 와 동일 쿼리 나란히 → 표·헤더·상태줄 1:1 비교.

## 이 Phase 의 위험과 대응

- **위험 #1 폴링 상태머신**: idempotency, 0.2s 폴링, Failed 사유 추출, next_token 페이지네이션 — 작업 5 에 집중. 합성 응답 단위테스트로 분기 커버.
- **위험 #2 헤더행**: 작업 5 의 `should_skip_header` + 문장종류 휴리스틱. Python 과 출력 대조가 검증의 핵심.
- **위험 #5 MSRV drift**: 작업 1 의 `rust-version` 게이트 + `Cargo.lock` 고정.
- **위험 #6 설정 마이그레이션**: 작업 3 의 매핑 메모. 변환 스크립트는 별도(Phase 외) 산출물로 미룸.

## 사용 크레이트
clap(derive), tokio, aws-config, aws-credential-types, aws-sdk-athena, reedline, comfy-table, toml, serde, sqlparser(분리만), tracing, tracing-subscriber, anyhow, thiserror, directories, shellexpand, inquire.

## 다음 단계
Phase 1 동작 확인(Python 과 출력 일치) 후 `phase-2-completion.md` 착수.
