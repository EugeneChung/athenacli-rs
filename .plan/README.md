# athenacli-rs 플랜 인덱스

athenacli(Python) → Rust 이식 플랜 모음. 현재 repo 는 빈 상태(커밋 없음)이므로 Phase 1 이 첫 코드 지점이다.

## 문서

- `rust-migration-plan.md` — 상위(마스터) 플랜. 왜 하는가, 아키텍처 핵심 결정, 크레이트 선택, 모듈 매핑, 위험 목록.
- `phase-1-mvp.md` — 접속 + 실행 + 출력 + 기본 REPL. async SDK↔동기 reedline 다리, Athena 쿼리 라이프사이클, 헤더행 처리.
- `phase-2-completion.md` — 자동완성. 직접 작성 스캐너, suggest_type 디스패치, ArcSwap 백그라운드 메타데이터 갱신.
- `phase-3-special.md` — special 명령 + 페이저 + 에디터 + 즐겨찾기 + 다운로드 + 파괴적 쿼리 확인.
- `phase-4-style.md` — 구문 강조 + 프롬프트 템플릿 + 키바인딩 + toolbar 근사(마감).

## 진행 방식

각 Phase 는 앞 Phase 의 동작 확인 후 착수(단계적 MVP). 각 Phase 종료 시 같은 쿼리를 Python `athenacli`(`/Users/euigeun/project/opensources/athenacli`)와 나란히 돌려 출력·헤더·상태줄을 1:1 비교한다.

## 공통 게이트
`cargo build`(1.96.0), `cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo test`.
