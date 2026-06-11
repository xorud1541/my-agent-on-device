use super::{opt_str, opt_u64, Tool, ToolCtx};
use crate::tools::workspace::ensure_in_workspace;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ScreenCapture;

impl Tool for ScreenCapture {
    fn name(&self) -> &'static str {
        "screen_capture"
    }
    fn description(&self) -> &'static str {
        "현재 화면을 캡처해 PNG 파일로 저장한다. 결과로 저장 경로를 돌려준다."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "monitor_index": { "type": "integer", "description": "캡처할 모니터 번호 (기본 0 = 주 모니터)" },
                "output_path": { "type": "string", "description": "저장 경로 (생략하면 사진 폴더에 자동 저장)" }
            },
            "required": []
        })
    }
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let idx = opt_u64(args, "monitor_index").unwrap_or(0) as usize;
        let monitors = xcap::Monitor::all().context("모니터 조회 실패")?;
        let monitor = monitors
            .get(idx)
            .with_context(|| format!("모니터 {idx} 없음 (총 {}개)", monitors.len()))?;
        let image = monitor.capture_image().context("화면 캡처 실패")?;

        let out_path = match opt_str(args, "output_path") {
            Some(p) => PathBuf::from(p),
            None => {
                // 기본 저장 위치는 워크스페이스 아래 captures/
                let dir = ctx.workspace().join("captures");
                std::fs::create_dir_all(&dir)?;
                dir.join(format!(
                    "capture_{}.png",
                    chrono::Local::now().format("%Y%m%d_%H%M%S")
                ))
            }
        };
        ensure_in_workspace(&out_path.to_string_lossy(), &ctx.workspace())?;
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        image.save(&out_path).context("캡처 저장 실패")?;
        Ok(format!(
            "캡처 완료: {} ({}x{} px)",
            out_path.display(),
            image.width(),
            image.height()
        ))
    }
}
