# 스크린샷 영역 캡처 재구현 + 속도 개선 (Codex 실행용)

## 목적
채팅 입력창의 📷 버튼 캡처 기능을 다시 구현한다. 두 가지 핵심 요구가 충족돼야 한다.

1. **현재 모니터 캡처 + 영역 드래그 선택**
   - 캡처 대상은 **현재 모니터**(앱 창 또는 마우스 커서가 위치한 모니터). 지금은 `monitor index 0`
     으로 하드코딩돼 있는데, 멀티모니터에서 엉뚱한 화면을 잡으므로 현재 모니터를 잡도록 고친다.
   - 사용자가 **마우스로 사각형 영역을 드래그**하면 **그 영역만** 캡처되어 첨부돼야 한다.
     (전체 화면이 아니라 선택 영역 결과물이 첨부 썸네일/도구 입력이 된다.)

2. **속도 개선 (중요)**
   - 현재 📷 클릭 후 영역을 선택할 수 있는 화면(모달)이 뜨기까지 **너무 느리다.**
   - 원인 추정: 전체 화면(Retina, 예: 3456×2234) PNG를 디스크에 저장 → 다시 읽기 →
     **전체 해상도 base64 data URL**(수 MB)을 IPC로 프론트에 전달 → 모달 표시. 인코딩+IPC가 병목.
   - 개선 방향(자유롭게 최적화하되 결과 품질 유지):
     - 모달 표시용 **프리뷰는 다운스케일**(예: 화면에 맞는 크기, JPEG 등 가벼운 포맷)로 보내 IPC를
       가볍게. 단, **실제 크롭은 원본 해상도**에서 수행해 화질 보존.
     - 전체 PNG 디스크 왕복/불필요한 재인코딩 최소화(가능하면 인메모리 처리).
     - 앱 hide 후 캡처까지의 고정 대기(현재 180ms)를 필요한 최소로 줄이거나 더 똑똑하게.
   - 체감상 "📷 누르면 거의 즉시 선택 화면이 뜬다" 수준을 목표로 한다.

## 반드시 지킬 제약 (위반 시 앱이 죽음)
- **두 번째 webview 창을 만들지 말 것.** 별도 전체화면 webview 창(`WebviewWindowBuilder`)으로
  오버레이를 띄우면 macOS WebKit이 레이어트리 커밋에서 SIGSEGV로 **앱 전체가 크래시**한다
  (크래시 리포트 `RemoteLayerTreeDrawingAreaProxyMac::displayLink()` 로 확인됨).
  → 영역 선택은 반드시 **메인 창 내부의 React 모달**(단일 webview 유지)로 처리한다.
- **Windows·macOS 동일 동작**이어야 한다. OS별 분기/네이티브 스니핑 도구 사용 금지(단일 코드패스).
- 캡처/크롭 결과는 기존 첨부 파이프라인을 그대로 타야 한다: 크롭 결과 파일 경로가
  `send_message` 의 `attachments` 로 전달되고, vision/기존 이미지 도구(remove_background 등)가
  그 경로를 입력으로 쓴다. 이 계약을 깨지 말 것.
- 원본/프리뷰는 워크스페이스가 아니라 **앱 캐시**(`app_cache_dir()/captures/`)에 둔다.
  도구 산출물만 워크스페이스에 생성(기존 `ensure_in_workspace` 가드 유지).

## 현재 구조 (재구현 대상)
- 백엔드 `src-tauri/src/commands.rs`:
  - `capture_screenshot(app) -> FullCapture{path, data_url, width, height}`: 앱 숨김 →
    `capture_full_to_cache` (xcap monitor[0] 전체 캡처, PNG 저장, 전체 base64 data_url 생성) → 앱 복귀.
  - `crop_capture(full_path, rect{x,y,w,h}) -> CaptureResult{path, thumb_data_url, width, height}`:
    `crop_to_cache` 가 원본 PNG를 열어 원본 픽셀 좌표로 크롭 + 320px 썸네일 base64.
  - 헬퍼: `capture_full_to_cache`, `crop_to_cache`. 구조체 `FullCapture`, `CaptureResult`, `RegionRect`.
  - lib.rs `invoke_handler` 에 `capture_screenshot`, `crop_capture` 등록됨.
- 프론트:
  - `src/components/Composer.tsx`: 📷 클릭 → `invoke("capture_screenshot")` → `pending`(FullCapture)
    설정 → `<RegionOverlay>` 모달 표시 → 모달 `onDone(att)` 로 첨부 추가.
  - `src/components/RegionOverlay.tsx`: 앱 내 전체화면 모달. 캡처 이미지를 `object-fit: contain` 으로
    중앙 표시, 사각형 드래그, 렌더된 `<img>` 의 `getBoundingClientRect` + `naturalWidth/Height` 로
    선택 영역을 **원본 픽셀**로 환산해 `invoke("crop_capture", {fullPath, rect})` 호출.
  - `src/styles/global.css`: `.region-overlay`, `.region-img`, `.region-dim-full`, `.region-sel`,
    `.region-hint` 스타일.
  - 첨부 칩/말풍선 썸네일: `Composer.tsx`, `MessageView.tsx`, `src/types.ts`(UserMessage.images).

## 의존성/환경
- Rust: `xcap = 0.9`(화면 캡처), `image = 0.25`(인코딩/크롭/썸네일), `base64 = 0.22`. Tauri 2.
- 프론트: React 19 + TS + Vite, `@tauri-apps/api`.
- 빌드 검증: `cd src-tauri && cargo build` / 루트 `npx tsc --noEmit` / `pnpm build`.
- 단위 테스트는 모델 없이 도는 것만(`cargo test --lib`). macOS에서 Windows 경로 테스트 7개는
  사전 존재 실패이므로 무시(신규 추가 테스트만 통과하면 됨).

## 완료 기준 (acceptance)
1. `cargo build`, `npx tsc --noEmit`, `pnpm build` 모두 성공.
2. 📷 → 현재 모니터가 잡히고, 드래그한 사각형 영역만 잘려 첨부된다.
3. 드래그 영역과 잘린 결과가 **Retina/스케일 환경에서도 픽셀 단위로 일치**한다.
4. 📷 클릭 후 선택 모달이 뜨기까지 체감상 빠르다(전체 해상도 base64 IPC 병목 제거).
5. 두 번째 webview 창을 만들지 않는다(단일 창). Windows에서도 동일하게 동작하는 코드.
6. 크롭 결과 경로가 기존 첨부/vision/도구 파이프라인에 그대로 연결된다.

## 참고
- 설계/이력: `docs/superpowers/specs/2026-06-12-screenshot-vision-design.md` (Addendum 2에 크래시
  원인과 단일-창 결정이 기록돼 있음).
- 변경은 캡처 관련 파일에 한정하고, 에이전트 루프/도구/세션 등 다른 영역은 건드리지 말 것.
