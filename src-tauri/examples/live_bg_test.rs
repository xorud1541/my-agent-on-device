//! 진단용: 실행 중인 llama-server(8736)에 실제 에이전트 루프를 돌려
//! '배경제거' 발화가 remove_background 까지 도달하는지 종단 검증한다.
//! 사용: cargo run --example live_bg_test
use local_agent_lib::agent::{run_turn, system_prompt};
use local_agent_lib::config::AppConfig;
use local_agent_lib::llm::client::HttpLlmClient;
use local_agent_lib::models::{AgentEvent, ChatMessage};
use local_agent_lib::tools::{ToolCtx, ToolRegistry};
use std::sync::atomic::AtomicBool;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let client = HttpLlmClient::new("http://127.0.0.1:8736".into(), 1024);
    let registry = ToolRegistry::with_default_tools();

    // 샌드박스 워크스페이스 + 테스트 이미지
    let sandbox = std::env::temp_dir().join("la-bg-live");
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox)?;
    let img: image::RgbImage = image::ImageBuffer::from_fn(96, 72, |x, y| {
        if (24..72).contains(&x) && (18..54).contains(&y) {
            image::Rgb([220, 40, 40])
        } else {
            image::Rgb([245, 245, 245])
        }
    });
    let dog = sandbox.join("dog.png");
    img.save(&dog)?;

    let mut cfg = AppConfig::default();
    cfg.workspace_dir = sandbox.to_string_lossy().into_owned();
    let ctx = ToolCtx::noop(cfg.clone());

    let utterances = [
        format!("{} 배경제거 해줘", dog.display().to_string().replace('\\', "/")),
        "dog.png를 배경제거 해봐".to_string(),
        "워크스페이스의 dog.png 누끼 따줘".to_string(),
    ];

    let mut pass = 0;
    for (i, utt) in utterances.iter().enumerate() {
        // 이전 라운드 출력물 제거 (파일 존재 여부로 성공 판정)
        for e in std::fs::read_dir(&sandbox)? {
            let p = e?.path();
            if p.file_name().map(|n| n.to_string_lossy().contains("_nobg")).unwrap_or(false) {
                std::fs::remove_file(p)?;
            }
        }
        println!("--- [{i}] {utt}");
        let mut messages =
            vec![ChatMessage::system(system_prompt(&cfg)), ChatMessage::user(utt.clone())];
        run_turn(&client, &registry, &ctx, &mut messages, "diag", 8, 0.4, &AtomicBool::new(false), &|ev| {
            match ev {
                AgentEvent::ToolCallStart { name, arguments, .. } => {
                    println!("    CALL {name} {}", arguments.chars().take(120).collect::<String>());
                }
                AgentEvent::ToolCallEnd { name, ok, result, .. } => {
                    println!("    -> {name} ok={ok} {}", result.chars().take(100).collect::<String>());
                }
                AgentEvent::Error { message, .. } => println!("    ERROR {message}"),
                _ => {}
            }
        })
        .await?;

        let nobg_exists = std::fs::read_dir(&sandbox)?
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("_nobg"));
        println!("    => 결과 파일 생성: {nobg_exists}");
        if nobg_exists {
            pass += 1;
        }
    }
    println!("\n통과 {pass}/{}", utterances.len());
    Ok(())
}
