# Local Agent 아키텍처

2026-06-11 1차 작업 기준. 코드와 어긋나면 코드가 정답이다.

## 전체 구조

```
┌─────────────────────────── Tauri App ───────────────────────────┐
│  React 19 (src/)                                                 │
│   ├─ hooks/useAgent.ts   agent-event 스트림 → 메시지 상태 환원    │
│   ├─ components/         ThinkingBlock, ToolStep, Composer,      │
│   │                      SettingsPanel, MessageView              │
│   └─ invoke() ↔ listen("agent-event")                            │
│                                                                  │
│  Rust (src-tauri/src/)                                           │
│   ├─ lib.rs        AppState (config/server/sessions/cancels)     │
│   ├─ commands.rs   IPC: send_message, new_session, set_config,   │
│   │                list_models, cancel_turn, restart_server      │
│   ├─ agent.rs      에이전트 루프 + 시스템 프롬프트                │
│   ├─ llm/server.rs llama-server 사이드카 (spawn/health/stop)     │
│   ├─ llm/client.rs OpenAI 호환 SSE 스트리밍 + 재시도              │
│   ├─ tools/        Tool trait + 도구 13종                        │
│   ├─ config.rs     %APPDATA%/com.estsoft.local-agent/config.json │
│   └─ logging.rs    대화 JSONL + llama-server.log                 │
└──────────────────────────────────────────────────────────────────┘
                 │ HTTP localhost:8736 (/v1/chat/completions)
                 ▼
        llama-server.exe  (--device Vulkan0 = Intel iGPU,
                           --jinja, --reasoning off, -c 16384)
```

## 핵심 설계 결정과 근거

| 결정 | 근거 |
|---|---|
| llama-server **사이드카 + HTTP** (FFI 바인딩 대신) | 모델 교체·크래시 격리·런타임 업그레이드가 프로세스 경계 뒤로 숨음. llama-server의 검증된 툴콜 파서(--jinja) 재사용 |
| 에이전트 루프는 **Rust 단일 소유** | 도구가 OS 자원(파일/화면)에 직접 닿으므로 프론트는 이벤트 구독만 |
| 이벤트 채널 **하나** (`agent-event`) | `thinking-delta / text-delta / tool-call-start / tool-call-end / turn-end / error / server-status` 페이로드 태그로 구분. 프론트 타입은 `src/types.ts`와 1:1 (serde kebab-case) |
| 기본 모델 **Qwen3.5-2B-Q4_K_M** | iGPU(Vulkan0) 실측 decode 19.3 t/s, 한국어 툴콜 5/5, 턴당 10~30초. 비교: supergemma4-E4B 10.5 t/s(5/5, 품질 대안), Qwen3.5-4B 7.8 t/s(탈락) |
| **사고(thinking) 기본 끔** (`--reasoning off`) | 사고는 호출당 +13초였고, `--reasoning-budget` 강제 종료 직후 모델이 EOS를 내는 불안정 경로 존재. 끄고도 툴콜 5/5, 호출당 1~5초 |
| 삭제는 **휴지통 경유** (trash crate) | 파괴적 행동 방지. 영구 삭제 도구는 의도적으로 없음 |

## 에이전트 루프 (agent.rs::run_turn)

```
사용자 발화 → [성공하는 동안 계속, 절대 상한 = max_tool_rounds×3(24)]
  complete() 스트리밍 (thinking/text 델타 즉시 emit)
  ├─ 툴콜 없음 → 종료
  ├─ 빈 완성 (본문·툴콜 모두 없음) → 1회 재생성, 그래도 비면 Error emit
  ├─ 컨텍스트 초과 오류 → 오래된 도구 결과 압축(최근 2개 보존) 후 재시도
  └─ 툴콜 있음 → 각 호출:
       동일 (이름, 인자) 반복이면 실행 없이 거부 메시지 반환
       실행 → ToolCallEnd emit → tool 메시지(4000자 클립) 추가 → 다음 라운드
취소(■): 델타 sink가 false 반환 → 클라이언트가 SSE 절단 (0.5초 내 반응)
```

## 레이턴시 예산 (목표: 발화당 30~60초)

- 모델 로드는 앱 시작 시 1회 (쿼리 경로 제외)
- 1턴 = 보통 2~4회 LLM 호출. 호출당 출력 상한 `max_output_tokens`(기본 1024 ≈ 20 t/s에서 50초 상한, 실제론 수백 토큰)
- 컨텍스트 16K + llama-server 프롬프트 캐시(같은 세션 prefill 재사용)
- 도구 결과 억제: list_dir 100개 상한, 히스토리 클립 4000자, 간결 응답 프롬프트 규칙

## 도구 추가 방법

1. `src-tauri/src/tools/`에 `Tool` trait 구현 (name/description/parameters/execute)
2. `ToolRegistry::with_default_tools()`에 등록
3. 단위 테스트 필수 (tempdir 기반). 파라미터 description은 모델이 읽으므로 한국어로 구체적으로
4. 결과 문자열은 모델 컨텍스트에 들어간다 — 장황하면 안 됨, 상한 명시

## 설정 (config.json)

| 키 | 기본값 | 비고 |
|---|---|---|
| server_exe | ~/Downloads/llama-b9334-bin-win-vulkan-x64/llama-server.exe | |
| model_path | ~/.lmstudio/models/.../Qwen3.5-2B-Q4_K_M.gguf | 설정 패널에서 교체 |
| device / n_gpu_layers | Vulkan0 / 99 | iGPU 전체 오프로드 |
| ctx_size | 16384 | 변경 시 서버 재시작 |
| max_output_tokens | 1024 | 레이턴시 상한 레버 |
| reasoning_budget | 0 (사고 끔) | N>0: 예산+유도 메시지, -1: 무제한 |
| temperature | 0.4 | 높이면 툴콜 JSON 오류율 증가 |
| max_tool_rounds | 8 | 루프 예산 기준값 — 성공 라운드는 예산을 깎지 않고, 성공 0회 라운드 2연속이면 조기 마무리, 절대 상한은 ×3 (2026-06-12 정책) |
