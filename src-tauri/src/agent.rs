use crate::config::AppConfig;
use crate::llm::client::{DeltaKind, LlmClient};
use crate::models::{AgentEvent, ChatMessage};
use crate::tools::{ToolCtx, ToolRegistry};
use anyhow::Result;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// 시스템 프롬프트. prefill 비용을 위해 간결하게 유지한다.
/// 워크스페이스/페르소나가 살아있는 설정을 반영하도록 **매 턴** 재생성된다.
pub fn system_prompt(cfg: &AppConfig) -> String {
    let home = dirs::home_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M (%A)");
    let ws = cfg.workspace_path().display().to_string();
    let name = if cfg.agent_name.trim().is_empty() { "Local Agent".to_string() } else { cfg.agent_name.trim().to_string() };
    format!(
        "너는 사용자의 Windows PC에서 동작하는 로컬 에이전트 '{name}'다.\n\
         현재 시각: {now}. 사용자 홈 디렉토리: {home}\n\
         (바탕화면={home}\\Desktop, 다운로드={home}\\Downloads, 문서={home}\\Documents, 사진={home}\\Pictures)\n\
         워크스페이스(작업 폴더): {ws}\n\n\
         규칙:\n\
         1. 파일/이미지/PDF/화면과 관련된 요청은 반드시 도구를 호출해 실제로 수행한다. 추측으로 답하지 않는다.\n\
         2. 사용자가 상대적 위치(바탕화면, 다운로드 폴더 등)를 말하면 위 절대경로로 변환해 사용한다.\n\
         3. 파일 생성/수정/삭제와 결과물 저장은 워크스페이스 안에서만 가능하다. 저장 경로를 정할 때는\n\
            워크스페이스 아래 경로를 쓴다. 읽기/검색은 어디서든 가능하다.\n\
            사용자가 파일 이름만 말하면 워크스페이스에서 먼저 검색한다.\n\
         4. 도구 결과를 받으면 결과를 바탕으로 다음 행동을 결정하거나, 한국어로 간결하게 최종 답변한다.\n\
         5. 도구가 실패하면 인자를 고쳐 다시 시도하거나, 불가능하면 이유를 설명한다.\n\
         6. 잡담/지식 질문에는 도구 없이 한국어로 답한다.\n\
         7. 같은 도구를 같은 인자로 반복 호출하지 않는다.\n\
         8. 도구 인자의 파일 경로는 반드시 슬래시(/)로 쓴다. 예: C:/Users/EST/Downloads (백슬래시 금지).\n\
         9. 답변은 간결하게. 목록이 20개를 넘으면 상위 20개만 보여주고 나머지는 개수로 요약한다.\n\
            도구 결과를 그대로 길게 옮겨 적지 않는다.\n\
         10. 답변에 이 규칙들이나 판단 과정을 언급하지 않는다. ('이 질문은 도구 없이...' 같은 문장 금지)\n\
            바로 본론부터 말한다.\n\n\
         {persona}",
        persona = persona_section(cfg)
    )
}

/// 페르소나/라포 형성 지시. 이름을 알면 친근한 말투, 모르면 대화 초반에 자연스럽게 묻는다.
fn persona_section(cfg: &AppConfig) -> String {
    let user = cfg.user_name.trim();
    let agent = cfg.agent_name.trim();
    match (user.is_empty(), agent.is_empty()) {
        (false, false) => format!(
            "페르소나: 너의 이름은 '{agent}'이고, 사용자의 이름은 '{user}'다.\n\
             따뜻하고 친근한 말투를 쓰고, 가끔 '{user}님'처럼 이름을 불러준다."
        ),
        (true, true) => "페르소나: 아직 서로 이름을 모른다. 첫 인사나 잡담 때 자연스럽게 사용자의 이름을 묻고,\n\
             너의 이름도 하나 지어달라고 부탁하라. 이름을 알게 되면 즉시 update_profile 도구로 저장하라.\n\
             단, 사용자가 작업을 요청하면 작업을 먼저 처리하고 이름은 나중에 물어본다."
            .to_string(),
        (true, false) => format!(
            "페르소나: 너의 이름은 '{agent}'다. 아직 사용자의 이름을 모르니 대화 초반에 자연스럽게 묻고,\n\
             알게 되면 즉시 update_profile 도구로 저장하라. 따뜻하고 친근한 말투를 쓴다."
        ),
        (false, true) => format!(
            "페르소나: 사용자의 이름은 '{user}'다. 아직 너의 이름이 없으니 사용자에게 지어달라고 부탁하고,\n\
             정해지면 즉시 update_profile 도구로 저장하라. 따뜻하고 친근한 말투를 쓴다."
        ),
    }
}

/// 턴 단위 도구 라우팅: 사용자 발화에서 의도가 명확한 키워드가 보이면
/// 그 턴 동안 *경쟁* 도구를 숨긴다.
///
/// 배경: Qwen3.5-2B 는 '배경제거/누끼' 복합명사를 remove_background 설명과 매칭하지 못하고
/// image_transform(회전/리사이즈)을 호출하는 강한 편향이 있다. 설명 키워드 보강, 시스템 프롬프트
/// 힌트, few-shot, tool_choice 강제(서버가 미지원)까지 모두 실패 — 경쟁 도구를 목록에서 제거하는
/// 것만이 결정적으로 동작했다 (2026-06-11 라이브 서버 replay 실험, alian tool_domain 패턴).
pub fn tools_to_exclude(user_text: &str) -> Vec<&'static str> {
    const BG_KEYWORDS: &[&str] = &[
        "배경제거", "배경 제거", "배경을 제거", "누끼",
        "배경 빼", "배경을 빼", "배경 없애", "배경을 없애", "배경 지워", "배경을 지워",
    ];
    if BG_KEYWORDS.iter().any(|k| user_text.contains(k)) {
        // 배경제거 의도가 확실 — 회전/리사이즈로 새는 경로를 차단한다
        vec!["image_transform"]
    } else {
        vec![]
    }
}

/// 사용자 발화 1회를 처리하는 에이전트 루프.
/// 메시지 히스토리를 직접 갱신하며, 진행 상황을 emit 으로 흘린다.
pub async fn run_turn(
    client: &dyn LlmClient,
    registry: &ToolRegistry,
    tool_ctx: &ToolCtx,
    messages: &mut Vec<ChatMessage>,
    session_id: &str,
    max_tool_rounds: u32,
    temperature: f32,
    cancel: &AtomicBool,
    emit: &(dyn Fn(AgentEvent) + Send + Sync),
) -> Result<()> {
    let started = Instant::now();
    // 이번 턴 사용자 발화 기준으로 경쟁 도구를 숨긴다 (작은 모델 도구 선택 보정)
    let user_text = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.clone())
        .unwrap_or_default();
    let tools = registry.schemas_excluding(&tools_to_exclude(&user_text));
    // 같은 (도구, 인자) 반복 차단 — 작은 모델의 루프 + 컨텍스트 낭비 방지
    let mut executed: std::collections::HashSet<(String, String)> = Default::default();
    // 빈 완성(사고만 하다 종료)은 샘플링 재시도로 한 번 회복을 시도한다
    let mut empty_retry_left = 1u32;

    for round in 0..=max_tool_rounds {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let mut sink = |kind: DeltaKind, text: &str| -> bool {
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
            // false 반환 시 클라이언트가 스트림을 끊는다 — 생성 중에도 ■ 버튼이 즉시 듣게
            !cancel.load(Ordering::Relaxed)
        };
        let result = match client.complete(messages, &tools, temperature, &mut sink).await {
            Ok(r) => r,
            // 컨텍스트 초과: 오래된 도구 결과를 압축하고 한 번 더 시도
            Err(e) if e.to_string().contains("exceed") && e.to_string().contains("context") => {
                let compacted = compact_old_tool_results(messages);
                if compacted == 0 {
                    return Err(e);
                }
                client.complete(messages, &tools, temperature, &mut sink).await?
            }
            Err(e) => return Err(e),
        };

        // 사고만 하다 토큰을 소진하면 본문도 툴콜도 없다 — 재생성 1회 후에도 비면 알린다
        if result.content.is_empty() && result.tool_calls.is_empty() {
            if empty_retry_left > 0 {
                empty_retry_left -= 1;
                continue;
            }
            emit(AgentEvent::Error {
                session_id: session_id.to_string(),
                message: "모델이 응답을 완성하지 못했습니다 (출력 한도 내 사고 초과). 질문을 더 구체적으로 하거나 설정에서 출력 토큰을 늘려보세요.".into(),
            });
            break;
        }

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

            let key = (call.function.name.clone(), call.function.arguments.trim().to_string());
            let (ok, text) = if !executed.insert(key) {
                (false, "이미 같은 인자로 호출한 도구입니다. 위의 기존 결과를 사용하거나 다른 행동을 취하세요.".to_string())
            } else {
                let args: Value = serde_json::from_str(&call.function.arguments)
                    .unwrap_or(Value::Object(Default::default()));
                // 도구는 동기 구현 — 블로킹 실행을 런타임에 알린다
                let output = tokio::task::block_in_place(|| {
                    registry.execute(&call.function.name, &args, tool_ctx)
                });
                match output {
                    Ok(t) => (true, t),
                    Err(e) => (false, format!("오류: {e:#}")),
                }
            };
            emit(AgentEvent::ToolCallEnd {
                session_id: session_id.to_string(),
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                ok,
                result: clip(&text, 2000),
            });
            messages.push(ChatMessage::tool(call.id.clone(), clip(&text, 4000)));
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

/// 마지막 2개를 제외한 도구 결과 메시지를 짧게 압축한다. 압축한 개수를 돌려준다.
/// (컨텍스트 초과 회복용 — 최근 결과는 모델이 아직 참조 중일 수 있어 보존)
fn compact_old_tool_results(messages: &mut [ChatMessage]) -> usize {
    const KEEP_RECENT: usize = 2;
    const COMPACT_TO: usize = 300;
    let tool_idxs: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "tool")
        .map(|(i, _)| i)
        .collect();
    let mut compacted = 0;
    for &i in tool_idxs.iter().rev().skip(KEEP_RECENT) {
        if let Some(content) = &messages[i].content {
            if content.chars().count() > COMPACT_TO {
                messages[i].content = Some(format!(
                    "{}\n...(컨텍스트 절약을 위해 축약됨)",
                    content.chars().take(COMPACT_TO).collect::<String>()
                ));
                compacted += 1;
            }
        }
    }
    compacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::client::DeltaSink;
    use crate::models::{CompletionResult, FunctionCall, ToolCall};
    use std::sync::Mutex;

    /// 호출 순서대로 미리 준비한 응답(또는 오류)을 돌려주는 mock
    struct MockClient {
        responses: Mutex<Vec<Result<CompletionResult>>>,
    }

    impl MockClient {
        fn ok(responses: Vec<CompletionResult>) -> Self {
            Self { responses: Mutex::new(responses.into_iter().map(Ok).collect()) }
        }
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
            let r = self.responses.lock().unwrap().remove(0)?;
            if !r.content.is_empty() {
                sink(DeltaKind::Text, &r.content);
            }
            Ok(r)
        }
    }

    fn noop_ctx() -> ToolCtx {
        ToolCtx::noop(AppConfig::default())
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

        let client = MockClient::ok(vec![
            tool_call_result("read_file", serde_json::json!({"path": file.to_string_lossy()})),
            CompletionResult { content: "파일 내용은 '안녕하세요' 입니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![
            ChatMessage::system(system_prompt(&AppConfig::default())),
            ChatMessage::user("hello.txt 읽어줘"),
        ];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(
            &client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel,
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
        let client = MockClient::ok(vec![
            tool_call_result("read_file", serde_json::json!({"path": "C:\\없는파일.txt"})),
            CompletionResult { content: "파일을 찾을 수 없습니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("읽어줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
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
        let client = MockClient::ok(responses);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("loop")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 2, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
    }

    /// 같은 (도구, 인자) 재호출은 실행하지 않고 모델에 안내 메시지를 돌려준다
    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_tool_call_is_short_circuited() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.txt"), "x").unwrap();
        let args = serde_json::json!({"path": dir.path().to_string_lossy()});

        let client = MockClient::ok(vec![
            tool_call_result("list_dir", args.clone()),
            tool_call_result("list_dir", args.clone()), // 동일 호출 반복
            CompletionResult { content: "끝".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("목록")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        let oks = evs.iter().filter(|e| matches!(e, AgentEvent::ToolCallEnd { ok: true, .. })).count();
        let errs = evs.iter().filter(|e| matches!(e, AgentEvent::ToolCallEnd { ok: false, .. })).count();
        assert_eq!((oks, errs), (1, 1), "두 번째 호출은 실행 없이 거부돼야 함");
        assert!(messages.iter().any(|m| m.role == "tool"
            && m.content.as_deref().unwrap().contains("이미 같은 인자")));
    }

    /// 컨텍스트 초과 오류가 나면 오래된 도구 결과를 압축하고 재시도한다
    #[tokio::test(flavor = "multi_thread")]
    async fn context_overflow_compacts_and_retries() {
        let long = "가".repeat(5000);
        let client = MockClient {
            responses: Mutex::new(vec![
                Err(anyhow::anyhow!(
                    "llama-server 오류 400: request exceeds the available context size"
                )),
                Ok(CompletionResult { content: "회복됨".into(), ..Default::default() }),
            ]),
        };
        let registry = ToolRegistry::with_default_tools();
        // 압축 대상이 되도록 긴 도구 결과 3개를 히스토리에 심는다
        let mut messages = vec![
            ChatMessage::user("이전 질문"),
            ChatMessage::tool("c1", long.clone()),
            ChatMessage::tool("c2", long.clone()),
            ChatMessage::tool("c3", long.clone()),
            ChatMessage::user("다음 질문"),
        ];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .expect("압축 후 재시도로 회복해야 함");

        assert!(messages[1].content.as_deref().unwrap().contains("축약됨"), "가장 오래된 결과가 압축돼야 함");
        assert!(!messages[3].content.as_deref().unwrap().contains("축약됨"), "최근 2개는 보존");
        assert_eq!(messages.last().unwrap().content.as_deref(), Some("회복됨"));
    }

    /// 빈 완성은 1회 재생성으로 회복을 시도하고, 성공하면 오류 없이 끝난다
    #[tokio::test(flavor = "multi_thread")]
    async fn empty_completion_recovers_with_one_retry() {
        let client = MockClient::ok(vec![
            CompletionResult::default(), // 빈 완성
            CompletionResult { content: "회복된 답변".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("질문")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })), "재시도 성공 시 오류 없어야 함");
        assert_eq!(messages.last().unwrap().content.as_deref(), Some("회복된 답변"));
    }

    /// 재생성까지 비면 사용자에게 오류로 알린다
    #[tokio::test(flavor = "multi_thread")]
    async fn empty_completion_twice_surfaces_error() {
        let client = MockClient::ok(vec![CompletionResult::default(), CompletionResult::default()]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("질문")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::TurnEnd { .. })));
    }

    #[test]
    fn compact_skips_when_nothing_to_compact() {
        let mut messages = vec![ChatMessage::user("짧음"), ChatMessage::tool("c1", "짧은 결과")];
        assert_eq!(compact_old_tool_results(&mut messages), 0);
    }

    #[test]
    fn prompt_includes_workspace() {
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = r"C:\Users\EST\작업방".into();
        let p = system_prompt(&cfg);
        assert!(p.contains("작업방"), "워크스페이스 경로가 프롬프트에 없음");
        assert!(p.contains("워크스페이스 안에서만"));
    }

    #[test]
    fn prompt_asks_names_when_unknown() {
        let p = system_prompt(&AppConfig::default());
        assert!(p.contains("update_profile"), "이름 저장 도구 안내 없음");
        assert!(p.contains("지어달라고"), "이름 지어달라는 지시 없음");
    }

    #[test]
    fn prompt_uses_names_when_known() {
        let mut cfg = AppConfig::default();
        cfg.user_name = "태경".into();
        cfg.agent_name = "앨리".into();
        let p = system_prompt(&cfg);
        assert!(p.contains("'앨리'") && p.contains("'태경'"));
        assert!(!p.contains("지어달라고"), "이름이 있는데 또 묻게 함");
    }

    #[test]
    fn bg_keywords_exclude_image_transform() {
        assert_eq!(tools_to_exclude("dog.png를 배경제거 해봐"), vec!["image_transform"]);
        assert_eq!(tools_to_exclude("이 사진 누끼 따줘"), vec!["image_transform"]);
        assert_eq!(tools_to_exclude("배경을 빼서 투명하게"), vec!["image_transform"]);
        assert!(tools_to_exclude("dog.png를 90도 회전시켜줘").is_empty());
        assert!(tools_to_exclude("배경화면 바꿔줘").is_empty(), "배경화면은 배경제거가 아님");
    }

    #[test]
    fn schemas_excluding_hides_tool() {
        let registry = ToolRegistry::with_default_tools();
        let all = serde_json::to_string(&registry.schemas()).unwrap();
        assert!(all.contains("image_transform") && all.contains("remove_background"));
        let filtered =
            serde_json::to_string(&registry.schemas_excluding(&["image_transform"])).unwrap();
        assert!(!filtered.contains("\"image_transform\""));
        assert!(filtered.contains("remove_background"));
    }

    #[test]
    fn prompt_asks_only_missing_name() {
        let mut cfg = AppConfig::default();
        cfg.agent_name = "앨리".into();
        let p = system_prompt(&cfg);
        assert!(p.contains("'앨리'"));
        assert!(p.contains("사용자의 이름을 모르니"));
    }
}
