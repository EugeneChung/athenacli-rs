# Phase 2 실행 플랜 — 자동완성

상위 플랜: `rust-migration-plan.md` 의 "Phase 2". 선행: `phase-1-mvp.md` 완료(접속·실행·출력 동작).

## 목표

커서 위치의 문맥을 보고 후보를 제안하는 자동완성. `FROM` 뒤에는 테이블, `WHERE`/`SELECT` 뒤에는 그 쿼리의 FROM 절 테이블 컬럼, `USE` 뒤에는 데이터베이스 목록. 메타데이터(테이블·컬럼·DB 이름)는 백그라운드에서 미리 받아두고 키 입력마다 락 없이 읽는다.

## 완료 정의

- `SELECT * FROM ` + Tab → 테이블 목록.
- `SELECT  FROM users` 의 `SELECT ` 위치 + Tab → `users` 의 컬럼.
- `USE ` + Tab → 데이터베이스 목록.
- `SELECT foo FROM bar LEFT ` + Tab → `OUTER JOIN`/`JOIN` 키워드.
- `CREATE TABLE` 후 백그라운드 갱신이 돌고, 새 테이블이 다음 완성에 반영.
- 이식한 단위테스트(parseutils / completion_engine / naive_completion) 통과.

## 선행 조건

- Phase 1 의 `SqlExecute` 가 살아 있고 `block_on` 브리지로 쿼리를 돌릴 수 있어야 함.
- reedline 루프가 동작 중(여기에 Completer/Menu 를 끼운다).

## 작업 순서

### 1. 직접 작성 스캐너 — `athenacli-core/src/parse/scanner.rs` (위험 #3)
산출물: Python `parseutils.py` 의 토큰 유틸을 손으로 옮긴 ~150줄 스캐너. **sqlparse 대체**(sqlparser 의 엄격 파서는 미완성 SQL 에 에러나므로 직접 구현).

- 정규식 4종(Python `cleanup_regex` 미러):
  - `alphanum_underscore` = `(\w+)$`
  - `many_punctuations` = `([^():,\s]+)$`
  - `most_punctuations` = `([^\.():,\s]+)$`
  - `all_punctuations` = `([^\s]+)$`
- `last_word(text, kind) -> &str`: 커서 앞 마지막 단어. Python 의 doctest 들을 그대로 단위테스트로 이식(`'abc def'`→`'def'`, `'abc def '`→`''`, `'bac $def'`→`'def'`, `most_punctuations` 변형 등).
- `find_prev_keyword(text) -> (Option<String>, &str)`: 커서 앞에서 가장 가까운 SQL 키워드 또는 여는 괄호 `(` 를 찾고, 그 앞까지의 텍스트를 함께 반환(컬럼 스코프 재귀에 쓰임).
- `extract_tables(full_text) -> Vec<(Option<String>, String, Option<String>)>` = `(schema, table, alias)`: FROM/JOIN 절의 식별자·별칭 추출. **가장 손이 많이 가는 부분**. 초기 범위: 콤마 구분 테이블 + 단순 JOIN + `schema.table` + `table alias`/`table AS alias`. 서브쿼리·CTE·중첩 괄호는 초기엔 sqlparse 보다 부정확할 수 있음(상위 플랜 위험 #3 명시) → 테스트로 한계 표시.
- 보조: `is_open_quote(text)`, `parse_partial_identifier(word) -> (schema, partial)`.
- 키워드/예약어 셋: `sqlparser` 의 키워드 + Python `literals`(keywords/functions) 를 합쳐 상수 테이블로(`literals/main.py` 의 keywords·functions 이식).

### 2. suggest_type 디스패치 — `athenacli-core/src/completion/engine.rs`
산출물: Python `completion_engine.py::suggest_type` 의 충실한 포트. 구조를 Python 과 평행하게 유지(테스트 이식 용이).

- 제안 타입 enum:
  - `Column { tables: Vec<(Option<String>,String,Option<String>)>, drop_unique: bool }`
  - `Function { schema: Option<String> }`, `Table { schema: Option<String> }`, `View { schema: Option<String> }`
  - `Alias { aliases: Vec<String> }`, `Database`, `Schema`
  - `Keyword { last_token: Option<String> }`, `Special`, `Show`, `TableFormat`, `FileName`, `FavoriteQuery`
- `suggest_type(full_text, text_before_cursor) -> Vec<Suggestion>` 분기:
  - special 명령 프리픽스: `\u`→`Database`; `\dt`/`\dt+`→`Table+View+Schema`; `\T`→`TableFormat`; `\f`→`FavoriteQuery`; `\.`/`source`→`FileName`.
  - 마지막 토큰/직전 키워드로 디스패치(`suggest_based_on_last_token` 포트):
    - `from`/`join`(및 `…join`)/`into`/`update`/`describe`/`desc`/`truncate`/`copy`/`explain`/`partitions` → `Table+View+Schema`(parent 있으면 schema 한정).
    - `select`/`where`/`having` → parent 있으면 `Column(한정)+Table+View+Function`; 없으면 `Column(extract_tables 스코프)+Function+Alias+Keyword`.
    - `on`/`and`/`or`/`set`/`by`/`distinct` → `Column(extract_tables)`.
    - `as` → 빈 제안(별칭 자리).
    - 여는 괄호 `(` 뒤(WHERE 문맥) → 컬럼/함수; `USING(` → `Column{drop_unique:true}`(둘 이상 테이블에 공통인 컬럼만).
    - 기본 → `Keyword + Special`.
- Comparison(`a.id = d.`) 처리: 마지막 식별자 parent 추출해 그 테이블 컬럼 한정(Python 의 Comparison 분기 포트).

### 3. Completer 구현 — `athenacli-core/src/completion/completer.rs`
산출물: `impl reedline::Completer`.

- reedline `Completer::complete(&mut self, line, pos) -> Vec<reedline::Suggestion>`. 내부에서 `suggest_type(line, &line[..pos])` 호출.
- `word_before_cursor` = `last_word(&line[..pos])`.
- 각 제안 타입 → 매처가 메타데이터에서 후보 생성:
  - `Table/View/Database/Schema/Function` → `ArcSwap<Metadata>` 에서 로드한 목록.
  - `Keyword`/`Special`/`TableFormat`/`FavoriteQuery` → 정적/설정 목록.
  - `Column` → `extract_tables` 스코프의 컬럼(드롭 unique 시 2개 이상 테이블 공통만 — Python `Counter` 로직 포트).
  - `Alias` → FROM 절 별칭.
- `find_matches(word, collection, fuzzy, casing)`: Python `find_matches` 포트 — 퍼지 매칭 + `keyword_casing`(upper/lower/auto) 적용. 키워드 케이싱 규칙(현재 입력의 대소문자로 auto 판정)도 이식.
- reedline `Span`(치환 범위) 계산: `word_before_cursor` 길이만큼.

### 4. 메타데이터 모델 + 조회 — `athenacli-core/src/completion/metadata.rs` + `exec` 메서드
산출물: 완성에 쓸 스키마 캐시 + 조회 쿼리.

- `Metadata { databases: Vec<String>, tables: HashMap<String, Vec<String>>, columns: HashMap<(String,String), Vec<String>>, functions: Vec<String> }`.
- `exec` 메서드(Phase 1 의 시그니처 stub 채움):
  - `databases()` = `SHOW DATABASES`.
  - `tables()` = `SHOW TABLES`.
  - `table_columns(db)` = `SELECT table_name, column_name FROM information_schema.columns WHERE table_schema = '<db>' ORDER BY table_name, ordinal_position`.
- 결과 행을 `Metadata` 로 적재하는 빌더.

### 5. 백그라운드 갱신 — `athenacli-core/src/completion/refresher.rs` (위험 #4 동시성)
산출물: `tokio::spawn` + `ArcSwap` + `Notify` 코얼레싱. Python `CompletionRefresher`(스레드 + restart Event) 대체.

- `Refresher { handle: Handle, current: Arc<ArcSwap<Metadata>>, notify: Arc<Notify>, running: Arc<AtomicBool> }`.
- `refresh()`:
  - 이미 갱신 중(`running` true)이면 재시작 신호만(`notify.notify_one()`) 주고 리턴(Python `_restart_refresh.set()` 미러).
  - 아니면 백그라운드 task spawn: 메타데이터 조회 → 완성 후 `current.store(Arc::new(new_meta))`(원자적 교체). completer 는 매 키 입력마다 `current.load()` 로 락 없이 읽음(상위 플랜 결정 #2 — RwLock 대신 ArcSwap 채택 이유: 쓰기 중 키 입력 블로킹 방지).
- `need_completion_refresh(sql) -> bool`: `use`/`create`/`drop` 류 후 true(Python `need_completion_refresh` 포트). repl 이 쿼리 성공 후 호출해 `refresh()` 트리거.
- 시작 시 1회 초기 갱신을 spawn.

### 6. REPL 배선
- reedline 빌더에 `.with_completer(Box::new(completer))`, `.with_menu(ReedlineMenu::EngineCompleter(Box::new(ColumnarMenu::default())))`.
- Tab 키 이벤트: 메뉴 표시/다음 후보(Python F-key 와 별개, 기본 Tab 완성).
- `main`/`repl` 시작 시 `refresher.refresh()` 1회.

## 테스트

### 단위 (Python 테스트 이식)
- `test_parseutils.py` → `last_word` doctest 전부, `extract_tables` 케이스(단순/별칭/스키마한정/JOIN; 서브쿼리·CTE 는 한계 케이스로 표시).
- `test_completion_engine.py` → `suggest_type` 케이스: `SELECT  FROM tabl`→Column/Function/Alias/Keyword, `SELECT  FROM sch.tabl`→스키마 한정, JOIN 스코프, `\u `→Database, `\dt`/`\dt+`→Table/View/Schema.
- `test_naive_completion.py` → 컬럼/조인 완성 결과 집합, `INNER/OUTER/CROSS/LEFT/RIGHT/FULL ` 뒤 `JOIN`, `LEFT/RIGHT/FULL ` 뒤 `OUTER JOIN`.
- `find_matches` 퍼지/케이싱.

### 수동
- FROM 뒤 Tab→테이블, WHERE 뒤 Tab→스코프 컬럼, USE 뒤 Tab→DB.
- `CREATE TABLE` 후 백그라운드 갱신 → 새 테이블이 다음 Tab 에 등장.

## 이 Phase 의 위험과 대응

- **위험 #3 토크나이저 재구현**: 작업 1·2 가 핵심. Python 테스트를 그대로 이식해 동치성으로 검증. `extract_tables` 의 서브쿼리/CTE 부정확은 테스트에 명시적 한계로 남기고 점진 개선.
- **위험 #4 reedline ≠ prompt_toolkit**: 완성 메뉴 스타일링이 덜 풍부 → `ColumnarMenu` 로 근사. 키 입력 중 네트워크 금지(완성은 동기) → 메타데이터는 ArcSwap 선적재로만 읽음(작업 5).

## 사용 크레이트
sqlparser(키워드셋/토크나이저), arc-swap, reedline(menu), tokio(spawn/Notify), 기존 Phase 1 크레이트.

## 다음 단계
Phase 2 수동 검증 후 `phase-3-special.md` 착수.
