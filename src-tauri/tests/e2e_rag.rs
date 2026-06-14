//! 실제 llama-server + LocalSearch 사이드카로 RAG 응답 경로 전체를 검증하는 E2E.
//! 앱의 send_message 경로(검색 → rag_context 주입 → run_turn)를 그대로 재현한다.
//!
//! 실행 (모델·바이너리 필요):
//!   LOCALSEARCH_CLI_BIN=<.../localsearch-cli> \
//!   LOCALSEARCH_MODELS_DIR=<harrier 부모> \
//!   ORT_DYLIB_PATH=/opt/homebrew/lib/libonnxruntime.dylib \
//!   cargo test --test e2e_rag --release -- --ignored --nocapture --test-threads=1
use local_agent_lib::agent::{run_turn, system_prompt};
use local_agent_lib::config::AppConfig;
use local_agent_lib::llm::client::HttpLlmClient;
use local_agent_lib::llm::server::LlamaServer;
use local_agent_lib::localsearch::{
    run_index, LocalSearchConfig, LocalSearchServer, SearchClient, RAG_MARKER, RAG_MIN_COSINE,
    RAG_TOP_K,
};
use local_agent_lib::models::{AgentEvent, ChatMessage};
use local_agent_lib::tools::{ToolCtx, ToolRegistry};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

fn ls_config(db_dir: PathBuf) -> LocalSearchConfig {
    LocalSearchConfig {
        binary: std::env::var("LOCALSEARCH_CLI_BIN").expect("LOCALSEARCH_CLI_BIN").into(),
        models_dir: std::env::var("LOCALSEARCH_MODELS_DIR").expect("LOCALSEARCH_MODELS_DIR").into(),
        db_dir,
        ort_dylib: std::env::var("ORT_DYLIB_PATH").ok().map(Into::into),
        port: 11239,
    }
}

/// 앱의 send_message 와 동일하게: rag_context 로 근거 블록을 system 에 합치고 run_turn.
async fn rag_turn(
    llm: &HttpLlmClient,
    search: &SearchClient,
    registry: &ToolRegistry,
    query: &str,
) -> (bool, String) {
    let mut messages = vec![ChatMessage::system(system_prompt(&AppConfig::default()))];
    messages.push(ChatMessage::user(query.to_string()));

    let sys_backup = messages.first().and_then(|m| m.content.clone());
    let rag = search.rag_context(query, RAG_TOP_K, RAG_MIN_COSINE).await;
    let rag_fired = rag.is_some();
    if let (Some(block), Some(s0)) = (&rag, messages.first_mut()) {
        s0.content = Some(format!("{}\n\n{}", sys_backup.as_deref().unwrap_or(""), block));
    }

    let cancel = AtomicBool::new(false);
    let events: Mutex<Vec<AgentEvent>> = Mutex::new(Vec::new());
    let ctx = ToolCtx::noop(AppConfig::default());
    run_turn(llm, registry, &ctx, &mut messages, "e2e-rag", 8, 0.2, &cancel, &|ev| {
        events.lock().unwrap().push(ev)
    })
    .await
    .expect("run_turn 실패");

    let answer = messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && m.content.is_some())
        .and_then(|m| m.content.clone())
        .unwrap_or_default();
    (rag_fired, answer)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "실제 모델·사이드카 필요 — 수동 실행"]
async fn rag_answers_from_indexed_pdfs() {
    let pdf_dir = dirs::home_dir().unwrap().join("Downloads/test_alice/PDF_테스트");
    assert!(pdf_dir.is_dir(), "PDF 테스트 폴더 없음: {}", pdf_dir.display());

    // 1) 색인 (serve 전에 index 서브프로세스로 채운다 — 같은 db_dir 공유)
    let db = std::env::temp_dir().join("e2e_rag_db");
    let _ = std::fs::remove_dir_all(&db);
    std::fs::create_dir_all(&db).unwrap();
    let lc = ls_config(db.clone());
    let summary = run_index(&lc, &pdf_dir.to_string_lossy()).expect("인덱싱 실패");
    println!("[rag-e2e] 인덱싱: {summary:?}");
    assert!(summary.indexed > 0);

    // 2) 사이드카(serve) 기동 → SearchClient
    let mut ls = LocalSearchServer::new();
    let search = ls.start(&lc).await.expect("localsearch 사이드카 기동 실패");

    // 3) llama-server 기동
    let mut server = LlamaServer::new();
    server.start(&AppConfig::default()).await.expect("llama-server 시작 실패");
    let llm = HttpLlmClient::new(server.base_url.clone(), 1024);
    let registry = ToolRegistry::with_default_tools();

    // 4) RAG 질의 — 인덱싱된 PDF 내용에 근거해 답해야 한다
    println!("\n[rag-e2e] ■ 질의: 건강검진에서 콜레스테롤 수치가 어땠어?");
    let (fired, ans) = rag_turn(&llm, &search, &registry, "건강검진에서 콜레스테롤 수치가 어땠어?").await;
    println!("[rag-e2e] RAG 발동={fired}\n[rag-e2e] 답변: {ans}\n");
    assert!(fired, "관련 PDF가 있는데 RAG 가 발동하지 않음");
    assert!(!ans.is_empty(), "빈 응답");
    assert!(!ans.contains(RAG_MARKER), "근거 블록 마커가 답변에 누출됨");

    // 5) 대조군 — 색인과 무관한 잡담은 RAG 가 발동하지 않아야 한다(일반대화)
    println!("[rag-e2e] ■ 대조 질의: 안녕 반가워");
    let (fired2, ans2) = rag_turn(&llm, &search, &registry, "안녕 반가워").await;
    println!("[rag-e2e] RAG 발동={fired2}\n[rag-e2e] 답변: {ans2}\n");
    assert!(!fired2, "잡담에 RAG 가 발동함(임계값 게이트 실패)");

    server.stop().await;
    ls.stop().await;
}
