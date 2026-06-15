# 스크린샷 캡처 + Vision 이미지 분석 구현 플랜

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 채팅 입력창의 📷 버튼으로 화면을 캡처해 첨부하고, vision 모델로 그 이미지를 설명/판독하거나 기존 이미지 도구(배경제거/변환/저장)로 처리한다. macOS·Windows 동일 동작.

**Architecture:** `ChatMessage.content`(`Option<String>`) 타입은 그대로 두고, 첨부 이미지의 **로컬 캐시 경로만** 담는 병렬 필드 `images: Option<Vec<String>>`를 추가한다. 세션 JSON에는 경로만 저장(작은 파일)되고, **llama-server로 보내기 직전 client.rs에서만** 그 경로를 읽어 base64 `image_url` 멀티모달 배열로 변환한다. 캡처 원본은 앱 캐시에 저장하고, 도구 산출물만 워크스페이스에 생성한다. mmproj는 모델과 같은 폴더에서 자동 페어링한다.

**Tech Stack:** Rust / Tauri v2 / `xcap`(화면 캡처) / `image` 0.25(썸네일) / `base64` 0.22 / React + TypeScript / llama-server(mtmd vision, `--mmproj`).

---

## 파일 구조

| 파일 | 책임 | 변경 |
|---|---|---|
| `src-tauri/Cargo.toml` | 의존성 | `base64 = "0.22"` 추가 |
| `src-tauri/src/config.rs` | 앱 설정 | `mmproj_path: String` 필드 |
| `src-tauri/src/llm/server.rs` | llama-server 기동 | `resolve_mmproj()` + `--mmproj` 부착 |
| `src-tauri/src/models.rs` | ChatMessage | `images` 필드 + `user_with_images()` |
| `src-tauri/src/llm/client.rs` | LLM 요청 | 전송 직전 멀티모달 변환 + `vision_enabled` |
| `src-tauri/src/commands.rs` | Tauri 커맨드 | `capture_screenshot`, `send_message` attachments, set_config |
| `src-tauri/src/lib.rs` | 핸들러 등록 | `capture_screenshot` 등록 |
| `src-tauri/src/agent.rs` | 시스템 프롬프트 | 첨부 이미지 처리 규칙 1줄 |
| `src/types.ts` | 프론트 타입 | `UserMessage.images`, `ChatMessage.images`, `AppConfig.mmproj_path` |
| `src/hooks/useAgent.ts` | 이벤트→메시지 | `send(text, attachments)` |
| `src/components/Composer.tsx` | 입력창 | 📷 버튼 + 첨부 칩 |
| `src/components/MessageView.tsx` | 말풍선 | user 말풍선 썸네일 |
| `src/lib/restore.ts` | 세션 복원 | 마커 제거 + 이미지 플레이스홀더 |
| `src/styles/global.css` | 스타일 | 캡처 버튼/칩/썸네일 CSS |

작업 순서: 백엔드(Task 1~6) → 프론트(Task 7~10) → 수동 검증(Task 11). 각 백엔드 로직 Task는 TDD, 캡처/프론트는 빌드+수동 검증.

---

## Task 1: mmproj 설정 + 자동 페어링 + `--mmproj` 부착

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/config.rs` (구조체 + Default)
- Modify: `src-tauri/src/llm/server.rs`

- [ ] **Step 1: base64 의존성 추가**

`src-tauri/Cargo.toml` 의 `[dependencies]` 블록 끝(`uuid` 줄 다음)에 추가:

```toml
base64 = "0.22"
```

- [ ] **Step 2: config 에 mmproj_path 필드 추가**

`src-tauri/src/config.rs` 의 `AppConfig` 구조체에서 `removebg_model` 필드 바로 아래에 추가:

```rust
    /// 배경제거 ONNX 모델 경로
    pub removebg_model: String,
    /// 멀티모달 vision 프로젝터(mmproj) 경로. 빈 값이면 모델과 같은 폴더의
    /// `mmproj-*.gguf` 를 자동 페어링한다.
    pub mmproj_path: String,
```

`Default for AppConfig` 의 `removebg_model: default_removebg_model(),` 아래에 추가:

```rust
            removebg_model: default_removebg_model(),
            mmproj_path: String::new(),
```

- [ ] **Step 3: resolve_mmproj 의 실패 테스트 작성**

`src-tauri/src/llm/server.rs` 파일 맨 아래에 테스트 모듈을 추가:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_mmproj_auto_pairs_sibling() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model-Q4.gguf"), "m").unwrap();
        std::fs::write(dir.path().join("mmproj-model-BF16.gguf"), "p").unwrap();
        let model = dir.path().join("model-Q4.gguf").to_string_lossy().into_owned();
        let got = resolve_mmproj(&model, "").unwrap();
        assert!(got.file_name().unwrap().to_string_lossy().to_lowercase().starts_with("mmproj"));
    }

    #[test]
    fn resolve_mmproj_prefers_configured_when_exists() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("custom-mmproj.gguf");
        std::fs::write(&cfg_path, "p").unwrap();
        std::fs::write(dir.path().join("mmproj-auto.gguf"), "p").unwrap();
        let got = resolve_mmproj("/any/model.gguf", &cfg_path.to_string_lossy()).unwrap();
        assert_eq!(got, cfg_path);
    }

    #[test]
    fn resolve_mmproj_none_when_no_sibling() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model-Q4.gguf"), "m").unwrap();
        let model = dir.path().join("model-Q4.gguf").to_string_lossy().into_owned();
        assert!(resolve_mmproj(&model, "").is_none());
    }
}
```

- [ ] **Step 4: 테스트 실패 확인**

Run: `cd src-tauri && cargo test --lib resolve_mmproj`
Expected: 컴파일 에러 (`resolve_mmproj` 미정의)

- [ ] **Step 5: resolve_mmproj 구현 + start() 에 --mmproj 부착**

`src-tauri/src/llm/server.rs` 상단 `use` 아래(예: `use std::time::Duration;` 다음 줄)에 추가:

```rust
use std::path::{Path, PathBuf};
```

파일 안(`impl LlamaServer` 위 또는 아래, 모듈 함수로) 추가:

```rust
/// 사용할 mmproj 경로를 결정한다. 설정값이 있으면 그것을(존재할 때만),
/// 없으면 모델 파일과 같은 폴더의 `mmproj-*.gguf` 를 자동 페어링한다.
pub fn resolve_mmproj(model_path: &str, configured: &str) -> Option<PathBuf> {
    if !configured.trim().is_empty() {
        let p = PathBuf::from(configured);
        return p.exists().then_some(p);
    }
    let dir = Path::new(model_path).parent()?;
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| {
                        let n = n.to_lowercase();
                        n.starts_with("mmproj") && n.ends_with(".gguf")
                    })
                    .unwrap_or(false)
        })
}
```

`start()` 안에서 첫 `cmd.args([...])`(`"--no-webui"` 로 끝나는 블록) **바로 다음**에 추가:

```rust
        // 멀티모달(vision) 프로젝터: 설정값 또는 모델과 같은 폴더의 mmproj-*.gguf 자동 페어링
        if let Some(mmproj) = resolve_mmproj(&cfg.model_path, &cfg.mmproj_path) {
            let mmproj_str = mmproj.to_string_lossy().into_owned();
            cmd.args(["--mmproj", &mmproj_str]);
        }
```

- [ ] **Step 6: 테스트 통과 확인**

Run: `cd src-tauri && cargo test --lib resolve_mmproj`
Expected: 3 passed

- [ ] **Step 7: 커밋**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/config.rs src-tauri/src/llm/server.rs
git commit -m "$(printf 'feat: mmproj 자동 페어링 + --mmproj 부착\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 2: ChatMessage 에 images 필드 + user_with_images 생성자

**Files:**
- Modify: `src-tauri/src/models.rs`

- [ ] **Step 1: serde 라운드트립 실패 테스트 작성**

`src-tauri/src/models.rs` 파일 맨 아래에 추가:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_roundtrip_through_json() {
        let m = ChatMessage::user_with_images("이거 설명해줘", vec!["/cache/a.png".into()]);
        let s = serde_json::to_string(&m).unwrap();
        let back: ChatMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(back.images.as_deref(), Some(&["/cache/a.png".to_string()][..]));
        // 텍스트에는 첨부 경로 마커가 포함된다 (2B 가 도구 인자로 쓸 경로)
        assert!(back.content.as_deref().unwrap().contains("/cache/a.png"));
    }

    #[test]
    fn text_only_message_has_no_images_field() {
        let m = ChatMessage::user("안녕");
        let s = serde_json::to_string(&m).unwrap();
        assert!(!s.contains("images"), "텍스트 전용 메시지는 images 키가 없어야 함: {s}");
    }

    #[test]
    fn user_with_images_embeds_path_marker() {
        let m = ChatMessage::user_with_images("배경 제거해줘", vec!["/c/x.png".into(), "/c/y.png".into()]);
        let c = m.content.as_deref().unwrap();
        assert!(c.starts_with("배경 제거해줘"));
        assert!(c.contains("[첨부 이미지: /c/x.png, /c/y.png]"));
    }
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cd src-tauri && cargo test --lib models::`
Expected: 컴파일 에러 (`user_with_images` 미정의, `images` 필드 없음)

- [ ] **Step 3: images 필드 추가 + 생성자 4개 갱신 + 신규 생성자**

`src-tauri/src/models.rs` 의 `ChatMessage` 구조체에 `tool_call_id` 필드 다음으로 추가:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 첨부 이미지의 로컬(캐시) 경로. 세션에는 경로만 저장되고,
    /// llama-server 로 보낼 때만 base64 image_url 로 인라인된다(client.rs).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,
```

기존 생성자 `system`, `user`, `assistant`, `tool` 4개의 `Self { ... }` 리터럴에 각각 `images: None,` 을 추가한다. 예 (`user`):

```rust
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            images: None,
        }
    }
```

(`system`, `assistant`, `tool` 도 동일하게 `images: None,` 추가)

`tool` 생성자 다음에 신규 생성자를 추가:

```rust
    /// 첨부 이미지가 있는 사용자 메시지. content 텍스트 끝에 경로 마커를 붙여
    /// 모델이 경로 기반 도구(remove_background 등)를 호출할 수 있게 한다.
    pub fn user_with_images(text: impl Into<String>, images: Vec<String>) -> Self {
        let text = text.into();
        let marker = format!("\n\n[첨부 이미지: {}]", images.join(", "));
        Self {
            role: "user".into(),
            content: Some(format!("{text}{marker}")),
            tool_calls: None,
            tool_call_id: None,
            images: Some(images),
        }
    }
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cd src-tauri && cargo test --lib models::`
Expected: 3 passed

- [ ] **Step 5: 전체 라이브러리 컴파일 확인 (생성자 호출처 영향 없음 확인)**

Run: `cd src-tauri && cargo build --lib`
Expected: 성공 (기존 `ChatMessage::user` 등 호출처는 변경 불필요)

- [ ] **Step 6: 커밋**

```bash
git add src-tauri/src/models.rs
git commit -m "$(printf 'feat: ChatMessage 에 첨부 이미지 경로 필드(images) 추가\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 3: 전송 직전 멀티모달 변환 + vision_enabled 플래그

**Files:**
- Modify: `src-tauri/src/llm/client.rs`

- [ ] **Step 1: 변환 함수 실패 테스트 작성**

`src-tauri/src/llm/client.rs` 의 기존 `#[cfg(test)] mod tests` 가 있으면 그 안에, 없으면 파일 맨 아래에 다음 테스트 모듈을 추가:

```rust
#[cfg(test)]
mod multimodal_tests {
    use super::*;
    use crate::models::ChatMessage;

    #[test]
    fn image_message_becomes_multimodal_when_vision_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.png");
        std::fs::write(&p, b"\x89PNG-fake-bytes").unwrap();
        let m = ChatMessage::user_with_images("설명해줘", vec![p.to_string_lossy().into_owned()]);
        let v = message_to_request_value(&m, true);
        let content = v.get("content").unwrap().as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert!(content[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert!(v.get("images").is_none(), "요청에는 내부 images 키가 없어야 함");
    }

    #[test]
    fn image_message_stays_text_when_vision_disabled() {
        let m = ChatMessage::user_with_images("설명해줘", vec!["/no/such.png".into()]);
        let v = message_to_request_value(&m, false);
        assert!(v.get("content").unwrap().is_string(), "vision 꺼짐: content 는 문자열 유지");
        assert!(v.get("images").is_none());
    }

    #[test]
    fn missing_image_file_is_dropped_not_errored() {
        let m = ChatMessage::user_with_images("설명", vec!["/definitely/missing.png".into()]);
        let v = message_to_request_value(&m, true);
        // 파일이 없으면 image_url 파트는 빠지고 text 파트만 남는다
        let content = v.get("content").unwrap().as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[test]
    fn plain_text_message_unchanged() {
        let m = ChatMessage::user("그냥 텍스트");
        let v = message_to_request_value(&m, true);
        assert!(v.get("content").unwrap().is_string());
    }
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cd src-tauri && cargo test --lib message_to_request_value`
Expected: 컴파일 에러 (`message_to_request_value` 미정의)

- [ ] **Step 3: 변환 함수 구현**

`src-tauri/src/llm/client.rs` 상단 `use` 에 추가:

```rust
use base64::Engine as _;
```

파일 안(모듈 함수로, `impl` 밖) 추가:

```rust
/// ChatMessage 를 llama-server /v1/chat/completions 요청용 JSON 으로 변환한다.
/// images 가 있고 vision 이 켜져 있으면 content 를 멀티모달 배열로 바꾸고
/// 파일을 base64 image_url 로 인라인한다. 그 외에는 기존 직렬화 그대로.
pub(crate) fn message_to_request_value(m: &crate::models::ChatMessage, vision_enabled: bool) -> Value {
    let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut v {
        map.remove("images"); // 내부 전용 필드 — 서버로 보내지 않는다
    }
    let Some(paths) = m.images.as_ref().filter(|p| !p.is_empty()) else {
        return v;
    };
    if !vision_enabled {
        return v; // 모델에 vision 없음 — 경로 마커가 든 텍스트만 전송(도구는 동작)
    }
    let mut parts: Vec<Value> = Vec::new();
    if let Some(text) = &m.content {
        parts.push(serde_json::json!({ "type": "text", "text": text }));
    }
    for p in paths {
        let Ok(bytes) = std::fs::read(p) else { continue }; // 캐시 비움 등 → 생략
        let lower = p.to_lowercase();
        let mime = if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else {
            "image/png"
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{mime};base64,{b64}") }
        }));
    }
    if let Value::Object(map) = &mut v {
        map.insert("content".into(), Value::Array(parts));
    }
    v
}
```

- [ ] **Step 4: HttpLlmClient 에 vision_enabled 추가 + 요청 본문 변환 적용**

`HttpLlmClient` 구조체 정의에 필드 추가:

```rust
pub struct HttpLlmClient {
    pub base_url: String,
    pub max_tokens: u32,
    /// 로드된 모델에 mmproj(vision)가 붙어 있는지. 꺼지면 이미지 파트를 전송하지 않는다.
    pub vision_enabled: bool,
    http: reqwest::Client,
}
```

`new` 시그니처/본문 갱신:

```rust
    pub fn new(base_url: String, max_tokens: u32, vision_enabled: bool) -> Self {
        Self { base_url, max_tokens, vision_enabled, http: reqwest::Client::new() }
    }
```

`complete()` 안 `let body = serde_json::json!({ ... "messages": messages, ... });` 에서 `"messages": messages,` 를 다음으로 교체:

```rust
            "messages": messages.iter()
                .map(|m| message_to_request_value(m, self.vision_enabled))
                .collect::<Vec<_>>(),
```

- [ ] **Step 5: 기존 HttpLlmClient::new 호출처 갱신**

`src-tauri/src/llm/client.rs` 내 테스트 4곳(`HttpLlmClient::new(base_url, 1024)`)을 `HttpLlmClient::new(base_url, 1024, false)` 로 바꾼다. (다음 줄 번호 기준이지만 정확 매칭은 `HttpLlmClient::new(base_url, 1024)` 문자열 4개 전부) 

- [ ] **Step 6: 테스트 통과 확인**

Run: `cd src-tauri && cargo test --lib`
Expected: 신규 4개 포함 전부 통과. (commands.rs 의 호출처는 Task 5 에서 갱신 — 지금은 `cargo test --lib` 의 client/models/server 테스트만 본다. 만약 `cargo build` 가 commands.rs 때문에 깨지면 Step 7 전에 Task 5 Step 4 를 먼저 적용)

> 주의: 이 시점에서 `cargo build` 는 commands.rs 의 `HttpLlmClient::new(base_url, max_output_tokens)`(인자 2개) 때문에 실패한다. Task 5 에서 인자 3개로 갱신한다. 컴파일 일관성을 위해 Task 5 Step 4 를 이어서 바로 진행할 것.

- [ ] **Step 7: 커밋**

```bash
git add src-tauri/src/llm/client.rs
git commit -m "$(printf 'feat: 전송 직전 첨부 이미지를 base64 멀티모달로 변환\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 4: capture_screenshot 커맨드 (윈도우 hide/show + 캐시 저장 + 썸네일)

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: CaptureResult + 캡처 헬퍼 + 커맨드 추가**

`src-tauri/src/commands.rs` 상단 `use base64::Engine as _;` 가 없으면 추가:

```rust
use base64::Engine as _;
```

`ModelEntry` 구조체 정의 아래에 추가:

```rust
#[derive(Debug, Serialize)]
pub struct CaptureResult {
    pub path: String,
    pub thumb_data_url: String,
    pub width: u32,
    pub height: u32,
}

/// 주 모니터를 캡처해 앱 캐시에 저장하고, 320px 썸네일(base64)과 경로를 돌려준다.
fn capture_primary_to_cache(cache_dir: std::path::PathBuf) -> Result<CaptureResult, String> {
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("캐시 폴더 생성 실패: {e}"))?;
    let monitors = xcap::Monitor::all().map_err(|e| format!("모니터 조회 실패: {e}"))?;
    // index 0 = 주 모니터 (기존 screen_capture 도구와 동일 관례)
    let monitor = monitors.into_iter().next().ok_or("사용 가능한 모니터가 없습니다")?;
    let image = monitor.capture_image().map_err(|e| format!("화면 캡처 실패: {e}"))?;
    let (width, height) = (image.width(), image.height());

    let path = cache_dir.join(format!(
        "capture_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S_%3f")
    ));
    image.save(&path).map_err(|e| format!("캡처 저장 실패: {e}"))?;

    // 썸네일: 저장된 파일을 image 0.25 로 다시 열어 다운스케일(xcap/image 버전 불일치 회피)
    let thumb = image::open(&path)
        .map_err(|e| format!("썸네일 로드 실패: {e}"))?
        .thumbnail(320, 320);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("썸네일 인코딩 실패: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.get_ref());

    Ok(CaptureResult {
        path: path.to_string_lossy().into_owned(),
        thumb_data_url: format!("data:image/png;base64,{b64}"),
        width,
        height,
    })
}

/// UI 주도 스크린샷: 앱 창을 숨기고 → 캡처 → 다시 보여준다.
/// 캡처가 실패해도 창은 반드시 복구된다.
#[tauri::command]
pub async fn capture_screenshot(app: AppHandle) -> Result<CaptureResult, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("앱 캐시 경로 조회 실패: {e}"))?
        .join("captures");

    let window = app.get_webview_window("main");
    if let Some(w) = &window {
        let _ = w.hide();
    }
    // 창이 화면 프레임에서 빠지도록 짧게 대기
    tokio::time::sleep(std::time::Duration::from_millis(180)).await;

    let result = tauri::async_runtime::spawn_blocking(move || capture_primary_to_cache(cache_dir))
        .await
        .map_err(|e| format!("캡처 태스크 실패: {e}"))?;

    // 성공/실패와 무관하게 창 복구
    if let Some(w) = &window {
        let _ = w.show();
        let _ = w.set_focus();
    }
    result
}
```

> `app.path()` 와 `app.get_webview_window()` 는 `tauri::Manager` 트레잇 필요 — commands.rs 는 이미 `use tauri::{AppHandle, Emitter, Manager, State};` 로 가져온다.

- [ ] **Step 2: lib.rs invoke_handler 에 등록**

`src-tauri/src/lib.rs` 의 `tauri::generate_handler![ ... ]` 목록에서 `commands::send_message,` 다음 줄에 추가:

```rust
            commands::send_message,
            commands::capture_screenshot,
```

- [ ] **Step 3: 컴파일 확인**

Run: `cd src-tauri && cargo build --lib`
Expected: 성공 (단, Task 5 미적용 시 `send_message`/`HttpLlmClient::new` 인자 불일치로 실패할 수 있음 → Task 5 를 이어서 진행)

- [ ] **Step 4: 커밋**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "$(printf 'feat: capture_screenshot 커맨드 — 창 숨김 후 주모니터 캡처+썸네일\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 5: send_message 가 attachments 수신 + vision_enabled 배선 + set_config

**Files:**
- Modify: `src-tauri/src/commands.rs`

- [ ] **Step 1: send_message 시그니처에 attachments 추가**

`send_message` 시그니처를 변경:

```rust
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    session_id: String,
    text: String,
    attachments: Vec<String>,
) -> Result<(), String> {
```

- [ ] **Step 2: 사용자 메시지를 첨부 유무로 분기 생성**

`send_message` 본문에서 기존:

```rust
        history.push(ChatMessage::user(text));
```

을 다음으로 교체:

```rust
        history.push(if attachments.is_empty() {
            ChatMessage::user(text)
        } else {
            ChatMessage::user_with_images(text, attachments.clone())
        });
```

- [ ] **Step 3: vision_enabled 계산 + 클라이언트 생성에 전달**

`send_message` 안에서 `max_output_tokens` 등을 읽는 config 잠금 블록:

```rust
    let (max_rounds, temperature, max_output_tokens) = {
        let cfg = state.config.lock().unwrap();
        (cfg.max_tool_rounds, cfg.temperature, cfg.max_output_tokens)
    };
```

을 다음으로 교체(같은 잠금에서 vision 여부 함께 계산):

```rust
    let (max_rounds, temperature, max_output_tokens, vision_enabled) = {
        let cfg = state.config.lock().unwrap();
        let vision = crate::llm::server::resolve_mmproj(&cfg.model_path, &cfg.mmproj_path).is_some();
        (cfg.max_tool_rounds, cfg.temperature, cfg.max_output_tokens, vision)
    };
```

그리고 spawn 안의:

```rust
        let client = HttpLlmClient::new(base_url, max_output_tokens);
```

을:

```rust
        let client = HttpLlmClient::new(base_url, max_output_tokens, vision_enabled);
```

- [ ] **Step 4: set_config 의 재시작 감지에 mmproj_path 추가**

`set_config` 의 `changed` 계산식 마지막 조건(`|| cfg.reasoning_budget != new_config.reasoning_budget`)에 이어 추가:

```rust
            || cfg.reasoning_budget != new_config.reasoning_budget
            || cfg.mmproj_path != new_config.mmproj_path;
```

- [ ] **Step 5: 전체 백엔드 빌드 + 테스트**

Run: `cd src-tauri && cargo build --lib && cargo test --lib`
Expected: 빌드 성공, 전체 테스트 통과

- [ ] **Step 6: 커밋**

```bash
git add src-tauri/src/commands.rs
git commit -m "$(printf 'feat: send_message 첨부 이미지 수신 + vision_enabled 배선\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 6: 시스템 프롬프트에 첨부 이미지 처리 규칙 추가

**Files:**
- Modify: `src-tauri/src/agent.rs`

- [ ] **Step 1: 규칙 15 추가**

`src-tauri/src/agent.rs` 의 `system_prompt` format! 문자열에서 규칙 14 의 마지막 줄:

```rust
         14. 작업 완료는 해당 도구의 성공 결과를 받았을 때만 말한다. 이름변경은 rename_file 의\n\
            '이름 변경 완료' 결과가 근거다. 파일에 목록을 적는 것(write_file)은 이름변경이 아니다.\n\n\
```

을 다음으로 교체 (규칙 15 추가 + `\n\n{persona}` 유지):

```rust
         14. 작업 완료는 해당 도구의 성공 결과를 받았을 때만 말한다. 이름변경은 rename_file 의\n\
            '이름 변경 완료' 결과가 근거다. 파일에 목록을 적는 것(write_file)은 이름변경이 아니다.\n\
         15. 사용자 메시지에 '[첨부 이미지: 경로]' 가 있으면 그 경로가 작업 대상 이미지다.\n\
            설명/판독 요청('설명해줘', '뭐라고 적혀있어')은 도구 없이 직접 본 대로 한국어로 답하고,\n\
            배경제거·변환·저장 요청은 그 경로를 인자로 해당 도구를 호출한다.\n\n\
```

- [ ] **Step 2: 빌드 + 시스템 프롬프트 테스트 영향 확인**

Run: `cd src-tauri && cargo test --lib agent::`
Expected: 통과 (규칙 텍스트 추가는 기존 테스트에 영향 없음)

- [ ] **Step 3: 커밋**

```bash
git add src-tauri/src/agent.rs
git commit -m "$(printf 'feat: 시스템 프롬프트에 첨부 이미지 처리 규칙 추가\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 7: 프론트 타입 + useAgent.send(text, attachments)

**Files:**
- Modify: `src/types.ts`
- Modify: `src/hooks/useAgent.ts`

- [ ] **Step 1: types.ts 갱신**

`UserMessage` 인터페이스를 교체:

```ts
export interface UserMessage {
  role: "user";
  text: string;
  /** 첨부 이미지(썸네일 data URL + 캐시 경로). 복원 시 thumb 가 빈 문자열이면 플레이스홀더 */
  images?: { path: string; thumb: string }[];
}
```

`ChatMessage` 인터페이스에 `images` 추가:

```ts
export interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool";
  content?: string | null;
  tool_calls?: { id: string; type: string; function: { name: string; arguments: string } }[] | null;
  tool_call_id?: string | null;
  images?: string[] | null;
}
```

`AppConfig` 인터페이스에 `mmproj_path` 추가 (`removebg_model: string;` 다음):

```ts
  removebg_model: string;
  mmproj_path: string;
```

- [ ] **Step 2: useAgent.send 시그니처 확장**

`src/hooks/useAgent.ts` 의 `send` 콜백을 교체:

```ts
  const send = useCallback(
    async (text: string, attachments: { path: string; thumb: string }[] = []) => {
      const sessionId = await ensureSession();
      setMessages((prev) => [
        ...prev,
        { role: "user", text, images: attachments.length ? attachments : undefined },
        { role: "assistant", segments: [] },
      ]);
      setBusy(true);
      try {
        await invoke("send_message", {
          sessionId,
          text,
          attachments: attachments.map((a) => a.path),
        });
      } catch (e) {
        patchAssistant((m) => ({
          ...m,
          elapsedMs: 0,
          segments: [...m.segments, { kind: "error", message: String(e) }],
        }));
        setBusy(false);
      }
    },
    [ensureSession, patchAssistant],
  );
```

- [ ] **Step 3: 타입체크**

Run: `pnpm tsc --noEmit` (또는 `npx tsc --noEmit`)
Expected: Composer/App 의 `onSend` 시그니처 불일치 에러가 날 수 있음 → Task 8 에서 해소. types.ts/useAgent.ts 자체 에러는 없어야 함.

- [ ] **Step 4: 커밋**

```bash
git add src/types.ts src/hooks/useAgent.ts
git commit -m "$(printf 'feat: useAgent.send 첨부 이미지 인자 + 프론트 타입 확장\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 8: Composer 에 스크린샷 버튼 + 첨부 칩

**Files:**
- Modify: `src/components/Composer.tsx`

- [ ] **Step 1: Composer 전체 교체**

`src/components/Composer.tsx` 를 다음으로 교체:

```tsx
import { invoke } from "@tauri-apps/api/core";
import { FormEvent, KeyboardEvent, useRef, useState } from "react";

interface Attachment {
  path: string;
  thumb: string;
}

interface Props {
  busy: boolean;
  disabled: boolean;
  onSend: (text: string, attachments: Attachment[]) => void;
  onCancel: () => void;
}

export function Composer({ busy, disabled, onSend, onCancel }: Props) {
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  const canSend = (text.trim().length > 0 || attachments.length > 0) && !busy && !disabled;

  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    if (!canSend) return;
    onSend(text.trim(), attachments);
    setText("");
    setAttachments([]);
    if (taRef.current) taRef.current.style.height = "auto";
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      submit();
    }
  };

  const capture = async () => {
    if (capturing || busy) return;
    setCaptureError(null);
    setCapturing(true);
    try {
      const r = await invoke<{ path: string; thumb_data_url: string }>("capture_screenshot");
      setAttachments((a) => [...a, { path: r.path, thumb: r.thumb_data_url }]);
    } catch (err) {
      setCaptureError(String(err));
    } finally {
      setCapturing(false);
    }
  };

  const removeAt = (i: number) => setAttachments((a) => a.filter((_, idx) => idx !== i));

  return (
    <div className="composer-wrap">
      {captureError && <div className="capture-error">캡처 실패: {captureError}</div>}
      {attachments.length > 0 && (
        <div className="composer-attachments">
          {attachments.map((a, i) => (
            <div key={a.path} className="attach-chip">
              <img src={a.thumb} alt="첨부 이미지" />
              <button type="button" className="attach-remove" title="제거" onClick={() => removeAt(i)}>
                ✕
              </button>
            </div>
          ))}
        </div>
      )}
      <form className="composer" onSubmit={submit}>
        <button
          type="button"
          className="capture-btn"
          title="스크린샷 첨부"
          onClick={capture}
          disabled={disabled || busy || capturing}
        >
          {capturing ? "…" : "📷"}
        </button>
        <textarea
          ref={taRef}
          rows={1}
          value={text}
          placeholder={disabled ? "모델 로딩 중…" : "무엇을 도와드릴까요? (📷 로 화면을 첨부할 수 있어요)"}
          onChange={(e) => {
            setText(e.target.value);
            e.target.style.height = "auto";
            e.target.style.height = `${Math.min(e.target.scrollHeight, 180)}px`;
          }}
          onKeyDown={onKeyDown}
        />
        {busy ? (
          <button type="button" className="send-btn stop" title="중단" onClick={onCancel}>
            ■
          </button>
        ) : (
          <button type="submit" className="send-btn" title="보내기" disabled={!canSend}>
            ↑
          </button>
        )}
      </form>
      <div className="composer-hint">Enter 전송 · Shift+Enter 줄바꿈 · 로컬에서만 동작 — 데이터가 PC를 떠나지 않습니다</div>
    </div>
  );
}
```

- [ ] **Step 2: 타입체크**

Run: `npx tsc --noEmit`
Expected: App.tsx 의 `onSend={send}` 는 시그니처 호환(send 의 attachments 는 기본값 있음). 통과.

- [ ] **Step 3: 커밋**

```bash
git add src/components/Composer.tsx
git commit -m "$(printf 'feat: Composer 스크린샷 버튼 + 첨부 칩\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 9: user 말풍선 썸네일 + 세션 복원

**Files:**
- Modify: `src/components/MessageView.tsx`
- Modify: `src/lib/restore.ts`

- [ ] **Step 1: MessageView 의 user 분기 교체**

`src/components/MessageView.tsx` 의 `MessageView` 함수에서:

```tsx
  if (msg.role === "user") {
    return <div className="msg-user">{msg.text}</div>;
  }
```

를 다음으로 교체:

```tsx
  if (msg.role === "user") {
    return (
      <div className="msg-user">
        {msg.images && msg.images.length > 0 && (
          <div className="msg-user-images">
            {msg.images.map((im, i) =>
              im.thumb ? (
                <img key={i} className="msg-thumb" src={im.thumb} alt="첨부 이미지" />
              ) : (
                <span key={i} className="msg-thumb-ph">
                  📷 캡처
                </span>
              ),
            )}
          </div>
        )}
        {msg.text && <div className="msg-user-text">{msg.text}</div>}
      </div>
    );
  }
```

- [ ] **Step 2: restore.ts 의 user 복원 교체**

`src/lib/restore.ts` 의:

```ts
    if (m.role === "user") {
      out.push({ role: "user", text: m.content ?? "" });
      current = null;
    } else if (m.role === "assistant") {
```

를 다음으로 교체:

```ts
    if (m.role === "user") {
      // 백엔드가 붙인 '[첨부 이미지: ...]' 마커는 표시용 텍스트에서 제거
      const raw = m.content ?? "";
      const text = raw.replace(/\n\n\[첨부 이미지: [^\]]*\]\s*$/, "");
      const images = (m.images ?? []).map((path) => ({ path, thumb: "" }));
      out.push({ role: "user", text, images: images.length ? images : undefined });
      current = null;
    } else if (m.role === "assistant") {
```

- [ ] **Step 3: 타입체크 + 프론트 빌드**

Run: `npx tsc --noEmit && pnpm build`
Expected: 성공

- [ ] **Step 4: 커밋**

```bash
git add src/components/MessageView.tsx src/lib/restore.ts
git commit -m "$(printf 'feat: user 말풍선 썸네일 렌더 + 세션 복원(마커 제거/플레이스홀더)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 10: CSS

**Files:**
- Modify: `src/styles/global.css`

- [ ] **Step 1: 스타일 추가**

`src/styles/global.css` 의 `.composer-hint { ... }` 규칙 **다음**(파일 끝 근처)에 추가:

```css
/* ── 스크린샷 캡처 버튼 / 첨부 ─────────────────────────── */
.capture-btn {
  flex: none;
  width: 38px;
  height: 38px;
  border-radius: 11px;
  border: 1px solid var(--line);
  display: grid;
  place-items: center;
  background: var(--bg-overlay);
  color: var(--ink);
  font-size: 16px;
  cursor: pointer;
  transition: transform 0.12s, background 0.15s, opacity 0.15s;
}
.capture-btn:hover:not(:disabled) {
  transform: translateY(-1px);
  border-color: var(--amber);
}
.capture-btn:disabled {
  opacity: 0.35;
  cursor: default;
}
.composer-attachments {
  pointer-events: auto;
  max-width: 780px;
  margin: 0 auto 8px;
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.attach-chip {
  position: relative;
  width: 64px;
  height: 64px;
  border-radius: 10px;
  overflow: hidden;
  border: 1px solid var(--line);
  background: var(--bg-overlay);
}
.attach-chip img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.attach-remove {
  position: absolute;
  top: 2px;
  right: 2px;
  width: 18px;
  height: 18px;
  border: none;
  border-radius: 50%;
  background: rgba(0, 0, 0, 0.6);
  color: #fff;
  font-size: 11px;
  line-height: 1;
  cursor: pointer;
  display: grid;
  place-items: center;
}
.capture-error {
  pointer-events: auto;
  max-width: 780px;
  margin: 0 auto 8px;
  font-size: 12px;
  color: var(--red);
}
/* user 말풍선 안 썸네일 */
.msg-user-images {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 8px;
}
.msg-thumb {
  max-width: 220px;
  max-height: 220px;
  border-radius: 10px;
  border: 1px solid var(--line);
  display: block;
}
.msg-thumb-ph {
  font-size: 12px;
  color: var(--ink-faint);
  padding: 6px 10px;
  border: 1px dashed var(--line);
  border-radius: 8px;
}
```

> `var(--red)`, `var(--bg-overlay)`, `var(--line)`, `var(--ink)`, `var(--ink-faint)`, `var(--amber)` 는 기존 global.css 에 정의된 토큰. 새 토큰 도입 없음.

- [ ] **Step 2: 빌드 확인**

Run: `pnpm build`
Expected: 성공

- [ ] **Step 3: 커밋**

```bash
git add src/styles/global.css
git commit -m "$(printf 'style: 캡처 버튼/첨부 칩/말풍선 썸네일 스타일\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 11: 수동 검증 (macOS 우선, Windows 점검)

**전제:** 기본 모델 `Qwen3.5-2B-Q4_K_M.gguf` 와 같은 폴더에 `mmproj-Qwen3.5-2B-BF16.gguf` 존재(이미 다운로드 완료). 앱 기동 시 `resolve_mmproj` 가 자동 페어링 → llama-server 로그에 `loaded multimodal model` 떠야 함.

- [ ] **Step 1: 개발 앱 실행**

Run: `pnpm tauri dev`
Expected: 앱 창이 뜨고, llama-server 가 mmproj 와 함께 로드(상태 ready). 백엔드 로그(`llama_server_log_file`)에 `loaded multimodal model` 확인.

- [ ] **Step 2: 캡처 흐름**

입력창의 📷 클릭 → 앱 창이 잠깐 숨었다가 → 캡처 후 다시 보이고 포커스 복귀 → 입력창 위에 썸네일 칩 표시. ✕ 로 제거 가능.
Expected: 창 복구 정상, 썸네일 표시.

- [ ] **Step 3: vision 직답**

칩이 있는 상태에서 "이 이미지 뭐라고 적혀있어?" 전송 → 도구 호출 없이 화면 내용/텍스트를 한국어로 설명. user 말풍선에 썸네일+텍스트가 함께 표시.
Expected: 화면 내용에 부합하는 설명.

- [ ] **Step 4: 경로 기반 도구**

같은(또는 새) 캡처로 "이 이미지 배경 제거해줘" 전송 → 모델이 `remove_background` 를 캡처 캐시 경로로 호출 → 결과물이 **워크스페이스**에 생성됨(파일 탐색기로 확인). 이어서 "저장해줘"/"png로 변환" 등도 워크스페이스 산출.
Expected: 산출물이 워크스페이스에 생성, 도구 성공 결과 보고.

- [ ] **Step 5: 세션 저장/복원**

새 대화로 전환했다가 사이드바에서 방금 세션을 다시 로드.
Expected: user 말풍선이 텍스트(마커 제거됨) + "📷 캡처" 플레이스홀더로 복원(캐시가 남아있으면 경로 유효, 비워졌으면 플레이스홀더). 앱이 크래시하지 않음.

- [ ] **Step 6: Windows 점검 (가능 환경에서)**

Windows 빌드/실행 후 Step 1~5 반복. 특히 (a) `hide()` 직후 캡처에 앱 창이 안 잡히는지, (b) `app_cache_dir` 경로에 captures 생성, (c) 썸네일/배경제거 동작 확인.
Expected: macOS 와 동일 동작. 창이 프레임에 남으면 `capture_screenshot` 의 대기(180ms)를 상향.

- [ ] **Step 7: 최종 점검 커밋(필요 시)**

검증 중 수정이 있었다면 커밋. 없으면 생략.

---

## Self-Review 체크

- **Spec 커버리지:**
  - 📷 버튼/입력창 옆 → Task 8 ✓
  - 앱 hide→캡처→show → Task 4 ✓
  - 썸네일 첨부(입력창 위) → Task 8/10 ✓
  - vision 설명/판독 → Task 3/6, Task 11 Step 3 ✓
  - 배경제거/저장 등 기존 도구 → Task 5/6, Task 11 Step 4 ✓ (도구 자체는 기존 유지, 신규 변경 없음)
  - 말풍선에 자연어+이미지 묶음 → Task 9/10 ✓
  - 산출물 워크스페이스 생성 → 기존 `ensure_in_workspace` 그대로(캡처 원본만 캐시) ✓
  - 원본=캐시 저장 → Task 4 ✓
  - mmproj 자동 적용 → Task 1 ✓
  - Windows 정합 → Task 11 Step 6, 영역선택 미채택으로 동일 코드패스 ✓
- **플레이스홀더 스캔:** 모든 코드 스텝에 실제 코드 포함, TODO/TBD 없음 ✓
- **타입 일관성:** `CaptureResult{path,thumb_data_url,width,height}` ↔ 프론트 `{path, thumb_data_url}` 사용 일치. `Attachment{path,thumb}` ↔ `UserMessage.images{path,thumb}` 일치. `message_to_request_value(m, vision_enabled)` 시그니처 Task 3 정의/사용 일치. `HttpLlmClient::new(_,_,vision_enabled)` 5개 호출처 갱신(Task 3 테스트 4 + Task 5 prod 1) ✓
- **상호 의존:** Task 3~5 는 함께 적용해야 `cargo build` 녹색(중간 상태 경고 명시됨). subagent 실행 시 Task 3→4→5 를 연속 처리 권장.
