# Phase 4 실행 플랜 — 스타일 / 키바인딩 / 프롬프트 / toolbar 마감

상위 플랜: `rust-migration-plan.md` 의 "Phase 4". 선행: Phase 1~3 완료. 이 Phase 는 "기능"보다 "마감"으로, reedline 이 prompt_toolkit 과 다른 부분은 근사로 처리한다(상위 플랜 위험 #4).

## 목표

입력 SQL 구문 색상, 프롬프트 템플릿 치환(`\d`/`\r`/날짜), emacs/vi 편집 모드와 F2/F3/F4 토글, 하단 상태 표시(toolbar 근사)를 올려 사용 경험을 Python 판에 가깝게 맞춘다.

## 완료 정의

- 입력 중 SQL 키워드가 색상으로 강조된다(`[colors]` 테마 반영).
- 프롬프트가 `database@region` 과 날짜/시간 토큰을 정확히 치환한다. 프롬프트가 너무 길면 짧은 형식으로 폴백한다.
- 멀티라인 continuation 프롬프트(`-> `)가 보인다.
- emacs/vi 모드 전환(F4), 완성 토글(F2), 멀티라인 토글(F3)이 동작한다.
- 모드/상태가 우측 프롬프트(또는 근사 위치)에 표시된다.

## 작업 순서

### 1. 구문 강조 — `athenacli-core/src/style/highlight.rs`
산출물: `impl reedline::Highlighter`. Python `lexer.py` + `clistyle.py` 대체.

- 키워드 셋: sqlparser 키워드 + Python `lexer.py` 가 추가한 `repair`/`offset` 등 보강 키워드. Phase 2 의 키워드 상수 재사용.
- 입력 라인을 토큰 단위로 훑어 키워드/문자열/숫자/식별자에 색을 입힘(`nu-ansi-term`).
- 색상 매핑은 설정 `[colors]` 테이블에서 읽어 테마화(Python `clistyle.style_factory` 의 색 클래스 → reedline 스타일로 매핑).
- reedline `Highlighter::highlight(&self, line, cursor) -> StyledText`.

### 2. 프롬프트 — `athenacli-core/src/prompt.rs`
산출물: `impl reedline::Prompt`. Python `get_prompt` + `clibuffer.py` 대체.

- 템플릿 치환(Python `get_prompt` 미러):
  - `\d` → 현재 database(없으면 `(none)`)
  - `\r` → region(없으면 `(none)`)
  - `\D` → 전체 날짜, `\m` 분, `\s` 초, `\P` AM/PM, `\R` 24시각, `\n` 개행
  - 기본 프롬프트 `\d@\r> `, 연속 프롬프트 `-> `.
- `MAX_LEN_PROMPT`(45) 초과 시 짧은 형식 `\r:\d> ` 로 폴백(Python `get_message` 미러).
- reedline `Prompt` 트레이트: `render_prompt_left/right/indicator/multiline_indicator`. 날짜/시간은 매 렌더 시점 계산(주의: 워크플로 샌드박스가 아닌 실제 런타임이므로 `chrono`/`time` 사용 가능).
- region/database 는 `SqlExecute` 의 현재 상태를 참조(공유 핸들 또는 Arc).

### 3. 키바인딩/편집 모드 — `athenacli-core/src/style/keybindings.rs`
산출물: emacs/vi + F2/F3/F4. Python `key_bindings.py` 대체.

- 편집 모드: 설정 `key_bindings` 가 `vi` 면 reedline `Vi`, 아니면 `Emacs`.
- F4: vi↔emacs 토글(Python `@kb.add('f4')` 미러). reedline 은 런타임 편집모드 교체가 prompt_toolkit 보다 수동 → `Reedline::with_edit_mode` 재구성 또는 커스텀 이벤트로 처리.
- F2: 스마트 완성 on/off 토글(completer 의 `smart_completion` 플래그).
- F3: 멀티라인 on/off 토글(Validator 활성 여부).
- Tab: 강제 완성(메뉴 없으면 표시, 있으면 다음 후보) — Python `@kb.add('tab')` 미러. Ctrl-Space 도 동일 계열.
- reedline 은 `ReedlineEvent` + `Keybindings` 로 구성. F-key 런타임 토글은 prompt_toolkit 보다 손이 더 감(위험 #4).

### 4. toolbar 근사 — 우측 프롬프트 상태
산출물: 하단 toolbar 대체.

- reedline 은 prompt_toolkit 의 bottom toolbar 를 네이티브로 지원하지 않음 → `render_prompt_right` 로 모드/상태(예: `vi`/`emacs`, 완성 on/off, 멀티라인) 근사 표시.
- Python `clitoolbar.py` 의 표시 항목 중 핵심만 추려 우측에.

## 테스트

- **수동 위주**(시각적 마감이라 자동화 가치 낮음):
  - 키워드 색상이 입력 중 즉시 반영.
  - 프롬프트가 `db@region` + 날짜 토큰 정확 치환, 긴 프롬프트 폴백.
  - 멀티라인 입력 시 `-> ` continuation.
  - F2/F3/F4 토글 동작, vi/emacs 전환.
- **단위(가능 범위)**: 프롬프트 템플릿 치환 함수(`\d`/`\r`/날짜 토큰)는 시각 출력과 분리해 순수 함수로 두고 입력→출력 문자열 테스트.

## 이 Phase 의 위험과 대응

- **위험 #4 reedline ≠ prompt_toolkit**: 하단 toolbar 미지원(우측 프롬프트 근사), 완성 메뉴 스타일 제약, F-key 런타임 토글 수동. 1:1 재현이 아니라 **근사**가 목표임을 명시하고, 핵심 사용성(색상·프롬프트·모드전환)부터 확보.

## 사용 크레이트
nu-ansi-term(색상), crossterm(키/터미널), chrono 또는 time(날짜 토큰), 기존 크레이트.

## 전체 마무리(Phase 4 종료 ≈ 1차 이식 완료)

- 회귀 게이트: `cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo test`.
- CI(GitHub Actions): Rust 1.96.0 toolchain 으로 build + clippy + fmt + test.
- 사용자 문서: 새 TOML 설정 설명 + 기존 INI→TOML 마이그레이션 안내(상위 플랜 위험 #6). 변환 스크립트 제공 여부 결정.
- Python 판과의 최종 출력 대조(Phase별 누적 검증의 종합).
