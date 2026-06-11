# Local Agent

CPU + iGPU(Vulkan)에서 llama.cpp 사이드카로 동작하는 로컬 온디바이스 LLM 에이전트.
모델이 도구를 호출해 파일/이미지/PDF/캡처 작업을 수행한다.

## 기술 스택
- **Frontend**: React 19 + TypeScript + Vite
- **Backend**: Rust + Tauri 2
- **추론**: llama-server (llama.cpp Vulkan, OpenAI 호환 HTTP) 사이드카
- **기본 모델**: Qwen3.5-2B-Q4_K_M (벤치마크 근거: docs/superpowers/specs/)
- **Package Manager**: pnpm
- **Build Target**: Windows (NSIS)

## 프로젝트 구조
```
src/                  → React 프론트엔드 (hooks/useAgent.ts 가 이벤트 → 상태 환원)
src-tauri/src/        → agent.rs(루프), llm/(사이드카+클라이언트), tools/(도구), commands.rs(IPC)
src-tauri/tests/      → e2e_agent.rs (실제 모델 E2E)
bench/                → 모델 벤치마크 하니스
```

## 개발 명령어
```bash
pnpm tauri dev        # 개발 서버 + Tauri 창 (앱 시작 시 llama-server 자동 기동)
pnpm tauri build      # 릴리즈 빌드 (NSIS 설치파일)
cd src-tauri && cargo test    # 단위 테스트 (모델 불필요)
cd src-tauri && cargo test --test e2e_agent --release -- --ignored --nocapture --test-threads=1
```

## 규칙
- Tauri 커맨드는 `src-tauri/src/commands.rs`에 정의, 프론트↔백 통신은 `invoke()` + `agent-event` 리슨
- 백엔드 이벤트(AgentEvent)와 `src/types.ts` 타입은 항상 동기화할 것 (serde kebab-case tag)
- 도구 추가 시: `tools/` 에 Tool trait 구현 + `ToolRegistry::with_default_tools` 등록 + 단위 테스트
- 파괴적 도구는 금지 — 삭제는 반드시 휴지통(trash crate) 경유
- 스킬을 추가/삭제/이름변경할 때 README.md도 같이 업데이트할 것
