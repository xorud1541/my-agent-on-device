use crate::models::ChatMessage;
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;

/// 로그 디렉토리: %APPDATA%/com.estsoft.local-agent/logs
/// LOCAL_AGENT_LOG_DIR 환경변수로 재지정 가능 — 테스트가 실사용 로그를 오염시키지 않게.
pub fn log_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LOCAL_AGENT_LOG_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.estsoft.local-agent")
        .join("logs")
}

fn append_jsonl(file: &str, value: serde_json::Value) {
    let dir = log_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(file))
    {
        let _ = writeln!(f, "{value}");
    }
}

/// 턴 종료 후 이번 턴에서 추가된 메시지들(user/assistant/tool)을 대화 로그에 남긴다.
pub fn log_turn(
    session_id: &str,
    new_messages: &[ChatMessage],
    elapsed_ms: u64,
    error: Option<&str>,
) {
    let today = chrono::Local::now().format("%Y%m%d");
    append_jsonl(
        &format!("chat_{today}.jsonl"),
        json!({
            "ts": chrono::Local::now().to_rfc3339(),
            "session_id": session_id,
            "elapsed_ms": elapsed_ms,
            "error": error,
            "messages": new_messages,
        }),
    );
}

pub fn llama_server_log_file() -> PathBuf {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir.join("llama-server.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_turn_appends_jsonl_line() {
        // 실사용 %APPDATA% 로그를 오염시키지 않도록 임시 폴더로 격리
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("LOCAL_AGENT_LOG_DIR", tmp.path());

        let msgs = vec![
            ChatMessage::user("테스트"),
            ChatMessage::assistant(Some("응답".into()), None),
        ];
        log_turn("test-session", &msgs, 1234, None);
        let today = chrono::Local::now().format("%Y%m%d");
        let path = tmp.path().join(format!("chat_{today}.jsonl"));
        let content = std::fs::read_to_string(&path).unwrap();
        std::env::remove_var("LOCAL_AGENT_LOG_DIR");

        let last = content.lines().last().unwrap();
        let v: serde_json::Value = serde_json::from_str(last).unwrap();
        assert_eq!(v["session_id"], "test-session");
        assert_eq!(v["messages"][0]["content"], "테스트");
    }
}
