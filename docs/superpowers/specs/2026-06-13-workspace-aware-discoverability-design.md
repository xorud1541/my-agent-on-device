# 설계 — 워크스페이스 인지형 빈 화면 (능력 디스커버빌리티)

> 작성: 2026-06-13 / 브랜치: `feat/capability-discoverability` (베이스 `feat/screenshot-vision`)
> 선행 검토: [`capability-discoverability-review.md`](./capability-discoverability-review.md) (옵션 A~D + 1·2차 교차검증)

## 1. 문제와 목표

사용자가 워크스페이스를 열면 **"뭐부터 해야 하지?"**라며 진입점을 못 찾는다. 현재 빈 화면은
정적 제안 칩 4개뿐이고, 워크스페이스가 비어 있으면 첫 경험이 막다른 길이 된다.

**목표:** 첫 화면이 *지금 이 폴더에서 실제로 할 수 있는 일*을 보여줘 사용자가 바로 시작하게 한다.
능력의 진실이 런타임/폴더에 따라 다르므로(예: 빈 폴더엔 "배경 제거"가 무의미), **현재 워크스페이스
내용에 grounded된 결정적 제안**을 만든다.

**비목표 (이번 범위 밖):** 한계 목록 UI 노출, 상시 능력 패널(A/C안), CapabilityManifest 풀 구현,
LLM 생성 제안. (선행 검토 §4 참고)

## 2. 동작 — 빈 화면 상태머신

빈 화면 진입(앱 시작/새 대화) 시 **하나의** 상태로만 렌더한다. 동시 표시 없음.

```
Q1. 사용자가 작업 폴더를 골랐나?  (workspace_dir == 홈 기본값 → "아직 안 고름")
 ├─ 아니오(홈/첫 실행) ──────────────→ 상태 ②  "작업할 폴더 선택" 유도
 └─ 예(폴더 지정됨) → Q2. 다룰 파일이 있나?
        ├─ 있음 ───────────────────→ 상태 ①  요약 줄 + 맞춤 제안
        └─ 비어 있음 ───────────────→ 상태 ①' "이 폴더는 비어 있어요" + 폴더 변경/캡처
```

### 상태 ① — 폴더 + 파일 있음 (핵심)
- **요약 줄**: "📁 `<폴더명>` 폴더에 🖼 이미지 N · 📄 PDF N · 🗜 zip N" (0인 타입은 생략).
- **맞춤 제안 칩(결정적 템플릿)** — 보유 타입에 매핑된 것만:
  - 이미지 ≥1 → "이미지 N장을 PDF 한 권으로 묶기", **(`.ort` 모델 있을 때만)** "사진 N장 배경 제거하기"
  - PDF ≥1 → "PDF N개에서 텍스트 추출"
  - zip ≥1 → "zip 풀기"
  - (항상) "화면 캡처해줘"
- 추가 능력 힌트("이런 것도…") **없음** — 요약 + 제안만.

### 상태 ② — 홈/첫 실행
- 한 줄 소개("사진 정리·배경제거, 이미지→PDF, 화면 캡처를 이 PC 안에서만 도와드려요").
- **"📁 작업할 폴더 선택"** 버튼(기존 `pick_folder` 커맨드 → 선택 시 `set_config`로 workspace 변경).
- 폴더무관 보편 제안: "화면 캡처해줘".

### 상태 ①' — 폴더 지정 + 빈 폴더
- "이 폴더는 비어 있어요" + [폴더 변경] + "화면 캡처해줘". (②의 변형, 문구만 폴더 기준)

### 제안 클릭 동작
- 클릭 시 **입력창(Composer)에 문장을 채운다**. 바로 실행하지 않음 — 사용자가 보고 Enter.
  (현재 `App.tsx:106`의 `send(s)` 즉시 실행을 prefill 방식으로 변경.)

## 3. 아키텍처

### 3.1 백엔드 — `workspace_summary` 커맨드 (신규, 읽기 전용)
- 위치: `src-tauri/src/commands.rs` + `lib.rs` `generate_handler` 등록.
- 입력: 없음(현재 `config.workspace_path()` 사용).
- 동작: 워크스페이스를 **1-depth** 스캔(하위 폴더 재귀 안 함, 속도/단순), 확장자로 분류:
  - `images` (png/jpg/jpeg/webp/gif/bmp/tiff), `pdfs` (pdf), `zips` (zip), `others`.
- 반환(JSON, serde): `{ workspace_dir, is_default_home: bool, is_empty: bool, images: u32, pdfs: u32, zips: u32, others: u32, removebg_available: bool }`
  - `is_default_home`: `workspace_dir`가 `dirs::home_dir()`와 같은가 (상태 ② 판별).
  - `is_empty`: 다룰 수 있는 파일(images+pdfs+zips)이 0인가.
  - `removebg_available`: `config.removebg_model` 경로의 `.ort` 파일이 존재하는가 → 배경제거 제안 게이트(없으면 그 칩 생략, 막다른 길 방지).
- 모델 미사용·결정적. 읽기 전용이라 `ensure_in_workspace` 가드 불필요.
- 에러: 스캔 실패 시 `Err` → 프론트는 안전하게 상태 ②로 폴백.

### 3.2 프론트엔드
- `src/types.ts`: `WorkspaceSummary` 타입 추가(백엔드 반환과 동기화).
- `src/hooks/useAgent.ts`: 빈 화면 진입 시 + `config-changed` 수신 시 `workspace_summary` invoke → 상태 보관.
- `src/App.tsx`: 빈 화면 분기를 상태머신으로 교체(현재 `SUGGESTIONS` 정적 배열 제거).
- 제안 생성: `src/lib/`에 결정적 매핑 함수(`buildSuggestions(summary)`) 신설 → 단위 테스트 용이.
- Composer prefill: `App` → `Composer`에 `prefill` prop 전달(또는 텍스트 상태를 lift). 클릭 시 입력창 채움.

### 3.3 반응형 한계 처리 (system_prompt)
- `agent.rs system_prompt`에 **간결한 1~2줄** 추가: 범위 밖(웹 검색/다운로드, 이메일, OCR/스캔판독,
  이미지 생성, 다른 앱 제어)은 **할 수 없다고 솔직히 말하고, 가능한 가장 가까운 대안을 제시**한다.
- ⚠️ **2B 라우팅 회귀 위험** (README/CLAUDE.md 경고). → **`bench/toolcall_test.mjs` 통과를 게이트**로 하고,
  통과 못 하면 문구를 줄이거나 도구 결과·UI 단 유도로 대체(선행 검토 §5.5 코덱스 의견).

## 4. 데이터 흐름

```
앱 시작/새 대화 ─┐
config-changed ──┴→ useAgent: invoke("workspace_summary")
                         │
                         ▼
                   WorkspaceSummary  ──→ App 상태 분기(①/①'/②)
                         │                     │
                         │              buildSuggestions(summary)  (결정적)
                         ▼                     ▼
                   요약 줄 렌더            맞춤 제안 칩 → 클릭 → Composer prefill
```

## 5. 에러 처리 / 엣지
- `workspace_summary` 실패/권한 거부 → 상태 ②로 폴백(폴더 선택 유도). 앱이 멈추지 않음.
- 파일이 매우 많은 폴더 → 1-depth + 개수만 세므로 비용 일정. 상한 불필요.
- 워크스페이스 도중 변경(헤더 칩/설정/에이전트 `set_workspace`) → `config-changed`로 재조회·재렌더.
- 서버 로딩 중(`server.status != ready`) → 제안 칩 비활성(현재 동작 유지).

## 6. 테스트
- **단위(Rust)**: `workspace_summary` — 타입 분류 정확도, `is_empty`/`is_default_home` 판별(임시 디렉터리 픽스처).
- **단위(프론트)**: `buildSuggestions(summary)` — 타입 조합별 제안 목록(이미지만/PDF만/혼합/빈 폴더).
- **렌더**: 빈 화면 3상태(①/①'/②) 분기.
- **회귀(게이트)**: system_prompt 변경 시 `bench/toolcall_test.mjs`로 2B 툴콜 정확도 회귀 확인.

## 7. 구현 순서(개략)
1. 백엔드 `workspace_summary` + 단위 테스트 + 핸들러 등록.
2. `types.ts` 동기화 + `buildSuggestions` + 단위 테스트.
3. `App.tsx` 빈 화면 상태머신 + `useAgent` 조회/구독.
4. Composer prefill 배선.
5. system_prompt 한계 문구 + bench 회귀 검증(게이트).
