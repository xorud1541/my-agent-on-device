use crate::llm::client::{DeltaKind, LlmClient};
use crate::models::{AgentEvent, ChatMessage};
use crate::tools::ToolRegistry;
use anyhow::Result;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// 시스템 프롬프트. prefill 비용을 위해 간결하게 유지한다.
pub fn system_prompt() -> String {
    let home = dirs::home_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M (%A)");
    format!(
        "너는 사용자의 Windows PC에서 동작하는 로컬 에이전트 'Local Agent'다.\n\
         현재 시각: {now}. 사용자 홈 디렉토리: {home}\n\
         (바탕화면={home}\\Desktop, 다운로드={home}\\Downloads, 문서={home}\\Documents, 사진={home}\\Pictures)\n\n\
         규칙:\n\
         1. 파일/이미지/PDF/화면과 관련된 요청은 반드시 도구를 호출해 실제로 수행한다. 추측으로 답하지 않는다.\n\
         2. 사용자가 상대적 위치(바탕화면, 다운로드 폴더 등)를 말하면 위 절대경로로 변환해 사용한다.\n\
         3. 도구 결과를 받으면 결과를 바탕으로 다음 행동을 결정하거나, 한국어로 간결하게 최종 답변한다.\n\
         4. 도구가 실패하면 인자를 고쳐 다시 시도하거나, 불가능하면 이유를 설명한다.\n\
         5. 잡담/지식 질문에는 도구 없이 한국어로 답한다.\n\
         6. 같은 도구를 같은 인자로 반복 호출하지 않는다.\n\
         7. 도구 인자의 파일 경로는 반드시 슬래시(/)로 쓴다. 예: C:/Users/EST/Downloads (백슬래시 금지)."
    )
}

/// 사용자 발화 1회를 처리하는 에이전트 루프.
/// 메시지 히스토리를 직접 갱신하며, 진행 상황을 emit 으로 흘린다.
pub async fn run_turn(
    client: &dyn LlmClient,
    registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    session_id: &str,
    max_tool_rounds: u32,
    temperature: f32,
    cancel: &AtomicBool,
    emit: &(dyn Fn(AgentEvent) + Send + Sync),
) -> Result<()> {
    let started = Instant::now();
    let tools = registry.schemas();

    for round in 0..=max_tool_rounds {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let mut sink = |kind: DeltaKind, text: &str| {
            let ev = match kind {
                DeltaKind::Thinking => AgentEvent::ThinkingDelta {
                    session_id: session_id.to_string(),
                    delta: text.to_string(),
                },
                DeltaKind::Text => AgentEvent::TextDelta {
                    session_id: session_id.to_string(),
                    delta: text.to_string(),
                },
            };
            emit(ev);
        };
        let result = client.complete(messages, &tools, temperature, &mut sink).await?;

        let content = if result.content.is_empty() { None } else { Some(result.content.clone()) };
        let tool_calls = if result.tool_calls.is_empty() { None } else { Some(result.tool_calls.clone()) };
        messages.push(ChatMessage::assistant(content, tool_calls));

        if result.tool_calls.is_empty() {
            break;
        }
        if round == max_tool_rounds {
            // 라운드 소진: 도구 결과 없이 종료를 알린다
            emit(AgentEvent::Error {
                session_id: session_id.to_string(),
                message: format!("도구 호출 한도({max_tool_rounds}회) 초과로 중단"),
            });
            break;
        }

        for call in &result.tool_calls {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            emit(AgentEvent::ToolCallStart {
                session_id: session_id.to_string(),
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                arguments: call.function.arguments.clone(),
            });

            let args: Value = serde_json::from_str(&call.function.arguments)
                .unwrap_or(Value::Object(Default::default()));
            // 도구는 동기 구현 — 블로킹 실행을 런타임에 알린다
            let output = tokio::task::block_in_place(|| registry.execute(&call.function.name, &args));

            let (ok, text) = match output {
                Ok(t) => (true, t),
                Err(e) => (false, format!("오류: {e:#}")),
            };
            emit(AgentEvent::ToolCallEnd {
                session_id: session_id.to_string(),
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                ok,
                result: clip(&text, 2000),
            });
            messages.push(ChatMessage::tool(call.id.clone(), clip(&text, 8000)));
        }
    }

    emit(AgentEvent::TurnEnd {
        session_id: session_id.to_string(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
    Ok(())
}

fn clip(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}\n...(잘림)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::client::DeltaSink;
    use crate::models::{CompletionResult, FunctionCall, ToolCall};
    use std::sync::Mutex;

    /// 호출 순서대로 미리 준비한 응답을 돌려주는 mock
    struct MockClient {
        responses: Mutex<Vec<CompletionResult>>,
    }

    #[async_trait::async_trait]
    impl LlmClient for MockClient {
        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &Value,
            _temperature: f32,
            sink: DeltaSink<'_>,
        ) -> Result<CompletionResult> {
            let r = self.responses.lock().unwrap().remove(0);
            if !r.content.is_empty() {
                sink(DeltaKind::Text, &r.content);
            }
            Ok(r)
        }
    }

    fn tool_call_result(name: &str, args: Value) -> CompletionResult {
        CompletionResult {
            content: String::new(),
            reasoning: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_0".into(),
                call_type: "function".into(),
                function: FunctionCall { name: name.into(), arguments: args.to_string() },
            }],
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loop_executes_tool_then_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "안녕하세요").unwrap();

        let client = MockClient {
            responses: Mutex::new(vec![
                tool_call_result("read_file", serde_json::json!({"path": file.to_string_lossy()})),
                CompletionResult { content: "파일 내용은 '안녕하세요' 입니다.".into(), ..Default::default() },
            ]),
        };
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::system(system_prompt()), ChatMessage::user("hello.txt 읽어줘")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(
            &client, &registry, &mut messages, "s1", 8, 0.7, &cancel,
            &|ev| events.lock().unwrap().push(ev),
        )
        .await
        .unwrap();

        // assistant(tool_call) + tool + assistant(final) 이 히스토리에 쌓였는지
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[2].role, "assistant");
        assert_eq!(messages[3].role, "tool");
        assert!(messages[3].content.as_deref().unwrap().contains("안녕하세요"));
        assert_eq!(messages[4].role, "assistant");

        let evs = events.lock().unwrap();
        let kinds: Vec<&str> = evs
            .iter()
            .map(|e| match e {
                AgentEvent::ToolCallStart { .. } => "tool-start",
                AgentEvent::ToolCallEnd { ok: true, .. } => "tool-ok",
                AgentEvent::ToolCallEnd { ok: false, .. } => "tool-err",
                AgentEvent::TextDelta { .. } => "text",
                AgentEvent::TurnEnd { .. } => "end",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["tool-start", "tool-ok", "text", "end"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tool_failure_is_fed_back_to_model() {
        let client = MockClient {
            responses: Mutex::new(vec![
                tool_call_result("read_file", serde_json::json!({"path": "C:\\없는파일.txt"})),
                CompletionResult { content: "파일을 찾을 수 없습니다.".into(), ..Default::default() },
            ]),
        };
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("읽어줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        assert!(messages.iter().any(|m| m.role == "tool" && m.content.as_deref().unwrap().starts_with("오류:")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn round_limit_stops_infinite_tool_loop() {
        // 항상 도구를 호출하는 모델 — 한도에서 끊겨야 한다
        let responses: Vec<CompletionResult> = (0..10)
            .map(|_| tool_call_result("list_dir", serde_json::json!({"path": "C:\\"})))
            .collect();
        let client = MockClient { responses: Mutex::new(responses) };
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("loop")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &mut messages, "s1", 2, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
    }
}
