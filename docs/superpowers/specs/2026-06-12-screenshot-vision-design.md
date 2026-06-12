# 스크린샷 캡처 + Vision 이미지 분석 — 설계

작성일: 2026-06-12
브랜치: `feat/screenshot-vision`
선행 검증: Qwen3.5-2B + `mmproj-Qwen3.5-2B-BF16.gguf` 로 llama-server(b9430, Metal) vision 동작 수동 확인 완료
(파란 사각형/빨간 원/"VISION 7" 텍스트 정확 인식, image_url base64, 102 토큰 응답)

## 목표

채팅 입력창 옆 스크린샷 버튼으로 화면을 캡처해 첨부하고, 그 이미지로 다음을 할 수 있게 한다:

- "이미지 설명해줘" / "이 이미지 뭐라고 적혀있니?" → vision 모델이 직접 답변
- "이 이미지 배경제거해줘" / "저장해줘" / 변환 등 → 기존 이미지 도구가 캡처 파일을 처리, 산출물은 워크스페이스에 생성
- 기존 이미지 관련 도구 지원은 그대로 유지
- Windows / macOS 양쪽에서 동일하게 동작

## 결정 사항 (확정)

- 캡처 방식: **전체 화면(주 모니터)** — `xcap` 사용 (기존 의존성). 영역 드래그 선택은 범위 밖.
- 원본 저장: **앱 캐시(`app_cache_dir`/captures/)** 에 임시 저장. 도구 산출물(배경제거 결과 등)만 워크스페이스에 생성.
- 캡처 흐름: 앱 hide → 캡처 → show.

## 데이터 흐름

```
[📷 버튼] → hide_window → xcap 주모니터 캡처 → show_window
   → 원본 PNG를 앱 캐시(app_cache_dir/captures/)에 저장
   → 다운스케일 썸네일(base64) + 원본 경로를 프론트로 반환
   → 입력창 위 첨부 칩(썸네일)으로 표시
[전송] → send_message(text, attachments:[캐시경로])
   → user ChatMessage = 멀티모달 [{text}, {image_url}]  (+ text 에 경로 명시)
   → run_turn → 전송 직전 image_url(로컬경로)를 base64 로 치환해 llama-server 로
   → vision 질문이면 모델이 직접 답 / "배경제거·저장"이면 경로로 기존 도구 호출
   → 도구 산출물은 워크스페이스에 생성
```

## 1. 백엔드 — 캡처 커맨드 & 윈도우 제어

새 Tauri 커맨드 `capture_screenshot(app) -> CaptureResult`:

```rust
struct CaptureResult { path: String, thumb_data_url: String, width: u32, height: u32 }
```

- `app.get_webview_window("main")` → `.hide()` → 짧은 지연(~150ms, 렌더 프레임 빠짐 보장)
  → `xcap::Monitor` 주모니터 `.capture_image()` → `.show()` + `.set_focus()`
- 원본은 `app.path().app_cache_dir()?/captures/capture_<timestamp>.png` 에 저장 (워크스페이스 아님)
- 썸네일은 백엔드에서 ~320px 로 다운스케일해 base64 data URL 반환
  → 프론트는 asset-protocol scope 설정 없이 바로 `<img src>` 가능 (크로스플랫폼 최단 경로)
- **show 보장 가드**: 캡처/저장 실패 시에도 반드시 `show()` 가 호출되도록 guard 패턴
  (앱이 숨은 채 갇히는 사고 방지)

기존 `screen_capture` 에이전트 도구는 그대로 유지(발화 기반 경로). 버튼은 UI 주도 경로로 별개 공존.

## 2. 백엔드 — 멀티모달 메시지 & Vision 연결 (가장 큰 변경)

`ChatMessage.content` 를 `Option<String>` → `Option<MessageContent>` 로 확장:

```rust
#[serde(untagged)]
enum MessageContent { Text(String), Parts(Vec<Part>) }

#[serde(tag = "type")]
enum Part {
    #[serde(rename = "text")]      Text  { text: String },
    #[serde(rename = "image_url")] Image { image_url: ImageUrl }, // url = 로컬 캐시 경로
}

struct ImageUrl { url: String }
```

- `untagged` serde → 텍스트 메시지는 기존처럼 문자열로 직렬화(하위호환), 이미지 메시지만 배열.
  세션 JSON 라운드트립 유지.
- 블라스트 반경 축소: `MessageContent::text(&self) -> Option<&str>` 접근자를 추가하고,
  agent.rs 의 문자열 접근 지점(clip / 요약 / `refresh_system_prompt` / `enforce_history_budget`)을
  이 접근자로 마이그레이션. **구현 플랜에서 호출지점을 전수 열거한다.**
- **영속화는 경로 참조로**(base64 아님) → 세션 파일이 수백 KB base64 로 붓는 것 방지.
  **전송 직전**에만 `image_url.url` 이 로컬 경로면 파일을 읽어 `data:image/png;base64,…` 로 치환
  (client.rs 직전 단계). 캐시가 비워져 파일이 없으면 그 이미지 파트는 드롭(컨텍스트 자연 감소).
- `mmproj_path` 를 `AppConfig` 에 추가:
  - OS별 기본값 + 모델과 같은 폴더의 `mmproj-*.gguf` 자동 페어링
  - `server.rs` 에서 값이 있을 때만 `--mmproj` 부착 (device 분기와 동일 패턴, 빈 값이면 인자 생략)
  - 이 부분은 수동 검증 완료.

## 3. 백엔드 — 기존 이미지 도구 연동

- vision 질문("설명/뭐라 적혀있니")은 도구 없이 모델이 직접 답변.
- "배경제거/저장/변환"은 모델이 **캡처 캐시 경로를 입력으로** 기존 도구(`remove_background` 등) 호출.
  출력은 워크스페이스 기본 (산출물=워크스페이스 요구 충족).
- user 메시지 text 에 캡처 절대경로를 명시(`[첨부 이미지: <경로>]`)해,
  시스템 프롬프트의 "경로 없으면 워크스페이스" 규칙과 충돌 없이 2B 가 캐시 경로를 도구 인자로 넘기게 함.
  (2B 인자 보정 레이어와 함께 동작 — 테스트로 확인)
- `remove_background` 등의 입력 읽기는 캐시에서, 출력은 `ensure_in_workspace` 그대로 통과
  → 워크스페이스 가드 우회 예외 불필요.

## 4. 프론트엔드 — 버튼/첨부/말풍선

- `Composer.tsx`: textarea 옆 📷 버튼. 클릭 → `invoke("capture_screenshot")`
  → 반환 썸네일을 입력창 위 **첨부 칩**(썸네일 + ✕제거)으로 표시. 첨부는 배열 → 여러 번 찍어 누적 가능.
- `useAgent.send(text, attachments)` 로 시그니처 확장 → `send_message` 에 `attachments: string[]`(캐시 경로) 전달.
  `UserMessage` 에 `images?: { thumb: string; path: string }[]` 추가.
- `MessageView.tsx` 의 `msg-user`: 자연어 텍스트 + 썸네일을 한 말풍선에 묶어 렌더
  (요구사항: "말풍선에 자연어와 같이 묶어서").
- 세션 복원(`chatToUi`): Parts 에서 text/이미지 경로 추출. 캐시가 비워졌으면 썸네일 대신 "📷 캡처" 플레이스홀더.

## 5. Windows 정합성

- `xcap` · 윈도우 `hide/show` · base64 썸네일 모두 크로스플랫폼. `app_cache_dir` 도 Tauri 가 OS별 처리.
- 영역 선택을 안 쓰기로 한 결정 덕에 Windows 전용 오버레이 구현 회피 → 양 OS 동일 코드패스.
- 주의: 일부 환경에서 `hide()` 직후 즉시 캡처하면 창이 프레임에 남을 수 있어
  **소폭 지연 + show 보장 가드**로 처리. 구현 플랜에서 양 OS 실측.

## 6. 에러 처리 / 엣지

- 캡처 실패 → 반드시 `show()` 복구 후 사용자에게 칩 대신 에러 토스트.
- mmproj 미로딩 모델인데 이미지 첨부 시 → 순수 vision 질문은 "이 모델은 이미지 분석 미지원" 안내,
  경로 기반 도구는 정상 동작(graceful degrade). 기본 배포는 2B+mmproj 자동 페어링이라 평시엔 동작.
- 컨텍스트 예산: 이미지(~1K 토큰)는 해당 user 턴에만 붙고 history budget 으로 자연 축출.

## 7. 테스트

- Rust 단위:
  - `MessageContent` serde 라운드트립(문자열 ↔ 배열)
  - 전송 직전 경로 → base64 치환, 파일 없으면 파트 드롭
  - 캐시 경로 입력 + 워크스페이스 출력 가드(`ensure_in_workspace`) 통과
- 수동(양 OS):
  - 버튼 → hide/show → 썸네일 표시
  - "뭐라 적혀있니"(vision 직답)
  - "배경제거"(도구 → 워크스페이스 산출물)
  - 세션 저장/복원

## 8. 범위 밖 (YAGNI)

- ~~영역 드래그 선택~~ → **범위 내로 변경됨 (아래 Addendum 참고)**
- 임의 파일 첨부(파일 피커), 이미지 외 첨부
- 멀티모니터 선택(이번엔 주 모니터 고정)

## Addendum (2026-06-12): 전체화면 → 영역 드래그 선택

사용자 요청으로 캡처 방식을 전체화면에서 **영역 드래그 선택**으로 변경. Windows·macOS 동일
동작이 필수라, OS별 네이티브 도구 대신 **오버레이 + 크롭 단일 코드패스**를 채택.

흐름: 앱 숨김 → `xcap` 으로 주 모니터 전체 캡처(캐시 저장) → **불투명 전체화면 오버레이 창**
(`region-overlay`)이 그 스크린샷을 꽉 채워 표시(얼어붙은 화면처럼 보임) → 사용자가 사각형 드래그
→ 선택 영역(뷰포트 논리 px)을 `region_finish` 로 전달 → 백엔드가 물리 픽셀로 환산해 크롭 →
크롭 결과만 캐시에 저장하고 첨부. Esc/우클릭/창닫기는 `region_cancel`(또는 120초 타임아웃)으로
취소 → `capture_screenshot` 이 `Ok(None)` 반환, 첨부 없음.

- 투명창 미사용(스크린샷이 불투명하게 화면을 덮음) → macOS `macOSPrivateApi` 불필요, 양 OS 동일.
- DPI 보정: 크롭 시 `물리폭/뷰포트폭`, `물리높이/뷰포트높이` 비율로 x/y 독립 스케일.
- 동기화: `oneshot` 채널을 AppState(`region_tx`)에 저장, 오버레이의 finish/cancel 이 깨운다.
- 오버레이 창은 같은 번들을 `index.html?overlay=1` 로 로드, `main.tsx` 가 `RegionOverlay` 렌더.
  `region-overlay` 라벨을 capability `windows` 에 추가.

### Addendum 2 (2026-06-12): 두 번째 webview 창 → 앱 내 모달

별도 전체화면 webview 창(`region-overlay`) 방식은 **macOS에서 WebKit 크래시**를 일으켰다.
크래시 리포트: `RemoteLayerTreeDrawingAreaProxyMac::displayLink()` 에서 SIGSEGV(메인 스레드,
KERN_INVALID_ADDRESS). 📷 클릭으로 두 번째 WKWebView 창을 만드는 순간 레이어트리 커밋이
널을 참조하며 앱 전체가 종료됐다. 두 번째 webview 창 생성 자체가 근본 원인.

해결: **두 번째 창을 만들지 않는다(단일 webview 유지).** 영역 선택을 메인 창 내부의
전체화면 React 모달(`RegionOverlay`)로 처리한다.

- `capture_screenshot`: 앱 숨김 → 주 모니터 전체 캡처(캐시) → 앱 복귀, `FullCapture{path,
  data_url, width, height}` 반환. 오버레이 창/oneshot/capability 모두 제거.
- 프론트: 반환된 전체 스크린샷을 앱 내 모달에 띄우고(중앙 contain), 드래그로 사각형 선택.
- 좌표 환산: 렌더된 `<img>` 의 `getBoundingClientRect` 와 `naturalWidth/Height` 로 선택 영역을
  **원본 픽셀**로 환산해 `crop_capture(full_path, rect{x,y,w,h})` 호출 → 크롭 결과만 첨부.
- 취소: Esc/우클릭/너무 작은 선택은 모달만 닫음.
- 트레이드오프: 실제 화면 위가 아니라 앱 창 안에 띄운 스크린샷 위에서 선택하지만, 단일 webview라
  크래시가 없고 Win/macOS 동일 동작.

### Addendum 3 (2026-06-13): 자체 네이티브 오버레이 영역 캡처 ★현행

**Addendum 2의 "앱 내 모달" 방식을 폐기하고, 완전 자체 구현 네이티브 오버레이로 바꾼다.**
(OS 기본 캡처 도구 호출안 — macOS `screencapture -i` / Windows `ms-screenclip:`+클립보드 — 도
검토했으나, Windows 취소 감지 불가·클립보드 폴링·OS별 UX 편차 때문에 기각. 사용자 결정.)

목표 UX (사용자 요구):
1. 📷(캡처 버튼) 클릭 시 **전체 화면에 살짝 음영**이 깔린다 (실제 화면 위, 앱 안이 아님).
2. 사용자가 그 위에서 **드래그로 영역을 지정**한다. 지정 영역은 밝게, 바깥은 어둡게.
3. 지정한 영역만 캡처본이 된다. Esc/우클릭 = 취소.
4. 캡처본(썸네일)은 입력창 위에 첨부된다. (기존 유지)
5. 이전 방식보다 빨라야 한다.

아키텍처 — **별도 프로세스 네이티브 오버레이 (`region-capture` 헬퍼 바이너리)**:
- 같은 Cargo 패키지의 두 번째 바이너리(`src-tauri/src/bin/region_capture.rs`).
  `winit`(창/입력) + `softbuffer`(소프트웨어 렌더) + `xcap`(캡처) — **webview 를 전혀 쓰지 않는
  순수 네이티브 창**이라 Addendum 2의 WebKit 크래시와 무관하고, **별도 프로세스라 헬퍼가 어떤
  이유로 죽어도 본 앱은 절대 죽지 않는다** (크래시 격리).
- winit/softbuffer/xcap 모두 크로스플랫폼 → **macOS·Windows 단일 코드패스**. 클립보드/OS 도구
  의존 없음, 취소 감지 명확(프로세스 즉시 종료).

헬퍼 동작 (`region-capture <out.png> <hint_x> <hint_y>`):
1. 힌트 좌표(본 앱 창이 있는 모니터 중심)가 속한 모니터를 `xcap` 으로 캡처 (인메모리, base64/IPC 없음).
2. 그 모니터에 **풀스크린 borderless + always-on-top** 네이티브 창을 띄우고, 캡처본을 어둡게
   (밝기 ~55%) 깔아 "화면에 음영이 깔린" 모습을 만든다. 커서는 crosshair.
3. 마우스 드래그 동안 선택 영역만 원본 밝기로, 테두리는 앱 액센트(amber)로 그린다
   (softbuffer 로 어두운/밝은 프레임 버퍼를 미리 만들어 행 단위 복사 — 드래그 60fps급).
4. 드래그 종료 시 선택 영역을 **원본 해상도에서 크롭**해 `<out.png>` 저장 후 즉시 종료(exit 0).
   좌표는 물리 픽셀 1:1(창=모니터=캡처 해상도)이라 Retina/스케일에서도 정확.
5. Esc/우클릭/5px 미만 선택 = 저장 없이 종료 → 본 앱은 "파일 없음 = 취소"로 판정.

본 앱 (`commands.rs`):
- `capture_region(app) -> Result<Option<CaptureResult>, String>` 단일 커맨드.
  - 앱 창이 있는 모니터 중심 좌표 계산 → 앱 `hide()` → 짧은 대기(화면에서 앱 제거)
    → 헬퍼 실행(블로킹, `spawn_blocking`) → 앱 `show()`/`set_focus()`.
  - `<out.png>` 존재 → 320px 썸네일 생성 후 `CaptureResult{path, thumb_data_url, width, height}`.
    없으면(취소) `Ok(None)`.
  - 헬퍼 경로: 본 앱 실행 파일과 같은 폴더의 `region-capture(.exe)` (cargo 가 두 바이너리를
    같은 target 폴더에 빌드. 릴리즈 번들에는 같은 위치에 동봉 — 패키징 단계에서 처리).
- 캡처 원본/결과는 앱 캐시(`app_cache_dir()/captures/`), 도구 산출물만 워크스페이스(기존 가드).

속도: 전체 화면 base64 IPC(구방식 병목)가 완전히 사라지고, 헬퍼가 인메모리로 캡처→오버레이→크롭만
수행. 본 앱↔헬퍼 간 데이터는 "출력 파일 경로" 하나.

제거되는 것 (Addendum 2 산물): 앱 내 모달 `RegionOverlay.tsx`, `capture_to_cache`(full+프리뷰),
`crop_capture`, 정규화 좌표 환산, `.region-*` CSS.

프론트 / 버튼 디자인:
- `Composer` 의 캡처 버튼을 이모지(📷)에서 **모던·심플 SVG 아이콘**(영역 선택 프레임 — 모서리
  브래킷 + 중앙 점, `currentColor` 스트로크)으로 교체. 평상시 잉크 색 고스트 버튼, hover 시
  amber 액센트 — 앱의 다크/amber 톤에 맞춤. 캡처 진행 중에는 비활성 + 펄스.
- 📷 → `invoke("capture_region")` → 결과 있으면 첨부 칩 추가, `null` 이면 무시(취소). 그 외
  첨부 칩/말풍선 썸네일/세션 복원은 기존 그대로.

크로스플랫폼 (필수 — 둘 다 동작):
- 오버레이 헬퍼는 단일 코드(winit/softbuffer/xcap 모두 양 OS 지원). OS 분기는
  헬퍼 실행 파일명(.exe)과 macOS activation policy(Accessory — 독 아이콘 미표시) 정도.
- Windows: 헬퍼에 `windows_subsystem = "windows"`(콘솔 창 없음), always-on-top 으로 작업표시줄
  위 덮음. 릴리즈(NSIS) 번들에 헬퍼 동봉 필수.
- macOS: 화면 기록 권한은 본 앱(부모)의 TCC 책임으로 귀속 — 기존 screen_capture 도구와 동일.

완료 기준:
1. `cargo build`(두 바이너리) / `npx tsc --noEmit` / `pnpm build` 성공.
2. 📷 → 실제 화면에 음영 + 드래그(자체 오버레이), 지정 영역만 첨부. Retina에서 픽셀 정확.
3. 이전(앱 내 모달)보다 체감상 빠르다.
4. 취소(Esc/우클릭) 시 첨부 없이 즉시 복귀. 헬퍼가 죽어도 본 앱 크래시 없음(프로세스 격리).
5. 캡처 결과 경로가 기존 첨부/vision/도구 파이프라인에 그대로 연결.
6. Windows 동일 동작(단일 코드패스) — Windows 환경에서 실측 검증.

## 핵심 리스크

1. **멀티모달 content 표현/영속화** — `MessageContent` 도입의 블라스트 반경(agent.rs 문자열 접근 지점).
   플랜에서 호출지점 전수 열거로 통제.
2. **2B 가 캐시 경로로 도구 호출** — 발화에 경로를 명시 + 기존 인자 보정 레이어. 수동 테스트로 확인.
