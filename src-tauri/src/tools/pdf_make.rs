//! 이미지들 → PDF 묶기. alian(alice-tools-pdf) 선작업 포팅.
//! JPEG 무손실 passthrough, EXIF 회전/알파/CMYK 보정, 일괄 검증 fail-fast, 원자적 저장.

use super::{opt_str, Tool, ToolCtx};
use crate::tools::workspace::ensure_in_workspace;
use anyhow::{bail, Result};
use image::{metadata::Orientation, ColorType, GenericImageView, ImageDecoder, ImageReader};
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use serde_json::{json, Value};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

// ---------- 페이지 기하 ----------

const A4: (f32, f32) = (595.0, 842.0); // pt (210x297mm)
const LETTER: (f32, f32) = (612.0, 792.0); // pt (8.5x11in)
const MARGIN_PT: f32 = 18.0; // ≈6mm

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageSize {
    A4,
    Letter,
    Fit,
}

impl PageSize {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "a4" => Some(Self::A4),
            "letter" => Some(Self::Letter),
            "fit" => Some(Self::Fit),
            _ => None,
        }
    }
}

/// 결과: (MediaBox 너비, 높이, cm 행렬). cm 은 단위 이미지를 스케일+평행이동.
fn place_on_page(img_w: u32, img_h: u32, page: PageSize) -> (f32, f32, [f32; 6]) {
    let (iw, ih) = (img_w as f32, img_h as f32);
    if page == PageSize::Fit {
        return (iw, ih, [iw, 0.0, 0.0, ih, 0.0, 0.0]);
    }
    let base = match page {
        PageSize::A4 => A4,
        PageSize::Letter => LETTER,
        PageSize::Fit => unreachable!(),
    };
    // 이미지 방향에 맞춰 페이지 가로/세로 자동 선택
    let (pw, ph) = if iw > ih {
        (base.1, base.0)
    } else {
        (base.0, base.1)
    };
    let avail_w = pw - 2.0 * MARGIN_PT;
    let avail_h = ph - 2.0 * MARGIN_PT;
    let s = (avail_w / iw).min(avail_h / ih);
    let (dw, dh) = (iw * s, ih * s);
    let (tx, ty) = ((pw - dw) / 2.0, (ph - dh) / 2.0);
    (pw, ph, [dw, 0.0, 0.0, dh, tx, ty])
}

// ---------- 입력 검증 (헤더만 읽음) ----------

struct ImageProbe {
    orientation: Orientation,
    has_alpha: bool,
    is_jpeg: bool,
    is_cmyk_jpeg: bool,
}

fn probe_image(path: &Path) -> Result<ImageProbe> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "bmp" | "webp" => {}
        other => bail!("지원하지 않는 형식: {other} (png, jpg, jpeg, bmp, webp 만 지원)"),
    }

    if ext == "webp" {
        let dec = image::codecs::webp::WebPDecoder::new(BufReader::new(File::open(path)?))?;
        if dec.has_animation() {
            bail!("animated webp 는 지원하지 않음: {}", path.display());
        }
    }

    let mut decoder = ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let _ = decoder.dimensions();
    let has_alpha = decoder.color_type().has_alpha();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let is_jpeg = matches!(ext.as_str(), "jpg" | "jpeg");
    let is_cmyk_jpeg = if is_jpeg {
        let bytes = std::fs::read(path)?;
        jpeg_component_count(&bytes) == Some(4)
    } else {
        false
    };
    Ok(ImageProbe {
        orientation,
        has_alpha,
        is_jpeg,
        is_cmyk_jpeg,
    })
}

/// JPEG SOF 마커의 컴포넌트 수(Nf). 1=Gray, 3=YCbCr/RGB, 4=CMYK/YCCK. 파싱 실패 시 None.
fn jpeg_component_count(bytes: &[u8]) -> Option<u8> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = ((bytes[i + 2] as usize) << 8) | bytes[i + 3] as usize;
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            // SOF payload: [precision(1)][height(2)][width(2)][Nf(1)]
            return bytes.get(i + 4 + 1 + 2 + 2).copied();
        }
        i += 2 + len;
    }
    None
}

// ---------- PDF 생성 ----------

fn image_to_pdf(inputs: &[&Path], output: &Path, page_size: PageSize) -> Result<()> {
    if inputs.is_empty() {
        bail!("입력 이미지가 없음");
    }

    // 1) 전체 입력 일괄 검증 — 문제 파일을 한 번에 모아 fail-fast
    let mut probes: Vec<ImageProbe> = Vec::with_capacity(inputs.len());
    let mut errors: Vec<String> = Vec::new();
    for input in inputs {
        match probe_image(input) {
            Ok(p) => probes.push(p),
            Err(e) => errors.push(format!("{}: {e}", input.display())),
        }
    }
    if !errors.is_empty() {
        bail!(
            "입력 검증 실패 ({}건):\n{}",
            errors.len(),
            errors.join("\n")
        );
    }

    // 2) 페이지 생성
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut page_ids: Vec<ObjectId> = Vec::new();
    for (input, probe) in inputs.iter().zip(probes.iter()) {
        page_ids.push(add_image_page(&mut doc, pages_id, input, probe, page_size)?);
    }

    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => page_ids.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>(),
        "Count" => page_ids.len() as i64,
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.compress();

    // 3) 원자적 출력: temp → rename
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = output.with_extension("pdf.tmp");
    doc.save(&tmp)?;
    std::fs::rename(&tmp, output)?;
    Ok(())
}

fn add_image_page(
    doc: &mut Document,
    pages_id: ObjectId,
    path: &Path,
    probe: &ImageProbe,
    page_size: PageSize,
) -> Result<ObjectId> {
    // EXIF 회전/알파/CMYK JPEG 은 디코드 경로(보정 후 raw RGB), 아니면 무손실 passthrough
    let needs_decode =
        probe.orientation != Orientation::NoTransforms || probe.has_alpha || probe.is_cmyk_jpeg;
    let (image_stream, img_w, img_h) = if needs_decode {
        encode_corrected(path, probe)?
    } else if probe.is_jpeg {
        encode_jpeg_passthrough(path)?
    } else {
        encode_raw(path)?
    };

    let image_id = doc.add_object(image_stream);
    let resources_id = doc.add_object(dictionary! {
        "XObject" => dictionary! { "Im1" => Object::Reference(image_id) },
    });

    let (page_w, page_h, cm) = place_on_page(img_w, img_h, page_size);
    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    cm[0].into(),
                    cm[1].into(),
                    cm[2].into(),
                    cm[3].into(),
                    cm[4].into(),
                    cm[5].into(),
                ],
            ),
            Operation::new("Do", vec!["Im1".into()]),
            Operation::new("Q", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode()?));
    let media_box = vec![
        0.into(),
        0.into(),
        (page_w as i64).into(),
        (page_h as i64).into(),
    ];

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => Object::Reference(resources_id),
        "MediaBox" => media_box,
    });
    Ok(page_id)
}

/// EXIF 회전/알파 보정: 디코드 → apply_orientation → 흰배경 합성 → raw RGB
fn encode_corrected(path: &Path, probe: &ImageProbe) -> Result<(Stream, u32, u32)> {
    let decoder = ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(probe.orientation);
    let (w, h) = img.dimensions();
    let rgb = if probe.has_alpha {
        let rgba = img.to_rgba8();
        let mut out = image::RgbImage::new(w, h);
        for (x, y, p) in rgba.enumerate_pixels() {
            let a = p[3] as f32 / 255.0;
            let blend = |c: u8| ((c as f32) * a + 255.0 * (1.0 - a)).round() as u8;
            out.put_pixel(x, y, image::Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
        }
        out
    } else {
        img.to_rgb8()
    };
    let stream = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image",
            "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
        },
        rgb.into_raw(),
    );
    Ok((stream, w, h))
}

/// JPEG 무손실 passthrough — 원본 DCT 스트림 그대로 임베드
fn encode_jpeg_passthrough(path: &Path) -> Result<(Stream, u32, u32)> {
    let bytes = std::fs::read(path)?;
    let img = image::open(path)?;
    let (w, h) = img.dimensions();
    let color_space = match img.color() {
        ColorType::L8 | ColorType::L16 => "DeviceGray",
        _ => "DeviceRGB",
    };
    let mut stream = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image",
            "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => color_space, "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        bytes,
    );
    stream.allows_compression = false;
    Ok((stream, w, h))
}

/// png/bmp(알파·회전 없음) → raw RGB (doc.compress 가 Flate)
fn encode_raw(path: &Path) -> Result<(Stream, u32, u32)> {
    let img = image::open(path)?;
    let (w, h) = img.dimensions();
    let rgb = img.to_rgb8();
    let stream = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image",
            "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
        },
        rgb.into_raw(),
    );
    Ok((stream, w, h))
}

// ---------- 도구 ----------

/// probe_image 가 받아주는 확장자와 동일하게 유지할 것
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp"];

/// output_path 생략 시 워크스페이스의 비충돌 기본 이름 (images.pdf, images_2.pdf, ...)
fn default_output_path(ws: &Path) -> PathBuf {
    let base = ws.join("images.pdf");
    if !base.exists() {
        return base;
    }
    (2u32..)
        .map(|i| ws.join(format!("images_{i}.pdf")))
        .find(|p| !p.exists())
        .expect("비충돌 이름은 반드시 존재")
}

/// 폴더(비재귀)의 이미지 파일을 이름순으로 모은다. 작은 모델이 경로 배열을
/// 직접 옮겨 적다 생기는 인자 오염을 피하기 위한 dir 인자 경로.
fn collect_images_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("폴더 열기 실패: {}: {e}", dir.display()))?;
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|s| s.to_str())
                    .map(|s| IMAGE_EXTS.contains(&s.to_ascii_lowercase().as_str()))
                    .unwrap_or(false)
        })
        .collect();
    out.sort();
    Ok(out)
}

pub struct ImagesToPdf;

impl Tool for ImagesToPdf {
    fn name(&self) -> &'static str {
        "images_to_pdf"
    }
    fn description(&self) -> &'static str {
        "여러 이미지를 한 장씩 페이지로 묶어 PDF 파일을 만든다. \
         폴더의 이미지 전체를 묶을 땐 paths 대신 dir 에 폴더 경로 하나만 넘긴다. \
         이미지 순서 = 페이지 순서 (dir 은 이름순)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "dir": {
                    "type": "string",
                    "description": "이미지 폴더 절대경로 — 폴더 안 모든 이미지를 이름순으로 묶는다 (paths 대신 사용)"
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "이미지 절대경로 목록 (페이지 순서대로). dir 을 줬으면 생략"
                },
                "output_path": { "type": "string", "description": "생성할 .pdf 경로. 사용자가 파일 이름을 말했으면 그 이름을 그대로 쓴다. 생략하면 워크스페이스에 자동 이름으로 저장" },
                "page_size": { "type": "string", "enum": ["a4", "letter", "fit"], "description": "페이지 크기 (기본 a4, fit=이미지 크기 그대로)" }
            },
            "required": []
        })
    }
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let mut paths: Vec<PathBuf> = args
            .get("paths")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        if paths.is_empty() {
            if let Some(dir) = opt_str(args, "dir") {
                paths = collect_images_sorted(Path::new(dir))?;
                if paths.is_empty() {
                    bail!("폴더에 이미지가 없음: {dir}");
                }
            }
        }
        if paths.is_empty() {
            bail!("필수 인자 누락: dir(이미지 폴더) 또는 paths(이미지 경로 배열)");
        }
        let output = match opt_str(args, "output_path") {
            Some(s) => {
                // 이름만 온 출력 경로("album.pdf")는 워크스페이스로 흡수 (2026-06-12 R7)
                let mut p = crate::tools::workspace::absorb_into_workspace(s, &ctx.workspace());
                // 작은 모델이 폴더 모양 경로(확장자 없음)를 주는 실수를 흡수한다
                // (2026-06-11: ".alice\pdfs" 거부 → 모델이 경로 수정 못 하고 라운드 소진)
                if p.extension().is_none() {
                    p.set_extension("pdf");
                }
                if p.extension()
                    .map(|e| !e.eq_ignore_ascii_case("pdf"))
                    .unwrap_or(true)
                {
                    bail!(
                        "출력은 .pdf 만 가능: {} (예: 워크스페이스 안의 문서모음.pdf)",
                        p.display()
                    );
                }
                p
            }
            // 생략 시 워크스페이스에 비충돌 이름으로 자동 저장 — 출력 경로 실수 원천 차단
            None => default_output_path(&ctx.workspace()),
        };
        if output.exists() {
            bail!(
                "대상이 이미 존재함: {} (다른 output_path 지정)",
                output.display()
            );
        }
        ensure_in_workspace(&output.to_string_lossy(), &ctx.workspace())?;
        let page_size = match opt_str(args, "page_size") {
            Some(s) => PageSize::parse(s)
                .ok_or_else(|| anyhow::anyhow!("page_size 는 a4|letter|fit 중 하나: {s}"))?,
            None => PageSize::A4,
        };

        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        image_to_pdf(&refs, &output, page_size)?;
        let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
        Ok(format!(
            "PDF 생성 완료: {} ({}페이지, {size} bytes)",
            output.display(),
            paths.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::ctx_with_workspace;
    use image::{ImageBuffer, Rgb};
    use tempfile::tempdir;

    fn write_png(path: &Path, w: u32, h: u32) {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(w, h, |_, _| Rgb([120, 180, 240]));
        img.save(path).unwrap();
    }

    #[test]
    fn multiple_images_produce_multiple_pages() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.jpg");
        write_png(&a, 32, 32);
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(40, 30, |_, _| Rgb([200, 100, 50]));
        img.save(&b).unwrap();
        let out = dir.path().join("out.pdf");

        let msg = ImagesToPdf
            .execute(
                &json!({
                    "paths": [a.to_string_lossy(), b.to_string_lossy()],
                    "output_path": out.to_string_lossy()
                }),
                &ctx,
            )
            .unwrap();
        assert!(msg.contains("2페이지"), "{msg}");
        let doc = Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
    }

    /// dir 인자만으로 폴더의 이미지 전체가 묶여야 한다 (이미지 아닌 파일은 제외)
    #[test]
    fn dir_argument_bundles_all_images() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        write_png(&dir.path().join("b.png"), 16, 16);
        write_png(&dir.path().join("a.png"), 16, 16);
        std::fs::write(dir.path().join("note.txt"), "not an image").unwrap();
        let out = dir.path().join("out.pdf");

        let msg = ImagesToPdf
            .execute(
                &json!({
                    "dir": dir.path().to_string_lossy(),
                    "output_path": out.to_string_lossy()
                }),
                &ctx,
            )
            .unwrap();
        assert!(msg.contains("2페이지"), "{msg}");
        assert_eq!(Document::load(&out).unwrap().get_pages().len(), 2);
    }

    /// output_path 생략 → 워크스페이스에 자동 이름, 재호출 시 비충돌 이름
    #[test]
    fn omitted_output_path_defaults_into_workspace() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        write_png(&dir.path().join("a.png"), 8, 8);

        let msg = ImagesToPdf
            .execute(&json!({"dir": dir.path().to_string_lossy()}), &ctx)
            .unwrap();
        assert!(msg.contains("images.pdf"), "{msg}");
        assert!(dir.path().join("images.pdf").exists());

        // 두 번째 호출은 기존 파일을 덮지 않고 다음 이름을 쓴다
        // (생성된 images.pdf 가 dir 스캔에 섞이지 않는 것도 함께 검증 — pdf 는 이미지 아님)
        let msg2 = ImagesToPdf
            .execute(&json!({"dir": dir.path().to_string_lossy()}), &ctx)
            .unwrap();
        assert!(msg2.contains("images_2.pdf"), "{msg2}");
        assert!(dir.path().join("images_2.pdf").exists());
    }

    /// 확장자 없는 출력 경로(폴더 모양 실수)는 .pdf 로 보정된다
    #[test]
    fn extensionless_output_path_gets_pdf_appended() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        write_png(&dir.path().join("a.png"), 8, 8);
        let out_no_ext = dir.path().join("묶음");

        let msg = ImagesToPdf
            .execute(
                &json!({
                    "dir": dir.path().to_string_lossy(),
                    "output_path": out_no_ext.to_string_lossy()
                }),
                &ctx,
            )
            .unwrap();
        assert!(msg.contains("묶음.pdf"), "{msg}");
        assert!(dir.path().join("묶음.pdf").exists());
    }

    #[test]
    fn collect_images_is_name_sorted_and_filtered() {
        let dir = tempdir().unwrap();
        write_png(&dir.path().join("b.png"), 8, 8);
        write_png(&dir.path().join("a.png"), 8, 8);
        std::fs::write(dir.path().join("c.gif"), b"x").unwrap(); // 미지원 확장자 제외
        let found = collect_images_sorted(dir.path()).unwrap();
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.png", "b.png"]);
    }

    #[test]
    fn missing_dir_and_paths_mentions_both_options() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let err = ImagesToPdf
            .execute(
                &json!({"output_path": dir.path().join("o.pdf").to_string_lossy()}),
                &ctx,
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("dir") && msg.contains("paths"), "{msg}");
    }

    #[test]
    fn empty_dir_reports_no_images() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let err = ImagesToPdf
            .execute(
                &json!({
                    "dir": dir.path().to_string_lossy(),
                    "output_path": dir.path().join("o.pdf").to_string_lossy()
                }),
                &ctx,
            )
            .unwrap_err();
        assert!(err.to_string().contains("이미지가 없음"), "{err}");
    }

    #[test]
    fn fit_page_size_keeps_pixel_dims() {
        let (w, h, cm) = place_on_page(640, 480, PageSize::Fit);
        assert_eq!((w, h), (640.0, 480.0));
        assert_eq!(cm, [640.0, 0.0, 0.0, 480.0, 0.0, 0.0]);
    }

    #[test]
    fn a4_landscape_for_wide_image() {
        let (w, h, _) = place_on_page(2000, 1000, PageSize::A4);
        assert_eq!((w, h), (842.0, 595.0));
    }

    #[test]
    fn mixed_batch_reports_all_unsupported_at_once() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let good = dir.path().join("a.png");
        let bad1 = dir.path().join("b.gif");
        let bad2 = dir.path().join("c.heic");
        write_png(&good, 8, 8);
        std::fs::write(&bad1, b"x").unwrap();
        std::fs::write(&bad2, b"x").unwrap();
        let out = dir.path().join("o.pdf");

        let err = ImagesToPdf
            .execute(
                &json!({
                    "paths": [good.to_string_lossy(), bad1.to_string_lossy(), bad2.to_string_lossy()],
                    "output_path": out.to_string_lossy()
                }),
                &ctx,
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("b.gif") && msg.contains("c.heic"), "{msg}");
        assert!(!out.exists(), "검증 실패 시 출력 파일을 만들지 않아야 함");
    }

    #[test]
    fn output_outside_workspace_rejected() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let ctx = ctx_with_workspace(&ws);
        let a = dir.path().join("a.png");
        write_png(&a, 8, 8);
        let out = dir.path().join("밖.pdf"); // 워크스페이스 밖
        let err = ImagesToPdf
            .execute(
                &json!({"paths": [a.to_string_lossy()], "output_path": out.to_string_lossy()}),
                &ctx,
            )
            .unwrap_err();
        assert!(err.to_string().contains("워크스페이스 밖"), "{err}");
    }

    #[test]
    fn rejects_non_pdf_output() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let a = dir.path().join("a.png");
        write_png(&a, 8, 8);
        let err = ImagesToPdf
            .execute(
                &json!({"paths": [a.to_string_lossy()], "output_path": dir.path().join("o.txt").to_string_lossy()}),
                &ctx,
            )
            .unwrap_err();
        assert!(err.to_string().contains(".pdf"), "{err}");
    }
}
