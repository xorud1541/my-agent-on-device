use super::{opt_u64, req_str, Tool, ToolCtx};
use anyhow::{Context, Result};
use serde_json::{json, Value};

const DEFAULT_MAX_CHARS: u64 = 20_000;

pub struct PdfExtractText;

impl Tool for PdfExtractText {
    fn name(&self) -> &'static str {
        "pdf_extract_text"
    }
    fn description(&self) -> &'static str {
        "PDF 파일에서 텍스트를 추출한다. PDF 요약/검색/질문답변 전에 반드시 먼저 호출한다."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "PDF 절대경로" },
                "max_chars": { "type": "integer", "description": "최대 추출 글자 수 (기본 20000)" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value, _ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        let max_chars = opt_u64(args, "max_chars").unwrap_or(DEFAULT_MAX_CHARS) as usize;
        let text = pdf_extract::extract_text(path)
            .with_context(|| format!("PDF 텍스트 추출 실패: {path}"))?;
        let text = text.trim();
        if text.is_empty() {
            return Ok("(텍스트 없음 — 스캔본이거나 이미지 기반 PDF일 수 있음)".into());
        }
        let truncated: String = text.chars().take(max_chars).collect();
        if truncated.len() < text.len() {
            Ok(format!(
                "{truncated}\n...(잘림: 전체 {}자 중 {max_chars}자)",
                text.chars().count()
            ))
        } else {
            Ok(truncated)
        }
    }
}
