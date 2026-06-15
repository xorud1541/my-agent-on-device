//! ONNX 기반 이미지 AI 도구. alian(alice-tools-image-ai) 선작업 포팅.

use super::{opt_str, req_str, Tool, ToolCtx};
use crate::tools::workspace::ensure_in_workspace;
use anyhow::{bail, Context, Result};
use image::{DynamicImage, GenericImageView, RgbaImage};
use ndarray::{Array4, ArrayD, IxDyn};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MODEL_INPUT_SIZE: u32 = 768;
const SUPPORTED_INPUT_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "tiff", "tif"];

/// ort load-dynamic 용 onnxruntime.dll 경로 설정.
/// System32 의 구버전 dll 에 잘못 바인딩되지 않도록 명시 경로를 쓴다.
/// (배포: exe 옆 리소스, 개발/테스트: src-tauri/vendor/onnxruntime/)
pub fn ensure_ort_dylib() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        if std::env::var_os("ORT_DYLIB_PATH").is_some() {
            return;
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("onnxruntime.dll"));
                candidates.push(
                    dir.join("vendor")
                        .join("onnxruntime")
                        .join("onnxruntime.dll"),
                );
            }
        }
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("vendor")
                .join("onnxruntime")
                .join("onnxruntime.dll"),
        );
        if let Some(found) = candidates.into_iter().find(|p| p.exists()) {
            std::env::set_var("ORT_DYLIB_PATH", &found);
        }
    });
}

/// ort Session 래퍼 (alian ort_model.rs 포팅)
struct OrtModel {
    session: ort::session::Session,
}

impl OrtModel {
    fn load(path: &Path) -> Result<Self> {
        ensure_ort_dylib();
        let session = ort::session::Session::builder()
            .context("ONNX 세션 생성 실패")?
            .commit_from_file(path)
            .with_context(|| format!("모델 로드 실패: {}", path.display()))?;
        Ok(Self { session })
    }

    fn run(&mut self, inputs: Vec<ArrayD<f32>>) -> Result<Vec<ArrayD<f32>>> {
        use ort::session::SessionInputValue;
        use ort::value::TensorRef;
        let input_refs: Vec<TensorRef<'_, f32>> = inputs
            .iter()
            .map(TensorRef::from_array_view)
            .collect::<ort::Result<Vec<_>>>()
            .context("입력 텐서 생성 실패")?;
        let session_inputs: Vec<SessionInputValue<'_>> =
            input_refs.into_iter().map(Into::into).collect();
        let outputs = self
            .session
            .run(session_inputs.as_slice())
            .context("추론 실패")?;
        let mut result = Vec::with_capacity(outputs.len());
        for (i, (_, value)) in outputs.iter().enumerate() {
            let (shape, data) = value
                .try_extract_tensor::<f32>()
                .with_context(|| format!("출력 텐서 추출 실패 (index {i})"))?;
            let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
            let array = ArrayD::from_shape_vec(IxDyn(&dims), data.to_vec())
                .with_context(|| format!("출력 텐서 shape 불일치 (index {i})"))?;
            result.push(array);
        }
        Ok(result)
    }
}

/// 모델 세션 캐시 — 호출마다 수백 MB 모델을 다시 읽지 않는다.
/// 경로가 바뀌면(설정 변경) 다시 로드한다.
static MODEL_CACHE: OnceLock<Mutex<Option<(PathBuf, OrtModel)>>> = OnceLock::new();

fn with_model<T>(path: &Path, f: impl FnOnce(&mut OrtModel) -> Result<T>) -> Result<T> {
    let cache = MODEL_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap();
    let needs_load = match guard.as_ref() {
        Some((cached, _)) => cached != path,
        None => true,
    };
    if needs_load {
        *guard = Some((path.to_path_buf(), OrtModel::load(path)?));
    }
    f(&mut guard.as_mut().unwrap().1)
}

pub struct RemoveBackground;

impl Tool for RemoveBackground {
    fn name(&self) -> &'static str {
        "remove_background"
    }
    fn description(&self) -> &'static str {
        // '배경제거(누끼)' 복합명사를 명시해야 2B 모델이 도구를 매칭한다 (2026-06-11 replay 검증)
        "배경제거(누끼따기) 전용 도구. 이미지의 배경을 제거해 투명 배경 PNG 로 저장한다. \
         output_path 생략 시 워크스페이스에 _nobg.png 로 저장(첨부 이미지처럼 입력이 워크스페이스 \
         밖이어도 결과는 워크스페이스로 저장됨)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "원본 이미지 절대경로" },
                "output_path": { "type": "string", "description": "저장할 .png 절대경로 (생략 가능)" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let input = req_str(args, "path")?;
        // 존재 확인을 먼저 — 없는 파일이면 위치 힌트/질문 지시로 회복 경로를 준다
        if !Path::new(input).exists() {
            bail!(crate::tools::not_found_msg(input, &ctx.workspace()));
        }
        let ext = Path::new(input)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !SUPPORTED_INPUT_EXTENSIONS.contains(&ext.as_str()) {
            bail!("지원하지 않는 입력 형식 .{ext} (jpg, jpeg, png, webp, bmp, tiff 지원)");
        }

        let output = match opt_str(args, "output_path") {
            // 이름만 온 출력 경로는 워크스페이스로 흡수 (2026-06-12 R7 패턴)
            Some(o) => crate::tools::workspace::absorb_into_workspace(o, &ctx.workspace()),
            None => {
                let p = Path::new(input);
                let stem = p.file_stem().context("잘못된 경로")?.to_string_lossy();
                // 입력이 워크스페이스 밖(캐시 캡처 등)이면 산출물을 워크스페이스로 떨군다.
                crate::tools::workspace::default_output_in_workspace(
                    p,
                    &format!("{stem}_nobg.png"),
                    &ctx.workspace(),
                )
            }
        };
        let out_ext = output
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if out_ext != "png" {
            bail!("출력은 .png 만 가능 (투명 배경 유지): {}", output.display());
        }
        ensure_in_workspace(&output.to_string_lossy(), &ctx.workspace())?;

        // content-sniffing — 확장자와 내용이 다른 파일도 연다 (alian 2026-06-10 교훈)
        let img = open_image_sniffed(Path::new(input))?;

        let model_path = PathBuf::from(&ctx.config.lock().unwrap().removebg_model);
        if !model_path.exists() {
            bail!(
                "배경제거 모델 없음: {} (설정에서 경로 확인)",
                model_path.display()
            );
        }

        let input_tensor = preprocess(&img, MODEL_INPUT_SIZE, MODEL_INPUT_SIZE);
        let outputs = with_model(&model_path, |m| m.run(vec![input_tensor]))?;
        let mask = outputs
            .first()
            .ok_or_else(|| anyhow::anyhow!("모델이 출력을 돌려주지 않음"))?;

        let result = apply_mask(&img, mask);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        result
            .save(&output)
            .with_context(|| format!("저장 실패: {}", output.display()))?;
        Ok(format!(
            "배경 제거 완료 -> {} ({}x{} px, 투명 PNG)",
            output.display(),
            result.width(),
            result.height()
        ))
    }
}

/// 확장자가 아닌 내용으로 포맷을 식별해 연다 (content-sniffing).
/// 디코드 실패 = 이미지가 아닌 파일 — 막다른 에러 대신 "사용자에게 알리라"는 지시를
/// 담아 배회를 끊는다 (2026-06-12 R5 실측: txt 회전 요청에 디코드 실패만 반복).
pub fn open_image_sniffed(path: &Path) -> Result<DynamicImage> {
    let img = image::ImageReader::open(path)
        .with_context(|| format!("이미지 열기 실패: {}", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("이미지 형식 식별 실패: {}", path.display()))?
        .decode()
        .map_err(|e| {
            anyhow::anyhow!(
                "이 파일은 이미지가 아니라서 이미지 작업(회전/리사이즈/변환)을 할 수 없습니다: {} ({e}). \
                 이 사실을 그대로 사용자에게 알리세요.",
                path.display()
            )
        })?;
    Ok(img)
}

fn preprocess(img: &DynamicImage, model_h: u32, model_w: u32) -> ArrayD<f32> {
    let resized = img.resize_exact(model_w, model_h, image::imageops::FilterType::Lanczos3);
    let rgb = resized.to_rgb8();
    let mut tensor = Array4::<f32>::zeros((1, 3, model_h as usize, model_w as usize));
    for (x, y, pixel) in rgb.enumerate_pixels() {
        tensor[[0, 0, y as usize, x as usize]] = pixel[0] as f32 / 255.0;
        tensor[[0, 1, y as usize, x as usize]] = pixel[1] as f32 / 255.0;
        tensor[[0, 2, y as usize, x as usize]] = pixel[2] as f32 / 255.0;
    }
    tensor.into_dyn()
}

fn apply_mask(img: &DynamicImage, mask: &ArrayD<f32>) -> RgbaImage {
    let (orig_w, orig_h) = img.dimensions();
    let rgba = img.to_rgba8();

    let mask_shape = mask.shape();
    let (mask_h, mask_w) = if mask_shape.len() == 4 {
        (mask_shape[2], mask_shape[3])
    } else {
        (mask_shape[1], mask_shape[2])
    };

    let mask_data: Vec<u8> = mask
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    let mask_img = image::GrayImage::from_raw(mask_w as u32, mask_h as u32, mask_data)
        .expect("마스크 크기 불일치");
    let mask_resized = image::imageops::resize(
        &mask_img,
        orig_w,
        orig_h,
        image::imageops::FilterType::Lanczos3,
    );

    let mut result = RgbaImage::new(orig_w, orig_h);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = mask_resized.get_pixel(x, y)[0];
        result.put_pixel(x, y, image::Rgba([pixel[0], pixel[1], pixel[2], alpha]));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::ctx_with_workspace;
    use tempfile::tempdir;

    fn model_available() -> bool {
        let p = PathBuf::from(&crate::config::AppConfig::default().removebg_model);
        if p.exists() {
            true
        } else {
            eprintln!("skip: 모델 없음 {}", p.display());
            false
        }
    }

    #[test]
    fn rejects_non_png_output() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let input = dir.path().join("in.png");
        image::DynamicImage::new_rgb8(8, 8).save(&input).unwrap();
        let err = RemoveBackground
            .execute(
                &json!({"path": input.to_string_lossy(), "output_path": dir.path().join("out.jpg").to_string_lossy()}),
                &ctx,
            )
            .unwrap_err();
        assert!(err.to_string().contains(".png"), "{err}");
    }

    #[test]
    fn rejects_unsupported_input_extension() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let input = dir.path().join("in.gif");
        std::fs::write(&input, b"fake").unwrap();
        let err = RemoveBackground
            .execute(&json!({"path": input.to_string_lossy()}), &ctx)
            .unwrap_err();
        assert!(err.to_string().contains("지원하지 않는 입력"), "{err}");
    }

    /// 캐시 등 워크스페이스 밖 입력 + output 생략 → 산출물이 워크스페이스로 라우팅되어
    /// '워크스페이스 밖' 거부가 더 이상 발생하지 않는다 (2026-06-13: 캡처 캐시 배경제거 실패 회귀 방지).
    #[test]
    fn outside_input_defaults_into_workspace_not_rejected() {
        if model_available() {
            return; // 모델이 있으면 실제 저장까지 진행 — 여기선 경로 거부 여부만 검증
        }
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let ctx = ctx_with_workspace(&ws);
        let input = dir.path().join("in.png"); // 워크스페이스 밖(캐시 흉내)
        image::DynamicImage::new_rgb8(8, 8).save(&input).unwrap();
        let err = RemoveBackground
            .execute(&json!({"path": input.to_string_lossy()}), &ctx)
            .unwrap_err();
        assert!(
            !err.to_string().contains("워크스페이스 밖"),
            "캐시 입력의 기본 산출물이 워크스페이스 밖으로 거부됨: {err}"
        );
    }

    /// 실제 모델 E2E — removeBG.ort 가 있을 때만 (alian 방식)
    #[test]
    fn end_to_end_produces_rgba_png_at_original_size() {
        if !model_available() {
            return;
        }
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let input = dir.path().join("in.png");
        let output = dir.path().join("out.png");
        let img: image::RgbImage = image::ImageBuffer::from_fn(96, 72, |x, y| {
            if (24..72).contains(&x) && (18..54).contains(&y) {
                image::Rgb([220, 40, 40])
            } else {
                image::Rgb([245, 245, 245])
            }
        });
        img.save(&input).unwrap();

        let out = RemoveBackground
            .execute(
                &json!({"path": input.to_string_lossy(), "output_path": output.to_string_lossy()}),
                &ctx,
            )
            .unwrap();
        assert!(out.contains("배경 제거 완료"), "{out}");
        let result = image::open(&output).unwrap();
        assert_eq!((result.width(), result.height()), (96, 72));
        assert!(matches!(result, image::DynamicImage::ImageRgba8(_)));
    }

    /// 실제 시나리오 E2E — 캐시(워크스페이스 밖) 캡처 입력 + output 생략 →
    /// 산출물이 워크스페이스 안에 _nobg.png 로 생성된다 (2026-06-13 사용자 보고 수정 검증).
    #[test]
    fn end_to_end_cache_input_saves_into_workspace() {
        if !model_available() {
            return;
        }
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("cache"); // 워크스페이스 밖(앱 캐시 흉내)
        std::fs::create_dir(&ws).unwrap();
        std::fs::create_dir(&cache).unwrap();
        let ctx = ctx_with_workspace(&ws);
        let input = cache.join("capture_x.png");
        let img: image::RgbImage = image::ImageBuffer::from_fn(64, 48, |x, _| {
            if x > 20 {
                image::Rgb([200, 30, 30])
            } else {
                image::Rgb([250, 250, 250])
            }
        });
        img.save(&input).unwrap();

        let out = RemoveBackground
            .execute(&json!({"path": input.to_string_lossy()}), &ctx)
            .unwrap();
        // 산출물이 워크스페이스 안 _nobg.png
        let expected = ws.join("capture_x_nobg.png");
        assert!(expected.exists(), "워크스페이스에 산출물 없음. 결과: {out}");
        assert!(
            out.contains(&expected.to_string_lossy().to_string()),
            "{out}"
        );
    }
}
