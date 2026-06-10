# Phase 3 실행 플랜 — special 명령 + 페이저 + 에디터 + 즐겨찾기 + 다운로드

상위 플랜: `rust-migration-plan.md` 의 "Phase 3". 선행: Phase 1·2 완료.

## 목표

백슬래시/이름 기반 명령(special command)과 그 주변 기능(페이저, tee, 외부 에디터, 즐겨찾기, watch, system, download)을 올린다. 파괴적 쿼리 실행 전 확인도 여기서 붙인다.

## 완료 정의

- 각 special 명령 1개씩 수동 실행 성공: `\e`, `\f`/`\fs`/`\fd`, `tee`/`notee`, `\once`, `\timing`, `system`(+`cd`), `watch`, `\dt`/`\l`, `pager`, `download`.
- `DROP`/`DELETE`/`TRUNCATE` 류 입력 시 실행 전 확인 프롬프트가 뜨고, 거절하면 실행 안 됨.
- 즐겨찾기 저장/목록/삭제가 TOML 설정에 영속화되고 재시작 후에도 보임.
- 이식한 special 단위테스트(test_dbspecial) 통과.

## 선행 조건

- Phase 1 의 `exec.run` 이 정규 SQL 을 돌린다(여기에 special 디스패치 훅을 추가).
- 설정 TOML 에 `favorite_queries` 테이블이 있다(Phase 1 config 구조체).

## special 명령 목록 (Python `@special_command` 기준)

전체 목록의 권위 소스는 Python `packages/special/*.py` 의 `@special_command` 데코레이터다. 확인된 명령:

| 명령 | 단축 | arg_type | 출처 파일 |
|---|---|---|---|
| `help`(`\?`, `?`) | `\?` | NO_QUERY | special/main.py |
| `exit`(`\q`) / `quit` | `\q` | NO_QUERY | special/main.py |
| `use` | `\u` | PARSED | main.py(앱 등록) |
| `prompt` | `\R` | PARSED | main.py |
| `tableformat` | `\T` | PARSED | main.py |
| `\dt [table]` | — | PARSED | dbcommands |
| `\l` | — | RAW | dbcommands |
| `pager` | `\P` | PARSED | iocommands |
| `nopager` | `\n` | NO_QUERY | iocommands |
| `tee [-o] file` | — | PARSED | iocommands |
| `notee` | — | PARSED | iocommands |
| `\once`(`\o`) | — | PARSED | iocommands |
| `\timing`(`\t`) | — | NO_QUERY | iocommands |
| `system [cmd]` | — | PARSED | iocommands |
| `watch [s] [-c] q` | — | PARSED | iocommands |
| `read [file]` | — | PARSED | iocommands |
| `\e` 에디터 | — | (편집 경로) | iocommands |
| `\f`/`\fs`/`\fd` | — | PARSED | iocommands(favorites) |
| `download` | — | NO_QUERY | iocommands |

> (구현 시 확정) `@special_command` 전수 grep 결과를 위 표에 반영했다. `notee`/`watch` 는 Python 코드상 기본값(PARSED)이고, `read`/`help`/`exit`/`nopager` 가 초안에서 빠져 있었다.

## 작업 순서

### 1. 레지스트리 + 디스패치 — `athenacli-core/src/special/mod.rs`
산출물: 명령 등록/탐색/디스패치 코어. Python `special/main.py` 미러.

- `enum ArgType { NoQuery, ParsedQuery, RawQuery }` (Python 0/1/2).
- `struct SpecialCommand { handler, name, shortcut, description, arg_type, hidden, case_sensitive }`.
- 레지스트리: Python 은 데코레이터+전역 dict. Rust 는 14개 고정이므로 **시작 시 명시적 `register()`** 로 `HashMap<String, SpecialCommand>` 구성(매크로 마법 불필요 — 상위 플랜 명시). `case_sensitive` 면 원형 키, 아니면 소문자 키.
- `parse_special_command(sql) -> (command, verbose, arg)`: Python `partition(' ')` + `+` 검출(`verbose`) 미러.
- 디스패치: 명령 탐색 후 `arg_type` 에 따라:
  - `ParsedQuery`/`RawQuery` → handler 에 `(cur/exec, arg)` 전달.
  - `NoQuery` → handler 에 인자 없이.
  - 핸들러는 `Vec<ResultSet>` 반환(또는 watch 처럼 yield 스트림).
- 핸들러 시그니처: `fn(ctx: &mut SpecialCtx, arg: &str) -> Result<Vec<ResultSet>>`. `SpecialCtx` 는 `exec`, `config`, 페이저/tee/timing 상태, `output_location` 을 보유(Python 의 전역 상태 + cursor 를 구조체로 대체).
- `exec.run` 훅: Python `sqlexecute.run` 처럼 먼저 `special::execute` 시도 → `CommandNotFound` 면 정규 SQL. 이 분기를 Phase 1 의 `exec.run` 에 삽입.

### 2. IO 명령 — `athenacli-core/src/special/io.rs`
산출물: 에디터/페이저/tee/once/timing/system/watch.

- `\e` 외부 에디터: `$VISUAL`/`$EDITOR`(기본 `vi`) + tempfile(`.sql` 접미사, `tempfile` 크레이트). Python `handle_editor_command` 의 while 루프(여러 번 편집) 포트 — 마지막 쿼리를 기본값으로 열고, 편집 결과를 다시 프롬프트 기본값으로. (구현: reedline 의 공개 API `run_edit_commands([Clear, InsertString])` 로 다음 `read_line` 버퍼를 시딩 — 제출 시에만 버퍼가 초기화되므로 Python 의 `prompt(default=sql)` 와 동일하게 동작.)
- `pager`(`\P [cmd]`): 인자 있으면 `$PAGER` 설정 + 활성화, 없으면 현재 PAGER 표시. 출력은 외부 `$PAGER` 프로세스로(작업 5).
- `tee [-o] file` / `notee`: 파일 append(기본)/overwrite(`-o`). 출력 라인을 tee 파일에도 기록. Python `parseargfile` 포트.
- `\once`(`\o`): 다음 결과 1회를 파일로. 기록 후 플래그 해제(Python `unset_once_if_written`).
- `\timing`(`\t`): TIMING 토글.
- `system [cmd]`: 셸 명령 실행. `cd` 로 시작하면 `handle_cd_command`(프로세스 cwd 변경) — Python `utils.handle_cd_command` 포트.
- `watch [seconds] [-c] query`: 쿼리를 주기 반복(기본 5초), `-c` 면 매 반복 화면 클리어. 반복 중 페이저 비활성(Python `set_pager_enabled(False)` 후 finally 복구), Ctrl-C 로 중단. 인자 파싱(seconds/-c/statement)도 Python 포트.

### 3. DB 명령 — `athenacli-core/src/special/db.rs`
산출물: `\dt`, `\l`. Python `dbcommands.py` 미러.

- `\dt [table]`(PARSED): 인자 있으면 `SHOW COLUMNS FROM <table>`, 없으면 `SHOW TABLES`. 헤더 유무 처리.
- `\l`(RAW): `SHOW DATABASES`.

### 4. 즐겨찾기 — `athenacli-core/src/special/favorites.rs`
산출물: `\f`/`\fs`/`\fd` + TOML 영속. Python `favoritequeries.py` 미러(섹션명 `favorite_queries`).

- `\fs name query`: 저장. name·query 둘 다 필요, 누락 시 usage. 저장 후 설정 파일에 write-back.
- `\f [name]`: 인자 없으면 전체 목록 표, 인자 있으면 해당 이름의 쿼리를 실행(치환 후 정규 실행 경로로).
- `\fd name`: 삭제. 없으면 `name: Not Found.`.
- 저장소: Python 은 configobj 섹션에 직접 write. Rust 는 `Config.favorite_queries: HashMap` 수정 후 athenaclirc(TOML) 재직렬화 저장.

### 5. 페이저 + tee/once 출력 — `athenacli-core/src/output/pager.rs`
산출물: 외부 `$PAGER` 프로세스 통합 + tee/once 라이터.

- (변경) `minus` 대신 **외부 `$PAGER` 서브프로세스**(`sh -c $PAGER`, 기본 `less -R`)를 띄운다. 이유: ① `pager <cmd>` 명령의 의미가 "그 외부 명령으로 출력"이라 내장 페이저로는 재현 불가(Python `click.echo_via_pager` 도 외부 프로세스), ② 내장 페이저가 직접 raw 모드를 잡으면 reedline 과 충돌(위험 #7) — 외부 프로세스는 자기 tty 제어를 스스로 정리한다.
- 출력 분기(Python `output()` 미러): 화면에 맞으면 stdout, 안 맞고 페이저 활성이면 페이저. 마진 계산(`get_reserved_space` min(높이*0.45, 8) + 프롬프트/상태/timing 줄) 포트.
- 모든 출력 라인은 tee/once 파일에도 기록(상태줄은 제외 — Python 동일).
- watch + 페이저 동시 사용 시 페이저 비활성으로 회피(Python 동일).

### 6. 다운로드 — `athenacli-core/src/special/download.rs`
산출물: `download`(NO_QUERY).

- 마지막 쿼리의 `OUTPUT_LOCATION`(`s3://bucket/key`)을 파싱.
- Python 은 `aws s3 cp` 셸아웃이지만, Rust 는 `aws-sdk-s3` 의 `get_object` 로 받아 `/tmp/` 에 저장(셸 의존 제거).
- `OUTPUT_LOCATION` 없으면 `No OUTPUT_LOCATION from last query`.

### 7. 파괴적 쿼리 확인
산출물: 실행 전 확인.

- `is_destructive(sql)` 포트: Python `parseutils.is_destructive` 의 실제 키워드는 **`drop`/`shutdown`/`delete`/`truncate` 4개뿐**(초안의 alter/create/insert 등은 과대 목록이었음). `queries_start_with` 미러: 문장 분리 후 주석을 건너뛴 첫 단어 비교. 멀티 문장이면 하나라도 파괴적이면 true.
- repl 에서 정규 실행 전 `config.destructive_warning` 켜져 있으면 `inquire::Confirm`. 거절 시 중단(Python `confirm_destructive_query` 흐름 미러: None=비파괴 통과, True=실행, False=중단).

## 테스트

### 단위 (Python `test_dbspecial.py` 이식)
- `parse_special_command`: `(command, verbose, arg)` 분해(공백/`+` 케이스).
- 레지스트리 등록/탐색(case_sensitive 분기, alias).
- 즐겨찾기 `$1..$N` 치환(과잉/누락 인자 에러 메시지 포함).
- `is_destructive` 멀티 문장 + 주석 스킵.
- `parseargfile`(`-o` 분기), watch 인자 파싱, `\e` 헬퍼(detect/filename/query), S3 URL 파싱, 페이저 마진 계산.
- (제외) `format_uptime`: Python 쪽에서도 호출처가 없는 죽은 유틸이라 이식하지 않음. `\u`/`\dt` suggest 테스트는 Phase 2 `completion/engine.rs` 에 이미 존재.

### 수동
- 표의 각 명령 1회씩 실행. 특히 `\e`(에디터 왕복), `watch -c`(클리어), `tee`+이후 쿼리(파일 누적), `download`(파일 생성), 파괴적 쿼리 거절.

## 이 Phase 의 위험과 대응

- **위험 #7 watch + 페이저 + raw 모드**: 작업 5·2. watch 중 페이저 강제 비활성으로 충돌 회피, 페이저는 외부 프로세스라 reedline raw 모드와 분리(read_line 밖에서만 실행). 수동 테스트로 터미널 깨짐 확인.
- **Ctrl-C**: reedline 이 읽는 동안은 키 이벤트, 실행 중에는 SIGINT. REPL 시작 시 tokio `ctrl_c` 리스너가 전역 cancel 플래그를 세팅 → watch 의 sleep 슬라이스와 Athena 폴링 루프가 플래그를 확인(폴링 중이면 `StopQueryExecution` 후 조용히 복귀 — Python pyathena 의 KeyboardInterrupt cancel 미러). `-e` 모드는 리스너를 안 달아 기본 종료 동작 유지.

## 사용 크레이트
inquire(확인), aws-sdk-s3(download), tempfile(에디터), crossterm(터미널 크기), shlex(`\f` 인자 분리), 기존 크레이트. (`minus` 는 외부 페이저 방식 채택으로 불필요.)

## 다음 단계
Phase 3 수동 검증 후 `phase-4-style.md` 착수.
