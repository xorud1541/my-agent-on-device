//! 워크스페이스를 1-depth 스캔해 타입별 개수 + 결정적 맞춤 제안을 만든다.
//! 제안 생성 로직을 여기(백엔드)에 둬 단위 테스트로 검증한다(프론트 테스트 러너 없음).

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize, PartialEq)]
pub struct WorkspaceSummary {
    pub workspace_dir: String,
    pub folder_name: String,
    pub is_default_home: bool,
    /// 에이전트가 다룰 수 있는 파일(이미지/PDF/zip)이 0인가.
    /// 다른 타입(.docx, .mp4 등)만 있는 폴더도 의도적으로 true — 처리할 도구가 없어 폴더 선택으로 안내한다.
    pub is_empty: bool,
    pub images: u32,
    pub pdfs: u32,
    pub zips: u32,
    pub others: u32,
    pub removebg_available: bool,
    /// 폴더의 대표(알파벳 최소) 이미지/PDF 파일명 — 제안에 실제 파일명을 쓰기 위해.
    pub sample_image: Option<String>,
    pub sample_pdf: Option<String>,
    /// 폴더가 지정됐고 다룰 파일이 있을 때만 채운다. 홈 폴더이거나 다룰 파일이 없으면 빈 목록.
    pub suggestions: Vec<String>,
}

/// 확장자로 파일을 분류한 개수. 하위 폴더는 세지 않는다(1-depth, 파일만).
#[derive(Default, PartialEq, Debug)]
struct Counts {
    images: u32,
    pdfs: u32,
    zips: u32,
    others: u32,
    sample_image: Option<String>,
    sample_pdf: Option<String>,
}

fn classify(dir: &Path) -> Counts {
    let mut c = Counts::default();
    let mut image_names: Vec<String> = Vec::new();
    let mut pdf_names: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return c };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" => {
                c.images += 1;
                image_names.push(name);
            }
            "pdf" => {
                c.pdfs += 1;
                pdf_names.push(name);
            }
            "zip" => c.zips += 1,
            _ => c.others += 1,
        }
    }
    c.sample_image = image_names.iter().min().cloned();
    c.sample_pdf = pdf_names.iter().min().cloned();
    c
}

/// 보유 타입에 매핑된 제안만 결정적으로 만든다. 실제 파일명을 써서 구체적으로.
/// 배경제거는 모델(.ort)이 있을 때만(막다른 길 방지). 화면 캡처는 목록에 넣지 않는다
/// (입력창 옆 캡처 버튼으로 안내 — 프론트 말풍선이 설명).
fn build_suggestions(c: &Counts, removebg_available: bool) -> Vec<String> {
    let mut s = Vec::new();
    if let Some(img) = &c.sample_image {
        if removebg_available {
            s.push(format!("{img} 배경 제거"));
        }
        s.push("사진들 PDF로 합치기".to_string());
    }
    if let Some(pdf) = &c.sample_pdf {
        s.push(format!("{pdf} 텍스트 추출"));
    }
    if c.zips >= 1 {
        s.push("압축 풀기".to_string());
    }
    s
}

/// 순수 함수: 경로들을 받아 요약을 만든다(파일시스템 접근만, Tauri 비의존 → 테스트 가능).
pub fn summarize(workspace_dir: &Path, home_dir: &Path, removebg_model: &Path) -> WorkspaceSummary {
    let c = classify(workspace_dir);
    let is_default_home = workspace_dir == home_dir;
    let is_empty = c.images + c.pdfs + c.zips == 0;
    let removebg_available = removebg_model.exists();
    let folder_name = workspace_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| workspace_dir.to_string_lossy().into_owned());
    // 상태 ① 에서만 제안을 만든다. 홈/빈 폴더는 프론트가 폴더 선택 UI 를 보여준다.
    let suggestions = if is_default_home || is_empty {
        Vec::new()
    } else {
        build_suggestions(&c, removebg_available)
    };
    WorkspaceSummary {
        workspace_dir: workspace_dir.to_string_lossy().into_owned(),
        folder_name,
        is_default_home,
        is_empty,
        images: c.images,
        pdfs: c.pdfs,
        zips: c.zips,
        others: c.others,
        removebg_available,
        sample_image: c.sample_image.clone(),
        sample_pdf: c.sample_pdf.clone(),
        suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    fn touch(dir: &Path, name: &str) {
        File::create(dir.join(name)).unwrap();
    }

    #[test]
    fn images_with_model_suggest_bg_and_merge_using_first_filename() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        let model = tempdir().unwrap();
        let model_path = model.path().join("removeBG.ort");
        File::create(&model_path).unwrap();
        touch(ws.path(), "b.jpg");
        touch(ws.path(), "a.png");
        let s = summarize(ws.path(), home.path(), &model_path);
        assert_eq!(s.images, 2);
        assert_eq!(s.sample_image.as_deref(), Some("a.png")); // 알파벳 최소
        assert!(!s.is_empty);
        assert!(s.removebg_available);
        assert_eq!(
            s.suggestions,
            vec![
                "a.png 배경 제거".to_string(),
                "사진들 PDF로 합치기".to_string(),
            ]
        );
    }

    #[test]
    fn pdf_only_suggests_extract_with_filename() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        touch(ws.path(), "report.pdf");
        let s = summarize(ws.path(), home.path(), Path::new("/none.ort"));
        assert_eq!(s.sample_pdf.as_deref(), Some("report.pdf"));
        assert_eq!(s.suggestions, vec!["report.pdf 텍스트 추출".to_string()]);
    }

    #[test]
    fn no_removebg_model_skips_bg_keeps_merge() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        touch(ws.path(), "a.png");
        let s = summarize(ws.path(), home.path(), Path::new("/does/not/exist.ort"));
        assert!(!s.removebg_available);
        assert!(!s.suggestions.iter().any(|x| x.contains("배경 제거")));
        assert!(s.suggestions.iter().any(|x| x == "사진들 PDF로 합치기"));
    }

    #[test]
    fn zip_only_suggests_extract() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        touch(ws.path(), "x.zip");
        let s = summarize(ws.path(), home.path(), Path::new("/none.ort"));
        assert_eq!(s.suggestions, vec!["압축 풀기".to_string()]);
    }

    #[test]
    fn capture_is_not_a_typeable_suggestion() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        touch(ws.path(), "a.png");
        let s = summarize(ws.path(), home.path(), Path::new("/none.ort"));
        assert!(!s.suggestions.iter().any(|x| x.contains("캡처")));
    }

    #[test]
    fn empty_dir_is_empty_and_no_suggestions() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        let s = summarize(ws.path(), home.path(), Path::new("/none.ort"));
        assert!(s.is_empty);
        assert!(s.suggestions.is_empty());
    }

    #[test]
    fn default_home_detected_and_no_suggestions() {
        let home = tempdir().unwrap();
        touch(home.path(), "a.png");
        let s = summarize(home.path(), home.path(), Path::new("/none.ort"));
        assert!(s.is_default_home);
        assert!(s.suggestions.is_empty());
    }

    #[test]
    fn folder_name_is_last_segment() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        let s = summarize(ws.path(), home.path(), Path::new("/none.ort"));
        assert_eq!(s.folder_name, ws.path().file_name().unwrap().to_string_lossy());
    }
}
