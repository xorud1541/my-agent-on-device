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

## 핵심 리스크

1. **멀티모달 content 표현/영속화** — `MessageContent` 도입의 블라스트 반경(agent.rs 문자열 접근 지점).
   플랜에서 호출지점 전수 열거로 통제.
2. **2B 가 캐시 경로로 도구 호출** — 발화에 경로를 명시 + 기존 인자 보정 레이어. 수동 테스트로 확인.
