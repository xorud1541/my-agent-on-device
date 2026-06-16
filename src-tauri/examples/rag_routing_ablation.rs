//! RAG 라우팅 게이트 2×2 절제(±A × ±B) 측정 하니스.
//! bench/rag_routing_eval.json 의 케이스를 4셀로 돌려 라우팅 정확도/회복률을 뽑는다.
//!
//! - A = tool_intent 키워드 게이트 (env RAG_DISABLE_TOOL_INTENT 로 끔)
//! - B = RAG 턴에도 조회 도구 유지 (env RAG_KEEP_READ_TOOLS 로 켬)
//! 판정은 답변 텍스트가 아니라 **이벤트(어느 도구가 불렸나 / Sources 떴나) + 파일근거**로.
//!
//! 라이브: llama-server(8736) + localsearch 사이드카(11434) 필요.
//! 워크스페이스는 ~/.alice/images 를 temp 로 얕은 복사(루트 파일만, 하위는 빈 디렉토리)해 보호.
//! 사용: cargo run --example rag_routing_ablation --release
use local_agent_lib::agent::{run_turn, system_prompt, tool_intent, tool_intent_disabled};
use local_agent_lib::config::AppConfig;
use local_agent_lib::llm::client::HttpLlmClient;
use local_agent_lib::localsearch::{SearchClient, RAG_MIN_COSINE, RAG_TOP_K};
use local_agent_lib::models::{AgentEvent, ChatMessage};
use local_agent_lib::tools::{ToolCtx, ToolRegistry};
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// 워크스페이스 루트를 temp 로 얕게 복사: 파일은 실복사, 하위 디렉토리는 빈 placeholder.
/// (라우팅 평가는 루트 파일 + list_dir 출력만 필요 — 하위 내용 불필요, 복사비용 절감)
fn shallow_copy_ws(src: &Path, dst: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_dir_all(dst);
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let p = entry.path();
        let target = dst.join(entry.file_name());
        if p.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            std::fs::copy(&p, &target)?;
        }
    }
    Ok(())
}

struct Observed {
    rag_fired: bool,
    tools: Vec<String>,
    answer: String,
}

async fn run_case(
    client: &HttpLlmClient,
    registry: &ToolRegistry,
    search: &SearchClient,
    cfg: &AppConfig,
    utterance: &str,
) -> Observed {
    let ctx = ToolCtx::noop(cfg.clone());
    let mut messages = vec![ChatMessage::system(system_prompt(cfg))];

    // ── RAG 프리훅: commands.rs send_message 와 동일 로직 (A 게이트 포함) ──
    let rag = if !tool_intent_disabled() && tool_intent(utterance) {
        None
    } else {
        search.rag_context(utterance, RAG_TOP_K, RAG_MIN_COSINE).await
    };
    let rag_fired = rag.is_some();
    if let Some(rc) = &rag {
        if let Some(sys0) = messages.first_mut() {
            let backup = sys0.content.clone().unwrap_or_default();
            sys0.content = Some(format!("{backup}\n\n{}", rc.block));
        }
    }
    messages.push(ChatMessage::user(utterance.to_string()));

    let events: Mutex<Vec<AgentEvent>> = Mutex::new(Vec::new());
    let _ = run_turn(
        client, registry, &ctx, &mut messages, "ablation", 8, 0.2,
        &AtomicBool::new(false),
        &|ev| events.lock().unwrap().push(ev),
    )
    .await;

    let events = events.into_inner().unwrap();
    let tools: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallStart { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let answer = messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && m.content.as_deref().is_some_and(|c| !c.is_empty()))
        .and_then(|m| m.content.clone())
        .unwrap_or_default();
    Observed { rag_fired, tools, answer }
}

const READ_TOOLS: &[&str] = &["list_dir", "read_file", "search_files", "image_info", "pdf_extract_text"];

#[derive(Default)]
struct Tally {
    route_ok: u32,
    catastrophic: u32,
    recovered: u32,
    grounded_ok: u32,
    grounded_total: u32,
    n: u32,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let eval_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/rag_routing_eval.json");
    let eval: Value = serde_json::from_str(&std::fs::read_to_string(&eval_path)?)?;
    let cases = eval["cases"].as_array().expect("cases 배열").clone();

    let client = HttpLlmClient::new("http://127.0.0.1:8736".into(), 1024, false);
    let registry = ToolRegistry::with_default_tools();
    let search = SearchClient::new("http://127.0.0.1:11434");

    let home = dirs::home_dir().unwrap();
    let real_ws = home.join(".alice").join("images");

    // 4셀: (A켜짐?, B켜짐?)
    let cells = [
        ("-A -B (순수코사인, 전면차단)", false, false),
        ("+A -B (현재=A단독)", true, false),
        ("-A +B (코사인+백스톱)", false, true),
        ("+A +B (둘다)", true, true),
    ];

    let mut summary: Vec<(String, Tally)> = Vec::new();

    for (label, a_on, b_on) in cells {
        // 토글: A 끄기 = RAG_DISABLE_TOOL_INTENT, B 켜기 = RAG_KEEP_READ_TOOLS
        if a_on { std::env::remove_var("RAG_DISABLE_TOOL_INTENT"); }
        else { std::env::set_var("RAG_DISABLE_TOOL_INTENT", "1"); }
        if b_on { std::env::set_var("RAG_KEEP_READ_TOOLS", "1"); }
        else { std::env::remove_var("RAG_KEEP_READ_TOOLS"); }

        let ws = std::env::temp_dir().join("la-ablation-ws");
        shallow_copy_ws(&real_ws, &ws)?;
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();

        println!("\n══════════ CELL {label} ══════════");
        let mut t = Tally::default();
        for c in &cases {
            let id = c["id"].as_str().unwrap_or("?");
            let utt = c["utterance"].as_str().unwrap_or("");
            let route = c["expected_route"].as_str().unwrap_or("");
            let expect: Vec<&str> = c["expect_tools"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();

            // 케이스마다 워크스페이스를 깨끗이 (쓰기도구 부작용 격리)
            shallow_copy_ws(&real_ws, &ws)?;
            let obs = run_case(&client, &registry, &search, &cfg, utt).await;

            t.n += 1;
            let tool_hit = expect.iter().any(|e| obs.tools.iter().any(|got| got == e));
            let wrote = obs.tools.iter().any(|got| !READ_TOOLS.contains(&got.as_str()));

            let (mark, route_ok) = if route == "tool" {
                let ok = tool_hit;
                if ok { t.route_ok += 1; }
                if obs.rag_fired && !tool_hit { t.catastrophic += 1; }
                if obs.rag_fired && tool_hit { t.recovered += 1; }
                (if ok { "✅" } else if obs.rag_fired { "❌하이재킹" } else { "❌무도구" }, ok)
            } else {
                let ok = obs.rag_fired && !wrote;
                if ok { t.route_ok += 1; }
                (if ok { "✅" } else { "❌" }, ok)
            };

            // 답변 근거 (선택)
            let must: Vec<&str> = c["answer_must_mention"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            let mustnot: Vec<&str> = c["answer_must_not_mention"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            if !must.is_empty() || !mustnot.is_empty() {
                t.grounded_total += 1;
                let g = must.iter().all(|m| obs.answer.contains(m)) && !mustnot.iter().any(|m| obs.answer.contains(m));
                if g { t.grounded_ok += 1; }
            }

            let _ = route_ok;
            println!(
                "  {mark:10} {id} route={route:4} rag={} tools=[{}]  {utt}",
                if obs.rag_fired { "ON " } else { "off" },
                obs.tools.join(",")
            );
        }
        println!(
            "  ── {label}: route_ok {}/{}, 하이재킹 {}, 회복(B) {}, 근거 {}/{}",
            t.route_ok, t.n, t.catastrophic, t.recovered, t.grounded_ok, t.grounded_total
        );
        summary.push((label.to_string(), t));
    }

    println!("\n\n════════════ 2×2 절제 요약 ════════════");
    println!("{:<28} {:>10} {:>10} {:>10} {:>10}", "셀", "route_ok", "하이재킹", "회복(B)", "근거");
    for (label, t) in &summary {
        println!(
            "{:<28} {:>8}/{:<2} {:>10} {:>10} {:>6}/{:<2}",
            label, t.route_ok, t.n, t.catastrophic, t.recovered, t.grounded_ok, t.grounded_total
        );
    }
    Ok(())
}
