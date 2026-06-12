use crate::models::{ChatMessage, CompletionResult, FunctionCall, ToolCall};
use anyhow::{bail, Context, Result};
use base64::Engine as _;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::BTreeMap;

/// ChatMessage 를 llama-server /v1/chat/completions 요청용 JSON 으로 변환한다.
/// images 가 있고 vision 이 켜져 있으면 content 를 멀티모달 배열로 바꾸고
/// 파일을 base64 image_url 로 인라인한다. 그 외에는 기존 직렬화 그대로.
pub(crate) fn message_to_request_value(m: &ChatMessage, vision_enabled: bool) -> Value {
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

/// 스트리밍 중간 콜백. (thinking 토큰인지 여부, 텍스트 조각)
/// false 를 돌려주면 생성을 즉시 중단한다 (사용자 취소).
pub type DeltaSink<'a> = &'a mut (dyn FnMut(DeltaKind, &str) -> bool + Send);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeltaKind {
    Thinking,
    Text,
}

/// 에이전트 루프가 의존하는 LLM 인터페이스. 테스트에서는 mock 으로 대체한다.
#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &Value,
        temperature: f32,
        sink: DeltaSink<'_>,
    ) -> Result<CompletionResult>;
}

/// llama-server OpenAI 호환 엔드포인트용 실제 클라이언트.
pub struct HttpLlmClient {
    pub base_url: String,
    /// 호출당 출력 토큰 상한 — 레이턴시 예산의 핵심 레버 (20 t/s 기준 1024 ≈ 50초)
    pub max_tokens: u32,
    /// 로드된 모델에 mmproj(vision)가 붙어 있는지. 꺼지면 이미지 파트를 전송하지 않는다.
    pub vision_enabled: bool,
    http: reqwest::Client,
}

/// 반복 패널티. llama-server 기본값은 1.0(꺼짐)이라, 2B 모델이 툴콜 인자에서
/// 단일 바이트 토큰을 출력 한도까지 반복하는 붕괴를 막지 못한다 (2026-06-11 사고).
/// 경로처럼 정당한 반복이 많은 출력도 있어 보수적으로 1.1 을 쓴다.
const REPEAT_PENALTY: f32 = 1.1;

impl HttpLlmClient {
    pub fn new(base_url: String, max_tokens: u32, vision_enabled: bool) -> Self {
        Self { base_url, max_tokens, vision_enabled, http: reqwest::Client::new() }
    }

    /// 서버가 /health 로 살아날 때까지 최대 `secs` 초 대기 (크래시 후 사이드카 재기동 대비)
    async fn wait_for_health(&self, secs: u64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Ok(r) = self.http.get(format!("{}/health", self.base_url)).send().await {
                if r.status().is_success() {
                    return;
                }
            }
        }
    }
}

/// 모델 재생성으로 해소될 수 있는 일시 오류인가?
/// 대표 사례: 작은 모델이 Windows 경로를 무이스케이프로 복사해 툴콜 인자 JSON 파싱이 깨지는 500.
pub(crate) fn is_retryable_generation_error(status: u16, body: &str) -> bool {
    status == 500 && body.contains("Failed to parse tool call")
}

const MAX_ATTEMPTS: u32 = 3;

#[async_trait::async_trait]
impl LlmClient for HttpLlmClient {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &Value,
        temperature: f32,
        sink: DeltaSink<'_>,
    ) -> Result<CompletionResult> {
        let body = serde_json::json!({
            "model": "default",
            "messages": messages.iter()
                .map(|m| message_to_request_value(m, self.vision_enabled))
                .collect::<Vec<_>>(),
            "tools": tools,
            "tool_choice": "auto",
            "temperature": temperature,
            "repeat_penalty": REPEAT_PENALTY,
            "stream": true,
            "max_tokens": self.max_tokens,
        });

        // 파싱 계열 500은 스트림 시작 전에 떨어지므로(델타 미방출) 재요청이 안전하다.
        // 연결 오류(10054 등)도 전송 단계라 델타 미방출 — llama-server 가 크래시해도
        // 사이드카 슈퍼바이저가 재기동하므로, health 회복을 기다렸다 재시도하면 턴이
        // 살아남는다 (2026-06-12 실측: 하니스 2회 연속 서버 크래시로 턴 전체 사망).
        let mut resp = None;
        for attempt in 1..=MAX_ATTEMPTS {
            let r = match self
                .http
                .post(format!("{}/v1/chat/completions", self.base_url))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) if attempt < MAX_ATTEMPTS => {
                    self.wait_for_health(30).await;
                    let _ = e;
                    continue;
                }
                Err(e) => return Err(e).context("llama-server 요청 실패"),
            };
            if r.status().is_success() {
                resp = Some(r);
                break;
            }
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            if attempt < MAX_ATTEMPTS && is_retryable_generation_error(status.as_u16(), &text) {
                continue;
            }
            bail!("llama-server 오류 {status}: {text}");
        }
        let resp = resp.expect("loop guarantees Some on success");

        let mut result = CompletionResult::default();
        // tool_calls 는 index 별로 조각나서 오므로 누적 후 합친다.
        let mut tool_acc: BTreeMap<u64, (String, String, String)> = BTreeMap::new();

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        'outer: while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("스트림 읽기 실패")?;
            buf.push_str(&String::from_utf8_lossy(&chunk));

            // SSE: "data: {...}\n\n" 단위로 파싱
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim().to_string();
                buf.drain(..=pos);
                let Some(payload) = line.strip_prefix("data: ") else { continue };
                if payload == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(payload) else { continue };
                let Some(delta) = v.pointer("/choices/0/delta") else { continue };

                if let Some(t) = delta.get("reasoning_content").and_then(Value::as_str) {
                    result.reasoning.push_str(t);
                    if !sink(DeltaKind::Thinking, t) {
                        break 'outer; // 사용자 취소 — 연결을 끊어 서버 생성도 중단시킨다
                    }
                }
                if let Some(t) = delta.get("content").and_then(Value::as_str) {
                    result.content.push_str(t);
                    if !sink(DeltaKind::Text, t) {
                        break 'outer;
                    }
                }
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for c in calls {
                        let idx = c.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let entry = tool_acc.entry(idx).or_default();
                        if let Some(id) = c.get("id").and_then(Value::as_str) {
                            entry.0.push_str(id);
                        }
                        if let Some(name) = c.pointer("/function/name").and_then(Value::as_str) {
                            entry.1.push_str(name);
                        }
                        if let Some(args) = c.pointer("/function/arguments").and_then(Value::as_str) {
                            entry.2.push_str(args);
                        }
                    }
                }
            }
        }

        for (idx, (id, name, args)) in tool_acc {
            result.tool_calls.push(ToolCall {
                id: if id.is_empty() { format!("call_{idx}") } else { id },
                call_type: "function".into(),
                function: FunctionCall { name, arguments: args },
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn retry_predicate_matches_tool_parse_500_only() {
        let parse_err = r#"{"error":{"code":500,"message":"Failed to parse tool call arguments as JSON: ..."}}"#;
        assert!(is_retryable_generation_error(500, parse_err));
        assert!(!is_retryable_generation_error(400, parse_err));
        assert!(!is_retryable_generation_error(500, "out of memory"));
    }

    /// 1회차: 툴콜 파싱 500 → 2회차: 정상 SSE. 클라이언트가 재시도로 회복해야 한다.
    #[tokio::test(flavor = "multi_thread")]
    async fn recovers_from_tool_parse_500_by_retrying() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicU32::new(0));

        let hits2 = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let n = hits2.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let resp = if n == 0 {
                    let body = r#"{"error":{"code":500,"message":"Failed to parse tool call arguments as JSON: invalid string"}}"#;
                    format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                } else {
                    let sse = concat!(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"안녕\"}}]}\n\n",
                        "data: [DONE]\n\n"
                    );
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        sse.len(),
                        sse
                    )
                };
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        let client = HttpLlmClient::new(base_url, 1024, false);
        let messages = vec![ChatMessage::user("hi")];
        let mut sink = |_k: DeltaKind, _t: &str| true;
        let result = client
            .complete(&messages, &serde_json::json!([]), 0.2, &mut sink)
            .await
            .expect("재시도로 회복해야 함");

        assert_eq!(result.content, "안녕");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "정확히 2회 요청해야 함");
    }

    /// 요청 바디에 반복 패널티가 실려야 한다 — 2B 모델 토큰 반복 붕괴 방지.
    #[tokio::test(flavor = "multi_thread")]
    async fn request_body_includes_repeat_penalty() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel::<String>();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let sse = "data: [DONE]\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });

        let client = HttpLlmClient::new(base_url, 1024, false);
        let messages = vec![ChatMessage::user("hi")];
        let mut sink = |_k: DeltaKind, _t: &str| true;
        let _ = client.complete(&messages, &serde_json::json!([]), 0.2, &mut sink).await;

        let request = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(request.contains("\"repeat_penalty\":1.1"), "요청에 repeat_penalty 없음:\n{request}");
    }

    /// 재시도 불가 오류(4xx 등)는 즉시 실패해야 한다.
    #[tokio::test(flavor = "multi_thread")]
    async fn non_retryable_error_fails_immediately() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicU32::new(0));

        let hits2 = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                hits2.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let body = r#"{"error":{"code":400,"message":"bad request"}}"#;
                let resp = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        let client = HttpLlmClient::new(base_url, 1024, false);
        let messages = vec![ChatMessage::user("hi")];
        let mut sink = |_k: DeltaKind, _t: &str| true;
        let err = client
            .complete(&messages, &serde_json::json!([]), 0.2, &mut sink)
            .await
            .expect_err("400은 즉시 실패");
        assert!(err.to_string().contains("400"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    /// sink 가 false 를 반환하면(사용자 취소) 스트림을 즉시 끊고 부분 결과를 돌려줘야 한다.
    #[tokio::test(flavor = "multi_thread")]
    async fn sink_false_aborts_stream_midway() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());

        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
                let _ = sock.write_all(head.as_bytes()).await;
                // 청크를 천천히 흘려보내며 무한 생성 흉내
                for i in 0..50 {
                    let line = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"토큰{i} \"}}}}]}}\n\n");
                    if sock.write_all(line.as_bytes()).await.is_err() {
                        break; // 클라이언트가 끊음 — 기대 동작
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }
            }
        });

        let client = HttpLlmClient::new(base_url, 1024, false);
        let messages = vec![ChatMessage::user("hi")];
        let mut seen = 0u32;
        let mut sink = |_k: DeltaKind, _t: &str| {
            seen += 1;
            seen < 3 // 3번째 델타에서 취소
        };
        let started = std::time::Instant::now();
        let result = client
            .complete(&messages, &serde_json::json!([]), 0.2, &mut sink)
            .await
            .expect("취소는 오류가 아니라 부분 결과");

        assert!(result.content.starts_with("토큰0"), "{}", result.content);
        assert!(seen >= 3 && seen < 50, "스트림이 일찍 끊겨야 함 (seen={seen})");
        assert!(started.elapsed().as_secs() < 5, "50청크 전체를 기다리면 안 됨");
    }
}

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
