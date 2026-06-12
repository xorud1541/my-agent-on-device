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
        // 비-PDF 가드 — 2B 는 OCR 요청("화면 텍스트 긁어줘")에 png 를 이 도구로 반복
        // 시도하며 배회하고 거짓 희망("흑백 변환 후 재시도")까지 안내한다 (2026-06-12
        // 기획자 테스트 턴 50/59/62/63). '알리세요' 마커로 1라운드 정직 종결을 이끈다.
        // 확장자가 .pdf 가 아니어도 헤더가 %PDF 면 진짜 PDF 로 보고 통과시킨다.
        let ext = Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if ext != "pdf" {
            let is_pdf_header = std::fs::File::open(path)
                .and_then(|mut f| {
                    use std::io::Read;
                    let mut head = [0u8; 5];
                    f.read_exact(&mut head).map(|_| head.starts_with(b"%PDF"))
                })
                .unwrap_or(false);
            if !is_pdf_header {
                const IMAGE_EXTS: &[&str] =
                    &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "tif"];
                if IMAGE_EXTS.contains(&ext.as_str()) {
                    bail!(
                        "이 파일은 PDF가 아니라 이미지(.{ext})입니다. 이미지 속 글자를 읽는 \
                         기능(OCR)은 없습니다 — 이 사실을 그대로 사용자에게 알리세요."
                    );
                }
                bail!(
                    "이 파일은 PDF가 아닙니다(.{ext}). PDF 텍스트 추출은 .pdf 파일에만 \
                     사용할 수 있습니다 — 이 사실을 그대로 사용자에게 알리세요."
                );
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::ctx_with_workspace;
    use serde_json::json;
    use tempfile::tempdir;

    /// 이미지에 pdf_extract_text 를 쓰면 OCR 부재를 알리는 '알리세요' 마커 에러를 준다
    /// (2026-06-12 기획자 테스트: "화면 텍스트 긁어줘"에 png 추출을 반복하며 배회)
    #[test]
    fn image_input_gets_ocr_unavailable_marker() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let p = dir.path().join("capture.png");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\n....").unwrap();
        let err = PdfExtractText
            .execute(&json!({"path": p.to_string_lossy()}), &ctx)
            .unwrap_err()
            .to_string();
        assert!(err.contains("OCR"), "{err}");
        assert!(err.contains("사용자에게 알리세요"), "{err}");
    }

    /// 일반 비-PDF(txt 등)도 마커 에러로 거른다 (턴 50: pngs.txt 를 PDF 로 추출 시도)
    #[test]
    fn non_pdf_input_gets_marker_error() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let p = dir.path().join("pngs.txt");
        std::fs::write(&p, "텍스트").unwrap();
        let err = PdfExtractText
            .execute(&json!({"path": p.to_string_lossy()}), &ctx)
            .unwrap_err()
            .to_string();
        assert!(err.contains("PDF가 아닙니다"), "{err}");
        assert!(err.contains("사용자에게 알리세요"), "{err}");
    }

    /// 확장자가 없어도 %PDF 헤더면 진짜 PDF 로 보고 가드를 통과시킨다 (오탐 방지)
    #[test]
    fn pdf_header_without_extension_passes_guard() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let p = dir.path().join("doc");
        std::fs::write(&p, b"%PDF-1.5 broken").unwrap();
        let err = PdfExtractText
            .execute(&json!({"path": p.to_string_lossy()}), &ctx)
            .unwrap_err()
            .to_string();
        // 가드가 아니라 파서 단계까지 도달해야 한다
        assert!(!err.contains("PDF가 아닙니다"), "{err}");
        assert!(err.contains("추출 실패"), "{err}");
    }
}
