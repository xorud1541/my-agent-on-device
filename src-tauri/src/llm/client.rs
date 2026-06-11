use crate::models::{ChatMessage, CompletionResult, FunctionCall, ToolCall};
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::BTreeMap;

/// 스트리밍 중간 콜백. (thinking 토큰인지 여부, 텍스트 조각)
pub type DeltaSink<'a> = &'a mut (dyn FnMut(DeltaKind, &str) + Send);

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
    http: reqwest::Client,
}

impl HttpLlmClient {
    pub fn new(base_url: String) -> Self {
        Self { base_url, http: reqwest::Client::new() }
    }
}

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
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "temperature": temperature,
            "stream": true,
            "max_tokens": 4096,
        });

        let resp = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body)
            .send()
            .await
            .context("llama-server 요청 실패")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("llama-server 오류 {status}: {text}");
        }

        let mut result = CompletionResult::default();
        // tool_calls 는 index 별로 조각나서 오므로 누적 후 합친다.
        let mut tool_acc: BTreeMap<u64, (String, String, String)> = BTreeMap::new();

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
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
                    sink(DeltaKind::Thinking, t);
                }
                if let Some(t) = delta.get("content").and_then(Value::as_str) {
                    result.content.push_str(t);
                    sink(DeltaKind::Text, t);
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
