# Local Agent

CPU + iGPU 환경에서 동작하는 로컬 온디바이스 LLM 에이전트 데스크톱 앱.
llama.cpp(Vulkan)를 사이드카로 구동하고, 모델이 도구(파일 제어/검색, 이미지 처리, PDF, 화면 캡처)를
스스로 호출해 사용자 요청을 수행한다. Tauri 2 + React 19 + TypeScript.

## 시작하기

### 요구 사항
- [Rust](https://rustup.rs/) 1.77.2+
- [Node.js](https://nodejs.org/) 18+
- [pnpm](https://pnpm.io/) 9+
- llama.cpp Vulkan 빌드 (`llama-server.exe`) — 기본 경로 `~/Downloads/llama-b9334-bin-win-vulkan-x64`
- GGUF 모델 — 기본 `~/.lmstudio/models/lmstudio-community/Qwen3.5-2B-GGUF/Qwen3.5-2B-Q4_K_M.gguf`

경로가 다르면 첫 실행 후 앱 내 **설정** 패널 또는
`%APPDATA%/com.estsoft.local-agent/config.json` 에서 변경한다.

### 설치 및 실행
```bash
pnpm install
pnpm tauri dev
```

### 빌드
```bash
pnpm tauri build
```

## 아키텍처

```
React (채팅 UI) ── invoke/listen ── Rust (Tauri)
                                      ├─ agent.rs    에이전트 루프 (툴콜 반복, 취소, 회복)
                                      ├─ llm/        llama-server 사이드카 + SSE 스트리밍 클라이언트
                                      ├─ tools/      list_dir, read/write_file, move/copy/delete,
                                      │              search_files, image_info/transform,
                                      │              remove_background(ONNX), images_to_pdf,
                                      │              pdf_extract_text, screen_capture,
                                      │              zip_create/zip_extract,
                                      │              set_workspace, update_profile
                                      └─ commands.rs IPC 커맨드 + 세션 관리
                                            │ HTTP (OpenAI 호환, localhost)
                                            ▼
                                     llama-server.exe (Vulkan0 = Intel iGPU, --reasoning off)
```

- 이벤트 채널 `agent-event` 하나로 `thinking-delta / text-delta / tool-call-start /
  tool-call-end / turn-end / error / server-status / config-changed` 를 스트리밍한다.
- 대화 로그: `%APPDATA%\com.estsoft.local-agent\logs\chat_YYYYMMDD.jsonl` (+ llama-server.log)

### 2차 스펙 기능
- **워크스페이스**: 파일 생성/수정/삭제는 설정된 워크스페이스 안으로 제한 (읽기/검색은 전체 허용).
  헤더 칩/설정 패널에서 변경, 에이전트도 `set_workspace` 로 변경 가능 — 양쪽 모두
  `config-changed` 이벤트로 실시간 동기화되고 시스템 프롬프트는 매 턴 재생성된다.
- **배경제거**: `remove_background` 도구. `~/.alice/models/removeBG.ort` (ONNX Runtime,
  Windows 는 load-dynamic — `src-tauri/vendor/onnxruntime/onnxruntime.dll` 사용, 빌드 시 번들).
- **이미지 → PDF**: `images_to_pdf` 도구 (JPEG 무손실 passthrough, EXIF/알파 보정, A4/Letter/Fit).
  경로 배열(`paths`) 또는 폴더(`dir`, 이름순) 입력 — 작은 모델이 경로를 직접 옮겨 적지 않아도 된다.
- **페르소나**: 이름(user_name/agent_name)이 비어 있으면 대화 초반에 서로 이름을 묻고
  `update_profile` 로 영속화. 설정 패널에서도 수정 가능.

## 문서

| 문서 | 내용 |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 구조, 설계 결정과 근거, 에이전트 루프, 레이턴시 예산, 도구 추가법, 설정 레퍼런스 |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | 실사용 로그로 잡은 버그 7건 사례집 + 로그 보는 법 + 개발 환경 함정 |
| [docs/TESTING.md](docs/TESTING.md) | 단위/E2E/UI 자동구동/모델 벤치 실행법 |
| [docs/superpowers/specs/](docs/superpowers/specs/) | 1차 설계 문서 + 모델 벤치마크 결과 |

## 테스트

```bash
cd src-tauri
cargo test                 # 단위 테스트 (도구 + 에이전트 루프, 모델 불필요)
cargo test --test e2e_agent --release -- --ignored --nocapture --test-threads=1
                           # E2E (실제 모델 구동, 시나리오 5종 + 레이턴시 검증)
node bench/toolcall_test.mjs <model.gguf> <label>   # 모델별 툴콜 정확도 비교
```

## 프로젝트 구조

```
src/                  React 프론트엔드
├── App.tsx           메인 앱 컴포넌트
├── hooks/useAgent.ts 이벤트 스트림 → 메시지 상태 환원
├── components/       ThinkingBlock, ToolStep, Composer, SettingsPanel, MessageView
├── types.ts          공통 타입 (백엔드 AgentEvent 와 1:1)
└── styles/           스타일시트

src-tauri/            Rust 백엔드
├── src/
│   ├── lib.rs        Tauri 앱 설정 + AppState
│   ├── commands.rs   IPC 커맨드 (send_message, 세션, 설정, 모델 목록)
│   ├── agent.rs      에이전트 루프 + 시스템 프롬프트
│   ├── llm/          llama-server 사이드카, 스트리밍 클라이언트
│   ├── tools/        도구 구현 (파일/검색/이미지/PDF/캡처)
│   ├── config.rs     앱 설정 영속화
│   └── models.rs     데이터 모델
├── tests/e2e_agent.rs E2E 시나리오
└── tauri.conf.json   Tauri 설정

bench/                모델 벤치마크 하니스
skills/               개발 스킬
└── experimental/     실험적 스킬
```
