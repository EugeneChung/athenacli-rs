# athenacli → Rust 전환 플랜 (athenacli-rs)

## Context (왜 하는가)

`athenacli` 는 Amazon Athena 대화형 터미널 클라이언트로, dbcli 계열(mycli/litecli)을 fork 한 Python 프로젝트다 (~3,245 LOC, `/Users/euigeun/project/opensources/athenacli`). 무거운 Python 의존성 묶음(click, prompt_toolkit, pyathena, sqlparse, cli_helpers, pygments, configobj, boto3) 위에 올라가 있어서 시작이 느리고 배포 시 Python 런타임·패키지 설치가 필요하다.

Rust 1.96.0 로 재작성하면 단일 정적 바이너리로 배포 가능하고, 시작이 빠르며, 메모리 안전성을 얻고, Python 런타임 의존을 제거할 수 있다. 기능은 한 번에 다 옮기지 않고 핵심(접속→쿼리→출력)부터 단계적으로 올려서 위험을 분산한다.

### 합의된 결정사항
- **위치**: 별도 새 repo `athenacli-rs` (본 repo). 기존 Python repo 는 그대로 두고 동작 비교용 레퍼런스로 사용.
- **범위**: 단계적 MVP 우선 (Phase 1 동작 확인 후 다음 단계)
- **라인 에디터**: reedline (nushell 라인 에디터 — 커스텀 Completer/Highlighter, vi/emacs, 멀티라인 지원)
- **설정**: 기존 INI(configobj) 대신 새 TOML 포맷 (serde 기반). 기존 사용자에게는 변환 안내/스크립트 제공.
- **타겟 toolchain**: Rust 1.96.0

---

## 현재 Python 구조 (파악 완료)

| 영역 | Python 파일 | 역할 |
|---|---|---|
| 진입/CLI | `main.py` (cli 함수) | click 옵션 파싱, 첫 실행 시 기본 설정 생성, `-e` 단발 실행 또는 REPL 진입 |
| REPL 루프 | `main.py` (run_cli/one_iteration) | prompt_toolkit 로 줄 입력 → 에디터 명령·파괴적 쿼리 확인 → 실행 → 결과 포맷·출력, >1000행 경고, timing, tee/pager |
| Athena 실행 | `sqlexecute.py` | `pyathena.connect()` 래핑, SQL 분리(`sqlparse`), `\G` 처리, special 명령 dispatch, `(title, rows, headers, status)` yield, 메타데이터 조회 |
| 자동완성 | `completer.py`, `packages/completion_engine.py`, `completion_refresher.py`, `packages/parseutils.py` | 커서 위치 기반 문맥 자동완성(FROM→테이블, WHERE→컬럼 등), 백그라운드 스레드로 메타데이터 갱신 후 completer 교체 |
| special 명령 | `packages/special/*` | 14개 백슬래시/이름 명령 (`\e`,`\G`,pager,tee,`\f`/`\fs`/`\fd`,read,system,watch,download 등), 전역 dict + 데코레이터 등록 |
| 설정 | `config.py`, `athenaclirc` | configobj INI 파싱, AWSConfig 자격증명 우선순위 (CLI > 설정파일 > boto3 기본 체인) |
| 스타일/UI | `clistyle.py`,`style.py`,`lexer.py`,`clibuffer.py`,`clitoolbar.py`,`key_bindings.py` | pygments→prompt_toolkit 스타일, 멀티라인 판정, 하단 toolbar, F2/F3/F4 키바인딩 |

---

## Rust 아키텍처 핵심 결정

### 1. async SDK ↔ 동기 reedline 브리지 (가장 중요)
AWS SDK for Rust 는 async(tokio) 전용인데 reedline 의 `read_line()` 은 블로킹이다. **`main` 이 tokio 멀티스레드 런타임을 소유하고, REPL 루프는 완전 동기로 돌리되, 쿼리 실행마다 `Handle::block_on` 으로 async 호출을 구동**한다. (REPL 을 async 로 만들고 에디터를 `spawn_blocking` 하는 방식은 reedline 이 터미널 raw 모드를 잡고 거의 항상 입력 대기 상태라 비효율 → 채택 안 함.)

```rust
fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let client = rt.block_on(build_athena_client(&auth))?;   // 비동기 초기화 1회
    let exec = SqlExecute::new(client, rt.handle().clone());  // Runtime 아닌 Handle 보유
    run_repl(exec)?;                                          // 완전 동기 루프
    Ok(())
}
// SqlExecute::run(sql) 내부: self.handle.block_on(self.run_async(sql))
```
멀티스레드 런타임이라 백그라운드 자동완성 갱신 task(`tokio::spawn`)가 `block_on` 중인 쿼리와 동시에 돈다. 런타임 두 개 불필요.

### 2. 자동완성 동시성: ArcSwap
reedline 의 `Completer::complete(&mut self, line, pos) -> Vec<Suggestion>` 은 **동기**라서 그 안에서 네트워크 조회 불가. 메타데이터는 백그라운드 task 가 미리 받아두고 `arc_swap::ArcSwap<Metadata>` 에 원자적 `store` 한다. completer 는 매 키 입력마다 `meta.load()` 로 **락 없이** 읽는다 (RwLock 은 쓰기 중 키 입력이 막힐 위험 → ArcSwap 채택). 이는 Python 의 "스레드+콜백으로 completer 교체" 패턴을 더 깔끔하게 대체.

### 3. 설정: serde + toml
`#[derive(Deserialize/Serialize)]` 구조체. INI 의 `[aws_profile default]` → TOML `[aws_profile.default]` 테이블. 즐겨찾기 쿼리도 TOML 영속화.

### 4. Workspace 2-crate
- `athenacli` (bin): CLI 파싱 + REPL 배선
- `athenacli-core` (lib): 모든 로직 (테스트 용이)

---

## Crate 선택 (MSRV Rust 1.96.0 검증 완료, 2026-06-09 기준)

가장 빡빡한 제약은 AWS SDK 군: `aws-sdk-athena 1.109`, `aws-config 1.8.18`, `aws-sdk-s3 1.135` 모두 MSRV 1.91.1 → 1.96 충족. **SDK 군은 버전 핀 고정** (분기마다 MSRV 상승 경향 → `Cargo.lock` 고정 + 매니페스트 `rust-version = "1.96"` 로 조기 실패).

| 관심사 | crate | 대체 대상(Python) |
|---|---|---|
| 라인 에디터 | `reedline 0.48` | prompt_toolkit |
| Athena | `aws-sdk-athena 1.109` | pyathena |
| 자격증명/리전 | `aws-config 1.8` + `aws-credential-types` | boto3 체인 |
| S3 (download) | `aws-sdk-s3 1.135` | boto3 S3 |
| SQL 분리 | `sqlparser 0.62` | sqlparse (분리·파괴성 판정만) |
| CLI 파싱 | `clap 4.6` (derive) | click |
| 테이블 출력 | `comfy-table 7.2` | cli_helpers |
| 설정 | `toml 1.1` + `serde 1.0` | configobj |
| 로깅 | `tracing` + `tracing-subscriber` | logging |
| 페이저 | `minus 5.7` | click pager |
| 확인 프롬프트 | `inquire 0.9` | click.prompt |
| 락프리 공유 | `arc-swap` | threading.Event |
| 에러 | `anyhow` + `thiserror` | — |
| 경로 | `directories` + `shellexpand` | os.path |
| 색상 | `nu-ansi-term` | pygments |

---

## 모듈 매핑 (Python → Rust)

```
athenacli-rs/
  Cargo.toml              # [workspace] members
  rust-toolchain.toml     # channel = "1.96.0"
  athenacli/              # bin
    src/main.rs           # 런타임 부트스트랩 + 첫 실행 설정 생성   ← main.py(cli)
    src/cli.rs            # clap Parser (click 옵션 미러)          ← main.py 옵션
    src/repl.rs           # 동기 루프: read_line→dispatch→render   ← main.py run_cli
  athenacli-core/         # lib
    src/exec.rs           # SqlExecute: client+Handle, run/tables  ← sqlexecute.py
    src/athena.rs         # start→poll→paginate→QueryRun 매핑      ← pyathena 내부
    src/config.rs         # serde 구조체 + 기본값 생성             ← config.py + athenaclirc
    src/parse/scanner.rs  # last_word/find_prev_keyword/extract_tables ← parseutils.py
    src/parse/destructive.rs # is_destructive                      ← parseutils
    src/completion/engine.rs    # suggest_type 디스패치           ← completion_engine.py
    src/completion/completer.rs # impl reedline::Completer         ← completer.py
    src/completion/refresher.rs # tokio task + ArcSwap            ← completion_refresher.py
    src/output/table.rs   # comfy-table + 세로(\G) + 1000행 경고   ← format_utils/tabular_output
    src/output/pager.rs   # minus + tee/once                       ← iocommands
    src/special/mod.rs    # 레지스트리 HashMap + arg_type 디스패치 ← special/main.py
    src/special/io.rs     # \e,pager,tee,timing,system,watch       ← special/iocommands.py
    src/special/db.rs     # \dt \l                                 ← special/dbcommands.py
    src/special/favorites.rs # \f \fs \fd (TOML 영속)              ← special/favoritequeries.py
    src/style/highlight.rs   # impl reedline::Highlighter (SQL)    ← lexer.py + clistyle.py
    src/style/keybindings.rs # emacs/vi, F2/F3/F4                  ← key_bindings.py
    src/prompt.rs         # impl reedline::Prompt: \d \r 치환      ← clibuffer.py + 프롬프트 템플릿
```
special 명령 등록: Python 데코레이터+전역 dict → `once_cell::Lazy<HashMap>` 또는 시작 시 `register()` 로 명시 구성 (14개 고정 → 매크로 마법 불필요).

---

## 단계별 마일스톤

### Phase 1 — MVP: 접속 + 실행 + 출력 + 기본 REPL
- clap 옵션 파싱 (`-e`, `-r`, `--profile`, `--s3-staging-dir`, `--work_group`, positional `catalog.database` 등)
- 자격증명 해석 (CLI 키 > 명명 profile > 기본 체인, 선택적 assume-role)
- `athena.rs`: StartQueryExecution → 200ms 폴링 → GetQueryResults 페이지네이션 → `QueryRun{headers,rows,output_location,scanned,ms}`
- `-e` 단발 실행 모드 (쿼리/파일/stdin)
- reedline 동기 REPL + FileBackedHistory + Ctrl-C/Ctrl-D
- comfy-table ASCII 출력 + `\G` 세로 출력 + >1000행 확인
- `;` 종료 멀티라인 (reedline Validator)
- TOML 설정 로드 + 첫 실행 기본 파일 생성
- tracing 파일 로그
- **크레이트**: clap, tokio, aws-config, aws-sdk-athena, reedline, comfy-table, toml, serde, sqlparser(분리만), tracing, anyhow, directories, shellexpand

### Phase 2 — 자동완성
- `parse/scanner.rs` (직접 작성 스캐너 — 아래 위험 참고) + `completion/engine.rs` suggest_type (FROM→테이블, WHERE/SELECT→스코프 컬럼, USE→DB)
- `impl Completer` + ColumnarMenu
- `exec.tables()/table_columns()/databases()` (`SHOW`/`information_schema` 쿼리)
- `refresher.rs`: `tokio::spawn` + `ArcSwap<Metadata>` + `Notify` 코얼레싱, use/create/drop 후 갱신
- **크레이트**: sqlparser(AST/키워드셋), arc-swap, reedline menu

### Phase 3 — special 명령 + 페이저 + 에디터 + 즐겨찾기
- `special/mod.rs` 레지스트리 + arg_type(NO_QUERY/PARSED/RAW) 디스패치
- `\e` ($EDITOR + tempfile), minus 페이저 + enable_pager, tee/notee/`\once`, `\timing`
- `\f`/`\fs`/`\fd` (TOML 영속), `read`, `system`(+`cd`), `watch`(-c 화면 클리어), `\dt`/`\l`
- `download` (aws-sdk-s3, output_location 의 s3://bucket/key 파싱)
- 파괴적 쿼리 확인 (inquire::Confirm)
- **크레이트**: minus, inquire, aws-sdk-s3, tempfile

### Phase 4 — 스타일/키바인딩/toolbar 마감
- `impl Highlighter` SQL 키워드 색상(nu-ansi-term), `[colors]` 테마
- `impl Prompt` 템플릿(`\d`,`\r`,날짜) + continuation
- emacs/vi EditMode, F2(완성 토글)/F3(멀티라인)/F4(vi-emacs) — ReedlineEvent
- 우측 프롬프트 상태 표시(toolbar 근사)
- **크레이트**: nu-ansi-term, crossterm

---

## 핵심 위험 / 주의 (lossy 가능 지점)

1. **Athena 폴링 상태머신 직접 구현** — pyathena 는 start→poll→fetch 를 DB-API cursor 뒤로 숨기지만 Rust 에선 직접 구현 (idempotency, 0.2s 폴링, Failed 사유 추출, next_token 페이지네이션). Phase 1 최대 작업.
2. **헤더 행 gotcha** — Athena `GetQueryResults` 는 SELECT/SHOW 에서 **첫 데이터 행이 컬럼명**. pyathena 는 숨기지만 Rust 는 직접 스킵 필요. 규칙: 첫 페이지 0번 행의 `var_char_value` 들이 메타데이터 컬럼명과 같으면 스킵. 문장 종류(첫 키워드)별로 pyathena 와 동일 휴리스틱 적용. 모든 값은 `Option<String>` 텍스트 → NULL vs 빈문자 구분은 `column_info` 타입 참고.
3. **자동완성 토크나이저 재구현** — Python 은 sqlparse 의 관대한(불완전 SQL 도 토큰화) flatten 에 의존. sqlparser 는 엄격해서 `SELECT x FROM t WHERE ` 같은 미완성 입력에 에러. → **직접 작성 스캐너(~150줄)** 로 "커서 앞 마지막 키워드"·"커서의 부분 단어" 검출. sqlparser 는 키워드셋만 사용. extract_tables(FROM 절 테이블/별칭) 가 가장 손이 많이 감 (서브쿼리·CTE·중첩 괄호 초기엔 sqlparse 보다 부정확할 수 있음).
4. **reedline ≠ prompt_toolkit** — 하단 toolbar 네이티브 미지원(우측 프롬프트로 근사), 완성 메뉴 스타일링 덜 풍부, F-key 런타임 토글 더 수동. 일부 마감은 근사.
5. **AWS SDK MSRV drift** — SDK 군 분기별 MSRV 상승 → 버전 핀 + `rust-version` 게이트 + 업데이트 시 검토.
6. **설정 마이그레이션** — 새 TOML 은 기존 INI 와 다름. 일회성 변환 스크립트 또는 매핑 문서 제공 (`[aws_profile <name>]`→`[aws_profile.<name>]`, bool/quoting 차이).
7. **watch + 페이저 + raw 모드 상호작용** — 쿼리 반복+화면 클리어와 페이저가 동시에 터미널을 잡음. reedline raw 모드 소유와의 순서 조율 까다로움 (Phase 3).

---

## 검증 방법

각 Phase 종료 시:
- **단위 테스트** (`athenacli-core`): scanner(last_word/prev_keyword/extract_tables), suggest_type 디스패치, config 파싱, 헤더행 스킵 로직, is_destructive — Python 테스트 케이스(`athenacli/test/`) 를 Rust 케이스로 이식.
- **통합/수동 테스트**: 실제 Athena 연결로 `SELECT 1`, `SHOW DATABASES`, `SHOW TABLES`, 멀티라인 쿼리, `-e` 단발 실행. 동일 쿼리를 Python `athenacli` 와 나란히 돌려 **출력·헤더·상태줄 1:1 비교** (별도 repo 라 양쪽 동시 보유 가능).
- **Phase 2**: FROM 뒤 Tab→테이블, WHERE 뒤 Tab→컬럼, USE 뒤→DB 목록 확인. 백그라운드 갱신 후 새 테이블 반영 확인.
- **Phase 3**: 각 special 명령 1개씩 수동 실행 (`\e`,`\f`,`tee`,`watch`,`download`).
- **회귀**: `cargo clippy -- -D warnings`, `cargo fmt --check`, CI(GitHub Actions)에서 1.96.0 toolchain 빌드.

### 첫 착수 항목 (Phase 1 시작점)
1. workspace 골격 (`Cargo.toml` + `athenacli/`, `athenacli-core/`) + `rust-toolchain.toml` (1.96.0)
2. `cli.rs` clap 구조체 (main.py 옵션 미러)
3. `athena.rs` 쿼리 라이프사이클 + 헤더행 처리 (위험 #1,#2 — 여기가 핵심)
4. `exec.rs` 동기 `run()` 래퍼 (block_on 브리지)
5. `repl.rs` 최소 루프 + comfy-table 출력
