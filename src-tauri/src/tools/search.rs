use super::{opt_bool, opt_u64, req_str, Tool};
use anyhow::Result;
use globset::GlobBuilder;
use serde_json::{json, Value};
use walkdir::WalkDir;

const DEFAULT_LIMIT: u64 = 50;
const MAX_DEPTH: usize = 8;

const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "ico", "heic",
];

pub struct SearchFiles;

impl Tool for SearchFiles {
    fn name(&self) -> &'static str {
        "search_files"
    }
    fn description(&self) -> &'static str {
        "디렉토리(하위 포함)에서 이름 패턴으로 파일을 검색한다. 이미지만 찾으려면 images_only=true. \
         패턴은 글롭(*.png, 보고서*.docx) 또는 부분 문자열."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "검색 시작 디렉토리 절대경로" },
                "pattern": { "type": "string", "description": "파일명 글롭 패턴 또는 부분 문자열 (예: *.png, 보고서)" },
                "images_only": { "type": "boolean", "description": "이미지 파일만 검색 (기본 false)" },
                "recursive": { "type": "boolean", "description": "하위 폴더 포함 (기본 true)" },
                "limit": { "type": "integer", "description": "최대 결과 수 (기본 50)" }
            },
            "required": ["root", "pattern"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let root = req_str(args, "root")?;
        let pattern = req_str(args, "pattern")?;
        let images_only = opt_bool(args, "images_only").unwrap_or(false);
        let recursive = opt_bool(args, "recursive").unwrap_or(true);
        let limit = opt_u64(args, "limit").unwrap_or(DEFAULT_LIMIT) as usize;

        // 글롭 문자가 없으면 부분 문자열 매칭으로 *pattern* 처리
        let glob_pat = if pattern.contains(['*', '?', '[']) {
            pattern.to_string()
        } else {
            format!("*{pattern}*")
        };
        let matcher = GlobBuilder::new(&glob_pat)
            .case_insensitive(true)
            .build()?
            .compile_matcher();

        let max_depth = if recursive { MAX_DEPTH } else { 1 };
        let mut results = Vec::new();
        let mut scanned: u64 = 0;

        for entry in WalkDir::new(root)
            .max_depth(max_depth)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !is_hidden_or_system(e))
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            scanned += 1;
            if scanned > 200_000 {
                break;
            }
            let name = entry.file_name().to_string_lossy();
            if images_only {
                let ext_ok = entry
                    .path()
                    .extension()
                    .map(|e| IMAGE_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))
                    .unwrap_or(false);
                if !ext_ok {
                    continue;
                }
            }
            if !matcher.is_match(name.as_ref()) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(super::fs_tools::fmt_time)
                .unwrap_or_default();
            results.push(format!(
                "{}\t{}\t{}",
                entry.path().display(),
                size,
                modified
            ));
            if results.len() >= limit {
                break;
            }
        }

        if results.is_empty() {
            return Ok(format!(
                "'{pattern}' 에 해당하는 파일 없음 (검색 위치: {root})"
            ));
        }
        Ok(format!(
            "{}개 발견:\npath\tsize\tmodified\n{}",
            results.len(),
            results.join("\n")
        ))
    }
}

fn is_hidden_or_system(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.')
        || name.eq_ignore_ascii_case("node_modules")
        || name.eq_ignore_ascii_case("$RECYCLE.BIN")
        || name.eq_ignore_ascii_case("System Volume Information")
        || name.eq_ignore_ascii_case("AppData")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("photo.png"), "x").unwrap();
        std::fs::write(dir.path().join("report.docx"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("screenshot.PNG"), "x").unwrap();
        dir
    }

    #[test]
    fn glob_matches_recursively_case_insensitive() {
        let dir = setup();
        let out = SearchFiles
            .execute(&json!({"root": dir.path().to_string_lossy(), "pattern": "*.png"}))
            .unwrap();
        assert!(out.contains("photo.png"));
        assert!(out.contains("screenshot.PNG"));
        assert!(!out.contains("report.docx"));
    }

    #[test]
    fn substring_pattern_works() {
        let dir = setup();
        let out = SearchFiles
            .execute(&json!({"root": dir.path().to_string_lossy(), "pattern": "report"}))
            .unwrap();
        assert!(out.contains("report.docx"));
    }

    #[test]
    fn images_only_filters_non_images() {
        let dir = setup();
        let out = SearchFiles
            .execute(
                &json!({"root": dir.path().to_string_lossy(), "pattern": "*", "images_only": true}),
            )
            .unwrap();
        assert!(out.contains("photo.png"));
        assert!(!out.contains("report.docx"));
    }
}
