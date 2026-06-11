//! 진단용: 실제 앱과 동일한 시스템 프롬프트 + 도구 스키마를 JSON 으로 덤프한다.
//! 사용: cargo run --example dump_request > ../target/dump.json
fn main() {
    let cfg = local_agent_lib::config::AppConfig::load();
    let registry = local_agent_lib::tools::ToolRegistry::with_default_tools();
    let out = serde_json::json!({
        "system": local_agent_lib::agent::system_prompt(&cfg),
        "tools": registry.schemas(),
    });
    println!("{}", serde_json::to_string(&out).unwrap());
}
