# Local Agent — 설계 문서

날짜: 2026-06-11
상태: 승인 대기 (자율 모드 — CLAUDE.md "확인 없이 진행" 원칙에 따라 가정 명시 후 진행)

## 목표

CPU + iGPU 데스크톱 환경에서 사용자 쿼리(싱글턴/멀티턴)에 대해 **30~60초 내**에
스스로 도구를 선택·실행하여 행동하는 로컬 온디바이스 에이전트.

지원 행동: 파일 제어, 파일/이미지 검색, 이미지 처리, PDF 처리, 화면 캡처.

## 명시된 가정

1. 프로젝트명 `local-agent`, 경로 `C:\repo_private\local-agent`, 제품명 "Local Agent".
2. 추론 런타임은 사용자가 보유한 `llama-b9334-bin-win-vulkan-x64`의 `llama-server.exe`,
   디바이스는 `Vulkan0`(Intel Arc iGPU, 공유메모리 ~18GB). NVIDIA dGPU는 타겟에서 제외.
3. 모델은 로컬 보유 모델(supergemma4-E4B, gemma-4-E2B/E4B, Qwen3.5-2B/4B, Llama-3.1-8B)을
   벤치마크해 선정하되, 설정으로 교체 가능하게 한다.
4. 한국어 사용자 발화를 기본으로 한다 (시스템 프롬프트 한국어 대응).

## 아키텍처

```
┌────────────────────────── Tauri App ──────────────────────────┐
│  React 19 (채팅 UI)                                            │
│   └─ invoke("send_message") / listen("agent-event")            │
│  Rust 백엔드                                                   │
│   ├─ llm/  llama-server 사이드카 관리 (spawn, health, 종료)     │
│   ├─ llm/  OpenAI 호환 스트리밍 클라이언트 (SSE, tool_calls)    │
│   ├─ agent/ 에이전트 루프: 쿼리→추론→툴콜→결과 주입→반복        │
│   ├─ tools/ 파일제어·검색, 이미지 검색/처리, PDF, 캡처          │
│   └─ session/ 멀티턴 대화 상태 (메시지 히스토리)                │
└───────────────────────────────────────────────────────────────┘
            │ HTTP (localhost, OpenAI 호환 /v1/chat/completions)
            ▼
      llama-server.exe (Vulkan0, iGPU 오프로드, 사이드카 프로세스)
```

### 핵심 결정과 근거

- **사이드카 + HTTP** (vs llama-cpp FFI 바인딩): 모델 교체/크래시 격리/런타임 업그레이드가
  프로세스 경계 뒤로 숨음. llama-server의 검증된 툴콜 파서(`--jinja`)를 그대로 활용.
- **에이전트 루프는 Rust에서** (vs 프론트엔드): 도구 실행이 OS 자원(파일, 화면)에 직접
  닿으므로 백엔드 단일 소유. 프론트는 이벤트 구독만.
- **이벤트 스트리밍**: Tauri `emit`으로 `agent-event` 단일 채널에
  `thinking-delta | text-delta | tool-call | tool-result | turn-end | error` 페이로드를 흘린다.
  UI는 이걸로 GPT/Codex 스타일 "생각 중…" 접이식 블록과 도구 타임라인을 그린다.

## 도구 목록 (1차)

| 도구 | 기능 | 구현 |
|------|------|------|
| `list_dir` | 디렉토리 나열 | std::fs |
| `read_file` | 텍스트 파일 읽기 (크기 제한) | std::fs |
| `write_file` | 파일 쓰기/생성 | std::fs |
| `move_path` / `copy_path` | 이동/복사/이름변경 | std::fs |
| `delete_path` | 삭제 (휴지통으로) | trash crate |
| `search_files` | 이름/글롭/수정일 기준 파일·이미지 검색 | walkdir + glob |
| `image_info` / `image_transform` | 메타조회, 리사이즈/회전/포맷변환/크롭 | image crate |
| `pdf_extract_text` | PDF 텍스트 추출 (페이지 범위) | pdf-extract |
| `screen_capture` | 전체/모니터별 캡처 → PNG 저장 | xcap crate |

안전장치: 삭제는 휴지통 경유. 쓰기/삭제/이동은 도구 결과에 수행 내역 명시(UI에 표시).

## 레이턴시 예산 (목표 30~60초)

- 모델 로드: 앱 시작 시 1회 (워밍업). 쿼리 경로에서 제외.
- 1 행동 = 보통 2~3 LLM 호출(계획→툴콜→최종응답). 호출당 prefill+decode ≤ 15초가 되도록
  4B급 Q4 모델 + iGPU 오프로드 + 컨텍스트 8K로 운용. 벤치마크로 검증.

## 모델 선정 기준 (벤치마크 항목)

1. tok/s (prefill/decode, Vulkan0 오프로드)
2. 툴콜 정확도: 한국어 지시 → 올바른 도구+인자 JSON 생성률 (시나리오 5종)
3. 멀티턴 추종성
4. 메모리 풋프린트 (iGPU 공유메모리 내)

후보: Qwen3.5-4B-Q4_K_M(권장 예상), Qwen3.5-2B, gemma-4-E4B(=supergemma4 베이스),
Llama-3.1-8B-Q4_K_M(상한 비교용).

## 에러 처리

- llama-server 다운 → 자동 재시작 1회, 실패 시 UI에 배너.
- 툴 실행 실패 → 에러 문자열을 tool 결과로 모델에 반환 (모델이 대안 시도 가능).
- 루프 상한: 툴콜 8회/턴, 초과 시 중단하고 현재까지 결과 요약.

## 테스트

- Rust 단위 테스트: 도구 각각 (임시 디렉토리 기반), 에이전트 루프 (mock LLM 클라이언트).
- E2E: 실제 모델로 시나리오 5종 레이턴시 측정 (파일 검색, 이미지 리사이즈, PDF 요약,
  캡처, 멀티턴 후속 질의).
