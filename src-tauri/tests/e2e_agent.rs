//! 실제 llama-server + 실제 모델로 에이전트 루프 전체를 검증하는 E2E.
//! 모델 파일이 필요하므로 기본으로는 제외 — 실행:
//!   cargo test --test e2e_agent --release -- --ignored --nocapture --test-threads=1
use local_agent_lib::agent::{run_turn, system_prompt};
use local_agent_lib::config::AppConfig;
use local_agent_lib::llm::client::HttpLlmClient;
use local_agent_lib::llm::server::LlamaServer;
use local_agent_lib::models::{AgentEvent, ChatMessage};
use local_agent_lib::tools::ToolRegistry;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::Instant;

struct Recorded {
    tool_names: Vec<String>,
    final_text: String,
    errors: Vec<String>,
}

async fn one_turn(
    client: &HttpLlmClient,
    registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    user: &str,
    budget_secs: u64,
) -> Recorded {
    messages.push(ChatMessage::user(user));
    let cancel = AtomicBool::new(false);
    let events: Mutex<Vec<AgentEvent>> = Mutex::new(Vec::new());
    let started = Instant::now();

    run_turn(client, registry, messages, "e2e", 8, 0.2, &cancel, &|ev| {
        events.lock().unwrap().push(ev)
    })
    .await
    .expect("run_turn 실패");

    let elapsed = started.elapsed().as_secs();
    let events = events.into_inner().unwrap();
    let tool_names: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallEnd { name, ok: true, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let final_text = messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && m.content.is_some())
        .and_then(|m| m.content.clone())
        .unwrap_or_default();
    let errors: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message, .. } => Some(message.clone()),
            AgentEvent::ToolCallEnd { ok: false, result, .. } => Some(result.clone()),
            _ => None,
        })
        .collect();

    println!(
        "  -> {elapsed}s | tools={tool_names:?} | errors={errors:?}\n     answer: {}",
        final_text.chars().take(200).collect::<String>()
    );
    assert!(
        elapsed <= budget_secs,
        "레이턴시 초과: {elapsed}s > {budget_secs}s (요청: {user})"
    );
    Recorded { tool_names, final_text, errors }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "실제 모델 필요 — 수동 실행"]
async fn agent_scenarios_within_latency_budget() {
    let cfg = AppConfig::default();
    let mut server = LlamaServer::new();
    server.start(&cfg).await.expect("llama-server 시작 실패");
    let client = HttpLlmClient::new(server.base_url.clone());
    let registry = ToolRegistry::with_default_tools();

    // 시나리오용 샌드박스 (홈 아래 — 시스템 프롬프트의 경로 안내와 일관)
    let home = dirs::home_dir().unwrap();
    let sandbox = home.join("local-agent-e2e");
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox).unwrap();
    std::fs::write(sandbox.join("메모.txt"), "회의는 6월 13일 오후 3시").unwrap();
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(640, 480));
    img.save(sandbox.join("sample_photo.png")).unwrap();

    // ── 1. 싱글턴: 파일 검색 ────────────────────────────────
    println!("[1] 파일 검색");
    let mut messages = vec![ChatMessage::system(system_prompt())];
    let r = one_turn(
        &client, &registry, &mut messages,
        &format!("{} 폴더에서 png 이미지 찾아줘", sandbox.display()),
        60,
    )
    .await;
    assert!(r.tool_names.iter().any(|n| n == "search_files"), "search_files 미호출");
    assert!(r.final_text.contains("sample_photo"), "검색 결과가 답변에 없음");

    // ── 2. 멀티턴: 직전 결과 후속 작업 (이미지 처리) ─────────
    println!("[2] 멀티턴 후속: 찾은 이미지 리사이즈");
    let r = one_turn(
        &client, &registry, &mut messages,
        "방금 찾은 그 이미지를 가로 320픽셀로 줄여줘",
        60,
    )
    .await;
    assert!(r.tool_names.iter().any(|n| n == "image_transform"), "image_transform 미호출");
    assert!(r.errors.is_empty(), "도구 오류 발생: {:?}", r.errors);
    let resized = sandbox.join("sample_photo_edited.png");
    assert!(resized.exists(), "리사이즈 결과 파일 없음");
    assert_eq!(image::image_dimensions(&resized).unwrap(), (320, 240));

    // ── 3. 싱글턴: 파일 읽고 내용 답변 ──────────────────────
    println!("[3] 파일 읽기 + 내용 질문");
    let mut messages2 = vec![ChatMessage::system(system_prompt())];
    let r = one_turn(
        &client, &registry, &mut messages2,
        &format!("{} 파일 읽고 회의가 언제인지 알려줘", sandbox.join("메모.txt").display()),
        60,
    )
    .await;
    assert!(r.tool_names.iter().any(|n| n == "read_file"), "read_file 미호출");
    assert!(
        r.final_text.contains("13") && r.final_text.contains('3'),
        "회의 일시가 답변에 없음: {}",
        r.final_text
    );

    // ── 4. 싱글턴: 화면 캡처 ───────────────────────────────
    println!("[4] 화면 캡처");
    let mut messages3 = vec![ChatMessage::system(system_prompt())];
    let r = one_turn(&client, &registry, &mut messages3, "지금 화면 캡처해줘", 60).await;
    assert!(r.tool_names.iter().any(|n| n == "screen_capture"), "screen_capture 미호출");

    // ── 5. 잡담: 도구 없이 즉답 ────────────────────────────
    println!("[5] 잡담 (도구 미사용)");
    let mut messages4 = vec![ChatMessage::system(system_prompt())];
    let r = one_turn(&client, &registry, &mut messages4, "고마워! 오늘 수고했어", 30).await;
    assert!(r.tool_names.is_empty(), "잡담에 도구 호출함: {:?}", r.tool_names);
    assert!(!r.final_text.is_empty());

    server.stop().await;
    let _ = std::fs::remove_dir_all(&sandbox);
    println!("E2E 전체 통과");
}
