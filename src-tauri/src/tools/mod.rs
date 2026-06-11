mod capture;
mod fs_tools;
mod image_tools;
mod pdf_tools;
mod search;

use anyhow::Result;
use serde_json::Value;

/// 에이전트가 호출할 수 있는 도구. 구현은 동기 — 에이전트 루프에서 spawn_blocking 으로 돌린다.
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON Schema (OpenAI function parameters 규격)
    fn parameters(&self) -> Value;
    fn execute(&self, args: &Value) -> Result<String>;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn with_default_tools() -> Self {
        Self {
            tools: vec![
                Box::new(fs_tools::ListDir),
                Box::new(fs_tools::ReadFile),
                Box::new(fs_tools::WriteFile),
                Box::new(fs_tools::MovePath),
                Box::new(fs_tools::CopyPath),
                Box::new(fs_tools::DeletePath),
                Box::new(search::SearchFiles),
                Box::new(image_tools::ImageInfo),
                Box::new(image_tools::ImageTransform),
                Box::new(pdf_tools::PdfExtractText),
                Box::new(capture::ScreenCapture),
            ],
        }
    }

    /// OpenAI `tools` 배열로 직렬화
    pub fn schemas(&self) -> Value {
        Value::Array(
            self.tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name(),
                            "description": t.description(),
                            "parameters": t.parameters(),
                        }
                    })
                })
                .collect(),
        )
    }

    pub fn execute(&self, name: &str, args: &Value) -> Result<String> {
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.execute(args),
            None => anyhow::bail!("알 수 없는 도구: {name}"),
        }
    }
}

/// 인자 추출 헬퍼
pub(crate) fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("필수 인자 누락: {key}"))
}

pub(crate) fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub(crate) fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

pub(crate) fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}
