use super::{opt_u64, req_str, Tool, ToolCtx};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

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
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        // 존재 확인을 먼저 — 없는 파일이면 위치 힌트/질문 지시로 회복 경로를 준다
        // (2026-06-12: PDF 발화에서 "추출 실패"가 막다른 골목이 되던 격차)
        if !Path::new(path).exists() {
            bail!(crate::tools::not_found_msg(path, &ctx.workspace()));
        }
        let max_chars = opt_u64(args, "max_chars").unwrap_or(DEFAULT_MAX_CHARS) as usize;
        let text = pdf_extract::extract_text(path)
            .with_context(|| format!("PDF 텍스트 추출 실패: {path}"))?;
        let text = text.trim();
        if text.is_empty() {
            // 결과가 곧 지시가 되도록 쓴다 — 2B 는 빈 결과를 받아들이지 못하고 다른
            // 도구로 배회하거나 내용을 지어낸다 (2026-06-12 R3 실측: read_file 로 원시
            // PDF 바이트를 읽으러 감). 양성 지시만 사용 (부정문은 환각 재료가 된다).
            return Ok(
                "이 PDF는 글자가 없는 스캔본(이미지 기반)이라 텍스트 추출 결과가 \
                       비어 있습니다. 이 사실을 그대로 사용자에게 알리고 마무리하세요."
                    .into(),
            );
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
