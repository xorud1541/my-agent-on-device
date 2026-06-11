# 2차 스펙 설계 — 워크스페이스 / 이미지 고도화 / 페르소나

날짜: 2026-06-11. 사용자 위임에 따라 권장안으로 확정하고 바로 구현한다.

## 1. 유저 워크스페이스

**목표**: 사용자가 지정한 폴더("워크스페이스") 안에서 에이전트의 작업을 보장하고,
UI와 에이전트 루프가 실시간으로 같은 워크스페이스를 본다.

- `AppConfig.workspace_dir: String` 추가 (기본값: 사용자 홈). config.json 영속.
- **강제 범위 (권장안)**: *쓰기성* 경로만 워크스페이스 내부로 제한한다 —
  `write_file`, `move_path`/`copy_path`의 목적지, `delete_path`, `zip_create`/`zip_extract`의 출력,
  `image_transform`/`remove_background`/`images_to_pdf`의 출력, `screen_capture` 저장 경로.
  읽기/검색(`list_dir`, `read_file`, `search_files`, `image_info`, `pdf_extract_text`)은 전체 허용.
  근거: "다운로드에서 찾아서 워크스페이스에 정리해줘" 같은 핵심 시나리오를 깨지 않으면서
  파일 생성/변경/삭제는 보장된 공간 안에 가둔다.
- 가드 구현: `tools/workspace.rs::ensure_in_workspace(path, ws)` — 어휘적 정규화
  (`/`↔`\`, `..` 거부, Windows 대소문자 무시) 후 prefix 검사. 위반 시 도구가 한국어 오류를
  돌려줘 모델이 워크스페이스 내부 경로로 재시도하게 한다.
- **실시간 동기화**:
  - UI → 루프: `send_message`가 매 턴 config를 읽고 **시스템 프롬프트(messages[0])를 턴마다 재생성**.
    세션 중 변경이 즉시 반영된다 (현재 시각 갱신 부수효과도 얻음).
  - 루프 → UI: 에이전트용 `set_workspace` 도구. config 저장 후 `AgentEvent::ConfigChanged { config }`
    방송 → 헤더/설정 패널 즉시 갱신. `set_config` 커맨드도 같은 이벤트를 쏜다.
- UI: 헤더에 워크스페이스 칩 표시. 설정 패널에서 `pick_folder` 커맨드(rfd 네이티브 다이얼로그)로 선택.

## 2. 이미지 처리 고도화

선작업 출처: `C:\repo\alian` (`alice-tools-image-ai`, `alice-tools-pdf`, `alice-tools-image`).

### 2a. 배경제거 `remove_background`
- alian `bg_remove.rs`(768 정사각 리사이즈 → NCHW f32 → 마스크 → 알파 합성) +
  `ort_model.rs`(ort Session 래퍼) 포팅.
- 모델: `AppConfig.removebg_model` (기본 `~/.alice/models/removeBG.ort`).
- 의존성: `ort = "=2.0.0-rc.12"` Windows `load-dynamic` (alian과 동일 — MSVC LNK2038 회피).
  `onnxruntime.dll`은 alian `vendor/onnxruntime/`에서 복사해 `src-tauri/vendor/onnxruntime/`에 보관,
  앱 시작 시 `ORT_DYLIB_PATH` 설정(개발: CARGO_MANIFEST_DIR 기준, 배포: exe 옆 리소스 번들).
  System32의 구버전 onnxruntime.dll 오바인딩을 막기 위해 명시 경로를 쓴다.
- 세션은 `OnceLock<Mutex<OrtModel>>` 캐시 — 호출마다 모델 재로드 방지.
- 출력은 PNG 강제(투명도). 입력은 content-sniffing으로 연다 (확장자-내용 불일치 견고성, alian 교훈).

### 2b. 이미지 회전
- 기존 `image_transform`의 rotate(90/180/270)가 이미 충족. 변경: 입력 열기를
  content-sniffing(`open_image_sniffed`)으로 교체해 견고성만 보강.

### 2c. 이미지들 → PDF `images_to_pdf`
- alian `image_to_pdf.rs` + `page.rs` + `probe.rs` 포팅 (`lopdf` 의존성 추가).
  JPEG 무손실 passthrough, EXIF 회전/알파/CMYK 보정, 일괄 검증 fail-fast, 원자적 저장 유지.
- 인자: `paths: string[]`, `output_path`, `page_size: a4|letter|fit` (기본 **a4**).

## 3. 페르소나 / 라포 형성

- `AppConfig.user_name`, `AppConfig.agent_name` (빈 문자열 = 미설정). config.json 영속 —
  앱을 껐다 켜도 이름을 기억한다.
- `update_profile` 도구: 모델이 대화에서 알게 된 이름을 저장 (`user_name?`, `agent_name?`).
  저장 시 `ConfigChanged` 이벤트로 UI 즉시 반영 (헤더에 에이전트 이름 표시).
- 시스템 프롬프트 분기:
  - 이름 미설정: "첫 인사에서 자연스럽게 사용자의 이름을 묻고, 너의 이름을 지어달라고
    부탁하라. 알게 되면 update_profile로 저장하라." (작업 요청이 먼저 오면 작업 우선)
  - 설정됨: "너의 이름은 {agent}. 사용자는 {user}. 따뜻하고 친근한 말투."
- UI: 설정 패널에서 두 이름 수동 수정 가능.

## 단위 경계 / 테스트

- `tools/workspace.rs`: 가드 순수 함수 + SetWorkspace 도구. 가드 경계 테스트(내부/외부/`..`/대소문자).
- `tools/image_ai.rs`: OrtModel + RemoveBackground. 모델 파일 없으면 skip하는 E2E 테스트(alian 방식).
- `tools/pdf_make.rs`: probe/page/image_to_pdf — alian 테스트 포팅.
- `tools/profile.rs`: UpdateProfile — 임시 config 경로 주입이 어려우므로 ToolCtx의 Arc<Mutex<AppConfig>>
  교체로 테스트.
- `agent.rs`: 기존 mock 테스트는 ToolCtx 시그니처 변경에 맞춰 수정.
- Tool trait 변경: `execute(&self, args: &Value, ctx: &ToolCtx)` — ToolCtx는
  `{ config: Arc<Mutex<AppConfig>>, notify: Arc<dyn Fn(AgentEvent)> }`.
