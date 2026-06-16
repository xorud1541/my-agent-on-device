# 로컬 검색 RAG 포팅 — 설계 / 핸드오프

날짜: 2026-06-14
브랜치: `feat/local-search-rag` (베이스: `docs/macos-cross-platform-port`)
상태: 설계만 (코드 미적용).
대상 독자: 로컬 검색 기능을 실제로 붙일 다음 에이전트/개발자.

> ⚠️ 이 문서는 **분석과 제안**만 담는다. 코드는 한 줄도 바꾸지 않았다.
> 갭 분석은 `2026-06-14-spec-vs-implementation-gap.md` 참조.

---

## 1. 결론 요약

- 로컬 검색은 **`team-util/LocalSearch`** 프로젝트를 가져다 쓴다. alian이 이미
  이걸 **사이드카(별도 프로세스) HTTP 바인딩**으로 붙여 검증했다.
- 우리도 **사이드카 방식 추천**. 우리는 이미 (a) llama-server를 사이드카로 띄우고,
  (b) `ort` load-dynamic + `onnxruntime` 벤더링이 되어 있어 인프라 궁합이 좋다.
- 라우팅은 **하드 3분류(로컬서치/일반대화/도구)를 만들지 않는다.** 도구 분기는 현행
  `tool_choice:auto` 그대로 두고, **대화 턴에 RAG 프리훅 + 임계값 게이트**로
  "로컬서치 vs 일반대화"를 자동으로 가른다.
- 일반대화는 정책상 Alice가 일반 챗봇이 아니므로 **범위 한정 페르소나 + 가벼운 시도**
  하이브리드로 처리(별도 분류기 없이 시스템 프롬프트).

---

## 2. LocalSearch 정체 (team-util/LocalSearch, `feature/cli` 브랜치)

Rust 오프라인 의미 검색 엔진. lib `local_search` + 바이너리 `localsearch-cli`.

| 요소 | 내용 |
|---|---|
| 임베딩 | **Harrier v1-270m**(텍스트) ONNX + `tokenizers`, CLIP-b32(이미지). `ort 2.0.0-rc.12` load-dynamic |
| 색인 DB | **SQLite**(`rusqlite`, bundled). 벡터 BLOB 저장, **코사인 유사도 브루트포스 스캔**(HNSW 아님) + FTS5/BM25 어휘 매칭 |
| 청킹 | `text-splitter` crate, **300~500자 / 50자 오버랩**, md·csv·plain + 문서타입 인지(법령/표/회의록) |
| PDF 파싱 | `pdfium-render` (pdfium.dll/dylib 필요) |
| 인터페이스 | `localsearch-cli`: `search` / `index` / `status` / `serve --port`(axum HTTP) |
| 네이티브 의존 | `onnxruntime` + `pdfium` (플랫폼별) |
| 지원 확장자 | 문서 `pdf docx txt md csv` (+ hwp/hwpx 브랜치), 이미지 `png jpg jpeg webp bmp gif` |

### alian의 바인딩 방식 (참고 레퍼런스)

- 빌드된 `localsearch-cli` 바이너리를 vendor에 넣고 `serve --port 11234`로 스폰 →
  `reqwest`로 `/api/status`·`/api/search` 호출.
- 라우팅은 **Chat/Tool 2분류**만. "로컬 검색"은 별도 카테고리가 아니라
  Chat 경로의 **RAG 프리훅**(`try_localsearch_context`)이 매 턴 검색 → top-1 ≥ 0.65면
  청크를 시스템 프롬프트에 주입.
- 임계값: chat threshold **0.65**, top_k **3** (백엔드 noise floor 0.04는 별개).

---

## 3. 우리 프로젝트 이식 방식

| 방식 | 내용 | 평가 |
|---|---|---|
| **A. 사이드카 (alian과 동일)** | `localsearch-cli` 바이너리 vendor + HTTP 호출 | ✅ **추천**. llama-server와 같은 패턴, 프로세스 격리로 ort 버전 충돌 무관 |
| B. 크레이트 링크(인프로세스) | `local-search`를 의존성으로 컴파일 | ⚠️ 비추천. ort를 2.0.0-rc.12에 강제 정렬해야 하고 pdfium/tokenizers/rusqlite/text-splitter까지 빌드에 끌려옴 |

**비용/리스크**

- 추가 배포물: `localsearch-cli`(~20MB) + Harrier 모델(quantized, 수백 MB) + `pdfium`.
  설치 파일 크기 증가가 가장 큰 실질 비용. → 모델은 기존 fetch-deps 패처 재활용 가능.
- 우리 `ort`(removeBG)와 LocalSearch `ort`는 **별 프로세스라 충돌 없음** (A의 결정적 장점).
- **크로스플랫폼**: LocalSearch는 플랫폼별 `localsearch-cli` 바이너리 + `onnxruntime`·`pdfium`
  네이티브 라이브러리가 필요하다. 베이스가 `docs/macos-cross-platform-port`인 이유가 여기에 있다.
  Windows는 `.exe`, macOS는 `dylib` 링크/번들 분기가 필요(alian은 Windows .exe만 검증).

---

## 4. 라우팅 설계 — "2분류 + RAG 프리훅"

### 4.1 왜 하드 3분류를 안 만드나

- 우리는 분류기가 **아예 없다**. 도구 vs 대화는 모델의 `tool_choice:auto`가 결정하고,
  2B가 도구 선택을 헷갈려 `tools_to_exclude` 스키마 숨김 휴리스틱으로 버티는 중.
- 여기에 3분류 부담을 더 얹으면 정확도가 더 흔들린다.

### 4.2 채택 구조

```
사용자 발화
  │
  ├─ (현행) 모델이 tool_choice:auto 로 도구 호출 판단 ──→ [도구 경로] 그대로 흘림
  │
  └─ 도구 미호출(=대화 턴)일 때:
        ├─ 인덱스 의미 검색(top-k)
        ├─ top-1 ≥ threshold?
        │     ├─ YES → 청크를 컨텍스트로 주입 → 모델이 청크 읽고 답 (= 로컬서치)
        │     └─ NO  → 일반대화
```

→ "로컬서치 vs 일반대화"의 경계를 **분류기가 아니라 검색 점수 임계값**이 자동으로 긋는다.
새 분류 모델 0개, graceful degradation, 인덱스 없으면 자동으로 일반대화. **도구는 기존대로.**

> 우리 구조 주의: 도구/대화 분기가 모델 내부라 alian처럼 "chat 경로에서만 RAG"라는
> 사전 분기점이 없다. 그래서 발화 시점에 검색해 임계값 통과 시 system에 주입하고,
> 도구 턴이면 "[참고 문서]는 관련될 때만 사용" 지시로 노이즈를 억제한다(§5.5).

---

## 5. RAG 프리훅을 run_turn에 끼우는 설계

### 5.1 우리 코드 기준 제약 3가지

1. **system은 messages[0] 1개만** (CLAUDE.md, Qwen 챗 템플릿). RAG 컨텍스트는
   별도 system 메시지로 못 넣고 **messages[0] 본문에 합쳐야** 한다.
2. **messages는 세션에 저장됨** (`commands.rs:318` clone → `:393` save). RAG 블록을
   messages[0]에 영구로 남기면 **다음 턴까지 스테일 컨텍스트가 누적**된다. 반드시
   **이번 턴 한정(ephemeral)** 처리.
3. **tool/chat 분기가 모델 내부**라 사전 분기점이 없다(§4.2 주의 참조).

### 5.2 데이터 흐름

```
commands.rs (async)
  ├ user_text 확정 (기존 :466~471 로직과 동일 소스)
  ├ refresh_system_prompt + enforce_history_budget        ← 기존 :324, :326
  ├ ★ rag = rag_prehook(search, user_text, ws).await       ← 신규 (검색 사이드카)
  └ run_turn(..., rag)                                      ← 시그니처에 rag 추가
        ├ ★ messages[0] 백업 후 RAG 블록 합침 (이번 턴만)
        ├ for round: client.complete(messages,...)         ← 기존 루프 그대로
        └ ★ 반환 직전 messages[0] 복원 (RAG 블록 제거)       ← 저장 오염 방지
```

### 5.3 프리훅 함수 (검색 사이드카 호출)

> ⚠️ **임계값 캘리브레이션 (2026-06-14 mac 빌드 실측, §8 참조)**: 이번 빌드의 `score`
> (RRF 하이브리드 점수)는 스케일이 작다 — 정확한 top 히트도 `score≈0.04`, `dense_cosine≈0.36`.
> alian의 0.65를 `score`에 그대로 쓰면 **모든 결과가 걸러져 RAG가 절대 안 걸린다.** 게이트는
> `score`가 아니라 **`dense_cosine` 기준**으로 두고 임계값도 재보정할 것(예: 0.4~0.5).

```rust
const RAG_TOP_K: u32 = 3;
// dense_cosine 기준 게이트(§8 실측). 작은 RRF score 가 아니라 의미 유사도로 거른다.
const RAG_MIN_COSINE: f32 = 0.52;  // 실측 보정(아래) — 빌드/모델 바뀌면 재보정
const RAG_MARKER: &str = "[참고 문서]";

/// 이번 발화에 대해 인덱스를 검색해 '근거 블록'을 만든다.
/// None = 색인 없음 / 검색 실패 / 임계값 미달 → 일반대화로 흘러감.
async fn rag_prehook(search: &SearchClient, query: &str, ws: &Path) -> Option<String> {
    if search.indexed_count().await.unwrap_or(0) == 0 { return None; } // /api/status
    let hits = search.search(query, RAG_TOP_K).await.ok()?;            // /api/search
    if hits.first()?.dense_cosine < RAG_MIN_COSINE { return None; }    // 의미 유사도 게이트
    let body = hits.iter()
        .filter(|h| h.dense_cosine >= RAG_MIN_COSINE)
        .enumerate()
        .map(|(i, h)| format!(
            "[#{n} 문서: {file} / 섹션: {head}] (관련도 {s:.3})\n{t}",
            n = i + 1, file = h.filename, head = h.heading, s = h.score, t = h.text))
        .collect::<Vec<_>>().join("\n\n");
    Some(format!(
        "{RAG_MARKER}\n{body}\n\n[지시] 위 문서가 질문과 의미상 관련될 때만 근거로 쓰고, \
         무관하면 완전히 무시한다. 문서에 없는 내용은 지어내지 말고 모른다고 답한다."))
}
```

### 5.4 호출부 + run_turn 주입/복원

```rust
// commands.rs — refresh/budget 뒤, run_turn 앞
let rag = match &state.search {
    Some(sc) => agent::rag_prehook(sc, &user_text, &ws).await,
    None => None,
};
let run = agent::run_turn(.., &mut messages, .., rag).await;   // ★ rag 인자 추가
```

```rust
// agent.rs run_turn — RAG 는 이번 턴 LLM 호출에만, 세션엔 저장 안 함
pub async fn run_turn(/* ...기존... */, rag: Option<String>) -> Result<()> {
    let user_text = /* 기존 */;
    let excluded  = tools_to_exclude(&user_text);

    let system_backup = messages.first().and_then(|m| m.content.clone());
    if let (Some(r), Some(s0)) = (&rag, messages.first_mut()) {
        s0.content = Some(format!("{}\n\n{}", system_backup.as_deref().unwrap_or(""), r));
    }

    let result = run_rounds(/* messages, tools, ... */).await; // 기존 루프 전체

    if rag.is_some() {
        if let Some(s0) = messages.first_mut() { s0.content = system_backup; } // 복원
    }
    result
}
```

> 모든 종료 경로에서 복원돼야 하므로 실구현은 **루프를 내부 함수로 빼고 반환값을 받은 뒤
> 복원**하거나 `scopeguard::defer!` 사용. 복원이 빠지면 RAG 블록이 세션에 눌러앉는다.

### 5.5 리스크 (구현 전 결정 필요)

1. **tool 턴 오염**: tool로 갈 발화에도 RAG 블록이 붙는다. 임계값 0.65 + "무관하면 무시"
   지시로 1차 방어. 더 보수적으로는 `tools_to_exclude`류 신호로 "명백한 파일조작 발화"면
   프리훅 자체를 skip하는 가드 추가 가능.
2. **prefill 비용**: 근거 블록(최대 3청크 × ~500자)이 매 턴 system에 얹혀 2B prefill·
   `history_budget` 압박. top_k·청크 길이 보수적으로.
3. **지연**: 검색 사이드카 왕복(임베딩 추론 포함)이 첫 토큰 앞에 직렬로 붙는다. top_k 작게,
   타임아웃 짧게(실패 시 None). alian이 `similar_doc`를 hot path에서 뺀 이유와 동일.
4. **취소(cancel)**: 프리훅 await 중 ■ 누르면 빠르게 빠지도록 타임아웃/cancel 체크.
5. **저장 정합성**: §5.4 복원 로직 누락 시 RAG 블록이 세션 오염 → 테스트로 가드.

---

## 6. 일반대화 품질 처리

정책서가 Alice를 "파일 검색·처리 도우미"로 스코프(일반 지식 Q&A는 목적 외). 2B의 약한
일반지식을 자신 있게 뱉으면 정책·신뢰 둘 다 해친다.

| 옵션 | 평가 |
|---|---|
| (a) 그냥 뱉기 | ❌ 환각을 확신 있게 출력 |
| (b) 순수 템플릿 거절 | ❌ 인사·잡담까지 막아 딱딱함 |
| (c) **하이브리드(추천)** | ✅ 인사/간단 잡담은 짧게 응답, 지식형 질문은 "저는 로컬 파일 검색·처리 도우미라 일반 지식은 부정확할 수 있어요" + 할 수 있는 일로 유도 |

→ **별도 분류기 없이 시스템 프롬프트 페르소나로** 처리(현 구조와 일관). 이미 있는
규칙 6/11(능력 질문→도구 나열) 및 `capability-discoverability` 작업과 결이 같다.
지식형 질문 감지가 필요하면 임계값-미달 RAG 결과(§5.3)를 신호로 재활용 가능.

---

## 7. 다음 단계 (제안 순서)

1. `SearchClient` 사이드카 래퍼 설계 — 기동·`/api/status`·`/api/search`를 우리
   `llm/server.rs` 패턴(+ macOS/Windows 분기)에 맞춰 구체화.
2. ~~`index_folder` 도구(폴더 인덱싱)~~ → **변경(2026-06-14)**: 채팅 도구 방식 폐기.
   대신 앱 시작 시 **워크스페이스(사용자 지정 폴더)의 문서를 자동 인덱싱**한다
   (`start_localsearch_inner` 가 serve 전에 `run_index(workspace)` 수행, 홈 전체는 가드로 제외).
   향후: 워크스페이스 변경 시 재인덱싱, 변경 감지(watcher), 증분 인덱싱.
3. RAG 프리훅 §5 구현 + "복원/저장 정합성" 단위 테스트.
4. 일반대화 페르소나 §6 시스템 프롬프트 반영.
5. 모델/바이너리 배포 경로 — fetch-deps 패처 확장.

> 참고 레퍼런스: alian `crates/alice-search/`(사이드카·watcher), `crates/alice-agent/src/chat.rs`
> (`try_localsearch_context`, `build_context`, `build_final_system`).

---

## 8. macOS(arm64) 빌드 레시피 — 검증 완료 (2026-06-14)

alian은 Windows `.exe`만 벤더링했다. macOS 바이너리는 없어서 `team-util/LocalSearch`
(`feature/cli`)를 직접 빌드했다. **빌드·인덱싱·의미검색 end-to-end 동작 확인.**

### 8.1 막은 것과 패치 (HWP 제외 빌드)

CLI(`localsearch-cli`)는 `--no-default-features`로 빌드하면 tauri 전용 모듈
(`commands`/`state`/`helpers`, `lib.rs`에서 `#[cfg(feature="tauri-app")]` 게이팅)이 빠진다.
그래도 막는 지점이 둘 있어 패치했다(throwaway 클론에만 적용):

| # | 파일 | 패치 | 이유 |
|---|---|---|---|
| 1 | `src-tauri/Cargo.toml` | `rhwp = { path = "../.reference/rhwp" }` 제거 | 비공개 경로 의존성(HWP 파서, goal-049) 접근 불가 |
| 2 | `src-tauri/src/indexing/hwp.rs` | rhwp 호출 → 빈 결과 스텁(`extract_chunks`/`extract_text`/`extract_text_for_classify`) | 1의 후속. hwp/hwpx 파일은 인덱싱 skip |
| 3 | `src-tauri/src/bin/cli.rs` | `/api/similar_doc` 핸들러를 미지원 스텁 | tauri 게이팅된 `commands::search::do_search_similar_doc` 의존 회피. RAG 불필요 |
| 4 | `src-tauri/src/bin/cli.rs` | `#[tokio::main]` 제거 → 일반 `fn main`, serve 만 수동 런타임(`Runtime::block_on`) | PDF 인덱싱 시 pdfium-auto 내부 런타임이 async 컨텍스트에서 드롭되며 "Cannot drop a runtime…" 패닉. Index/Search 를 tokio 밖에서 실행해 회피 |

> HWP/HWPX는 검색 전용 스펙 항목이지만 이번 빌드에서 제외. 고도화 시 rhwp 소스를 받아 원복.

### 8.2 빌드 명령

```bash
gh repo clone team-util/LocalSearch <dir> -- -b feature/cli --depth 1
# 위 패치 3개 적용 후:
cargo build --manifest-path <dir>/src-tauri/Cargo.toml \
  --bin localsearch-cli --no-default-features --release
# 산출물: <dir>/src-tauri/target/release/localsearch-cli  (Mach-O arm64)
```

### 8.3 런타임 의존성

| 의존 | 조달 | 지정 방법 |
|---|---|---|
| `libonnxruntime.dylib` (arm64) | Homebrew `onnxruntime` 1.25.0 (`/opt/homebrew/lib/libonnxruntime.dylib`)로 검증됨. ort 2.0.0-rc.12 load-dynamic 과 호환 | `ORT_DYLIB_PATH` 환경변수 |
| Harrier 모델 | `harrier-v1-270m-onnx/`(model_quantized.onnx + .onnx_data 344MB + tokenizer.json). alian/models 재활용 | `--models-dir <parent>` |
| pdfium | `pdfium-auto` crate 가 빌드 시 처리 (별도 조달 불필요) | — |
| 색인 DB | SQLite `text.db` 자동 생성 | `--db-dir <dir>` |

### 8.4 동작 검증 (스모크)

```bash
export ORT_DYLIB_PATH=/opt/homebrew/lib/libonnxruntime.dylib
localsearch-cli --models-dir <models> --db-dir <db> index <folder>
localsearch-cli --models-dir <models> --db-dir <db> search "<자연어 질의>" --top-k 3 --json
localsearch-cli --models-dir <models> --db-dir <db> serve --port 11234   # HTTP 사이드카
```

- 실측: Harrier dim=640. `"연차 며칠 쓸 수 있어"` → 단어 안 겹치는 "휴가 정책" 문서를 top 히트로
  반환(의미 매칭 OK).
- **점수 스케일 주의**: `score`(RRF) ≈ 0.04, `dense_cosine` ≈ 0.36. RAG 게이트는
  `dense_cosine` 기준으로(§5.3 캘리브레이션 경고).

### 8.6 PDF 자가 테스트 결과 (2026-06-14)

테스트셋: `~/Downloads/test_alice/PDF_테스트` 8개 PDF(가계부·건강검진·QA플랜·가이드·월간보고서 3부·임대차계약서). 인덱싱 **32 chunks, 0 errors**.

자연어(키워드 비직결) 질의 9개 중 **8개 정답(top-1)**:

| 질의 | top-1 | dense_cosine | 판정 |
|---|---|---|---|
| 콜레스테롤·혈압 수치 정상이었나 | 건강검진_결과지 | 0.575 | ✅ |
| 이번 달 식비로 얼마나 썼지 | 가계부 | 0.549 | ✅ |
| 보증금·월세 조건 | 주택임대차_계약서 | 0.569 | ✅ |
| 프로젝트에서 위험한 부분 | 월간보고서_2부_이슈및리스크 | 0.517 | ✅ |
| 이 앱 처음인데 어떻게 쓰나 | 앨리스_가이드 | 0.455 | ✅ |
| 다음 달 업무 추진 계획 | 월간보고서_3부_다음달계획 | 0.577 | ✅ |
| QA 테스트 절차 | 앨리스_QA테스트플랜 | 0.569 | ✅ |
| 이번 달 주요 성과 요약 | 월간보고서_1부_성과요약 | 0.563 | ✅ |
| (모호) 다음 달에 뭘 하기로 했더라 | 가계부 | 0.473 | ❌ (재표현 시 정답) |

관찰:
- 정답 top-1 dense_cosine 범위 **0.455~0.577**. 경쟁(오답) 청크도 0.45~0.52라 분리도는 크지 않다.

**임계값 재보정 (2026-06-14, 워크스페이스 494 chunks 기준)**: 0.45 는 잡담에도 오발동했다
("하이?"에 출처 칩 노출). 실측 결과 군집이 갈린다 — 잡담(하이?/안녕/하이/반가워/고마워)
**cos 0.434~0.463**, 내용 질문(식비 0.566 / 콜레스테롤 0.593) **0.566~0.593**. 두 군집 사이
**0.52** 로 상향. 경계 질문("이 앱 어떻게 쓰나" 0.455)은 일반대화로 빠질 수 있으나 인사
오발동보다 낫다. (코사인만으로 완벽 분리는 안 됨 — 향후 길이/패턴 가드 보강 여지.)
- 모호한 질의는 빗나갈 수 있으나, 의도가 분명하면 안정적으로 정답.

### 8.5 배포 시 (3번 단계에서)

- 플랫폼별 `localsearch-cli`(win `.exe` / mac Mach-O)를 vendor에 분기 배치.
- `libonnxruntime.dylib`는 우리 `vendor/onnxruntime` 정책과 통일(우리는 이미 ort load-dynamic).
- Harrier 모델·바이너리는 용량이 크므로 fetch-deps 패처로 최초 실행 시 내려받는 방안 우선.
