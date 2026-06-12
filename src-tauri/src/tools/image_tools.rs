use super::{opt_str, opt_u64, req_str, Tool, ToolCtx};
use crate::tools::workspace::ensure_in_workspace;
use anyhow::{bail, Context, Result};
use image::ImageFormat;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct ImageInfo;

impl Tool for ImageInfo {
    fn name(&self) -> &'static str {
        "image_info"
    }
    fn description(&self) -> &'static str {
        "이미지의 크기(가로x세로), 포맷, 파일 크기를 조회한다."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "이미지 절대경로" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        // 존재 확인을 먼저 — 위치 힌트가 os 에러 꼬리 없이 문장 끝에 오도록
        if !Path::new(path).exists() {
            bail!(crate::tools::not_found_msg(path, &ctx.workspace()));
        }
        let meta =
            std::fs::metadata(path).with_context(|| format!("파일 정보 조회 실패: {path}"))?;
        let img = image::image_dimensions(path).with_context(|| {
            format!("이 파일은 이미지가 아닙니다: {path}. 이 사실을 그대로 사용자에게 알리세요.")
        })?;
        let format = ImageFormat::from_path(path)
            .map(|f| format!("{f:?}"))
            .unwrap_or_else(|_| "unknown".into());
        Ok(format!(
            "{path}: {}x{} px, 포맷 {format}, {} bytes",
            img.0,
            img.1,
            meta.len()
        ))
    }
}

pub struct ImageTransform;

impl Tool for ImageTransform {
    fn name(&self) -> &'static str {
        "image_transform"
    }
    fn description(&self) -> &'static str {
        "이미지를 리사이즈/회전/그레이스케일/포맷변환한다. output_path 를 생략하면 원본 옆에 _edited 로 저장."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "원본 이미지 절대경로" },
                "output_path": { "type": "string", "description": "저장할 경로 (생략 가능)" },
                "resize_width": { "type": "integer", "description": "목표 가로(너비) 픽셀, 비율 유지. 사용자가 '가로 N으로'라고 하면 이 인자를 쓴다" },
                "resize_height": { "type": "integer", "description": "목표 세로(높이) 픽셀, 비율 유지. 사용자가 '세로 N으로'라고 하면 이 인자를 쓴다" },
                "rotate": { "type": "integer", "enum": [90, 180, 270], "description": "시계방향 회전 각도" },
                "grayscale": { "type": "boolean", "description": "흑백 변환" },
                "format": { "type": "string", "enum": ["png", "jpeg", "webp", "bmp"], "description": "출력 포맷" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        // 존재 확인을 먼저 — 없는 파일이면 위치 힌트/질문 지시로 회복 경로를 준다
        if !Path::new(path).exists() {
            bail!(crate::tools::not_found_msg(path, &ctx.workspace()));
        }
        // content-sniffing — 확장자와 내용이 다른 파일도 연다
        let mut img = super::image_ai::open_image_sniffed(Path::new(path))?;
        let (ow, oh) = (img.width(), img.height());
        let mut ops = Vec::new();

        let rw = opt_u64(args, "resize_width").map(|v| v as u32);
        let rh = opt_u64(args, "resize_height").map(|v| v as u32);
        if rw.is_some() || rh.is_some() {
            let w = rw.unwrap_or(u32::MAX);
            let h = rh.unwrap_or(u32::MAX);
            img = img.resize(w, h, image::imageops::FilterType::Lanczos3);
            ops.push(format!(
                "리사이즈 {}x{} -> {}x{}",
                ow,
                oh,
                img.width(),
                img.height()
            ));
        }
        match opt_u64(args, "rotate") {
            Some(90) => {
                img = img.rotate90();
                ops.push("회전 90°".into());
            }
            Some(180) => {
                img = img.rotate180();
                ops.push("회전 180°".into());
            }
            Some(270) => {
                img = img.rotate270();
                ops.push("회전 270°".into());
            }
            Some(other) => bail!("지원하지 않는 회전 각도: {other} (90/180/270만 가능)"),
            None => {}
        }
        if args
            .get("grayscale")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            img = img.grayscale();
            ops.push("흑백 변환".into());
        }

        let format = opt_str(args, "format");
        let out_path = resolve_output_path(path, opt_str(args, "output_path"), format)?;
        // 이름만 온 출력 경로는 워크스페이스로 흡수 (2026-06-12 R7 패턴)
        let out_path = crate::tools::workspace::absorb_into_workspace(
            &out_path.to_string_lossy(),
            &ctx.workspace(),
        );
        ensure_in_workspace(&out_path.to_string_lossy(), &ctx.workspace())?;
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // JPEG 는 알파 채널을 지원하지 않으므로 RGB 로 강제
        let is_jpeg = out_path
            .extension()
            .map(|e| {
                let e = e.to_string_lossy().to_lowercase();
                e == "jpg" || e == "jpeg"
            })
            .unwrap_or(false);
        if is_jpeg {
            img = image::DynamicImage::ImageRgb8(img.to_rgb8());
        }
        img.save(&out_path)
            .with_context(|| format!("저장 실패: {}", out_path.display()))?;
        if ops.is_empty() {
            ops.push("포맷 변환".into());
        }
        Ok(format!("{} 완료 -> {}", ops.join(", "), out_path.display()))
    }
}

fn resolve_output_path(input: &str, output: Option<&str>, format: Option<&str>) -> Result<PathBuf> {
    if let Some(o) = output {
        return Ok(PathBuf::from(o));
    }
    let p = Path::new(input);
    let stem = p.file_stem().context("잘못된 경로")?.to_string_lossy();
    let ext = match format {
        Some(f) => f.to_string(),
        None => p
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| "png".into()),
    };
    Ok(p.with_file_name(format!("{stem}_edited.{ext}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::ctx_with_workspace;
    use tempfile::tempdir;

    fn make_png(dir: &Path, name: &str, w: u32, h: u32) -> String {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(w, h));
        let p = dir.join(name);
        img.save(&p).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn info_reports_dimensions() {
        let dir = tempdir().unwrap();
        let p = make_png(dir.path(), "a.png", 32, 16);
        let out = ImageInfo
            .execute(&json!({"path": p}), &ctx_with_workspace(dir.path()))
            .unwrap();
        assert!(out.contains("32x16"));
    }

    #[test]
    fn resize_keeps_aspect_ratio() {
        let dir = tempdir().unwrap();
        let p = make_png(dir.path(), "a.png", 100, 50);
        let out = ImageTransform
            .execute(
                &json!({"path": p, "resize_width": 40}),
                &ctx_with_workspace(dir.path()),
            )
            .unwrap();
        assert!(out.contains("40x20"), "{out}");
        assert!(dir.path().join("a_edited.png").exists());
    }

    #[test]
    fn convert_to_jpeg_drops_alpha() {
        let dir = tempdir().unwrap();
        let p = make_png(dir.path(), "a.png", 10, 10);
        let out = ImageTransform
            .execute(
                &json!({"path": p, "format": "jpeg"}),
                &ctx_with_workspace(dir.path()),
            )
            .unwrap();
        assert!(out.contains("a_edited.jpeg"), "{out}");
    }
}
