use super::{opt_str, req_str, Tool};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

pub struct ZipCreate;

impl Tool for ZipCreate {
    fn name(&self) -> &'static str {
        "zip_create"
    }
    fn description(&self) -> &'static str {
        "파일 또는 폴더(하위 포함)를 ZIP으로 압축한다. paths 는 쉼표로 여러 개 지정 가능."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "paths": { "type": "string", "description": "압축할 파일/폴더 절대경로 (여러 개는 쉼표 구분)" },
                "output_path": { "type": "string", "description": "생성할 zip 절대경로 (생략하면 첫 대상 옆에 .zip)" }
            },
            "required": ["paths"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let paths: Vec<PathBuf> = req_str(args, "paths")?
            .split(',')
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if paths.is_empty() {
            bail!("압축할 대상이 없음");
        }
        for p in &paths {
            if !p.exists() {
                bail!("경로 없음: {}", p.display());
            }
        }

        let out_path = match opt_str(args, "output_path") {
            Some(o) => PathBuf::from(o),
            None => {
                let first = &paths[0];
                let stem = first
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "archive".into());
                first.with_file_name(format!("{stem}.zip"))
            }
        };
        if out_path.exists() {
            bail!(
                "대상이 이미 존재함: {} (다른 output_path 지정)",
                out_path.display()
            );
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut zip = zip::ZipWriter::new(File::create(&out_path)?);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut count = 0usize;

        for path in &paths {
            if path.is_file() {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                add_file(&mut zip, path, &name, opts)?;
                count += 1;
            } else {
                // 폴더: 폴더명을 루트로 상대경로 유지
                let base = path.parent().unwrap_or(Path::new(""));
                for entry in walkdir::WalkDir::new(path)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let rel = entry
                        .path()
                        .strip_prefix(base)
                        .unwrap_or(entry.path())
                        .to_string_lossy()
                        .replace('\\', "/");
                    add_file(&mut zip, entry.path(), &rel, opts)?;
                    count += 1;
                }
            }
        }
        zip.finish()?;
        let size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
        Ok(format!(
            "압축 완료: {} (파일 {count}개, {size} bytes)",
            out_path.display()
        ))
    }
}

fn add_file(
    zip: &mut zip::ZipWriter<File>,
    src: &Path,
    name_in_zip: &str,
    opts: SimpleFileOptions,
) -> Result<()> {
    zip.start_file(name_in_zip, opts)
        .with_context(|| format!("zip 엔트리 생성 실패: {name_in_zip}"))?;
    let mut f = File::open(src).with_context(|| format!("파일 열기 실패: {}", src.display()))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    zip.write_all(&buf)?;
    Ok(())
}

pub struct ZipExtract;

impl Tool for ZipExtract {
    fn name(&self) -> &'static str {
        "zip_extract"
    }
    fn description(&self) -> &'static str {
        "ZIP 파일의 압축을 푼다. 내용 확인만 하려면 list_only=true."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "zip 파일 절대경로" },
                "output_dir": { "type": "string", "description": "풀 디렉토리 (생략하면 zip 이름의 폴더)" },
                "list_only": { "type": "boolean", "description": "압축을 풀지 않고 목록만 조회" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let path = req_str(args, "path")?;
        let file = File::open(path).with_context(|| format!("파일 없음: {path}"))?;
        let mut archive = zip::ZipArchive::new(file).context("zip 형식 아님")?;

        if args
            .get("list_only")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let mut lines = Vec::new();
            for i in 0..archive.len().min(200) {
                let entry = archive.by_index(i)?;
                lines.push(format!(
                    "{}\t{}",
                    decode_name(entry.name_raw()),
                    entry.size()
                ));
            }
            return Ok(format!(
                "{}개 항목:\nname\tsize\n{}",
                archive.len(),
                lines.join("\n")
            ));
        }

        let out_dir = match opt_str(args, "output_dir") {
            Some(o) => PathBuf::from(o),
            None => {
                let p = Path::new(path);
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "extracted".into());
                p.with_file_name(stem)
            }
        };
        std::fs::create_dir_all(&out_dir)?;

        let mut count = 0usize;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = decode_name(entry.name_raw());
            // Zip Slip 방지: 절대경로/상위 탈출 차단
            let rel = Path::new(&name);
            if rel.is_absolute()
                || rel
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                continue;
            }
            let target = out_dir.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&target)?;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&target)
                .with_context(|| format!("생성 실패: {}", target.display()))?;
            std::io::copy(&mut entry, &mut out)?;
            count += 1;
        }
        Ok(format!(
            "압축 해제 완료: {} (파일 {count}개)",
            out_dir.display()
        ))
    }
}

/// zip 엔트리 이름 디코딩 — UTF-8 이 아니면 한국어 레거시(CP949/EUC-KR)로 시도
fn decode_name(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (decoded, _, had_errors) = encoding_rs::EUC_KR.decode(raw);
            if had_errors {
                String::from_utf8_lossy(raw).into_owned()
            } else {
                decoded.into_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn zip_roundtrip_with_korean_names() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("보고서");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("요약.txt"), "내용입니다").unwrap();
        std::fs::create_dir(src.join("하위")).unwrap();
        std::fs::write(src.join("하위").join("자료.md"), "# 자료").unwrap();

        let out = ZipCreate
            .execute(&json!({"paths": src.to_string_lossy()}))
            .unwrap();
        assert!(out.contains("파일 2개"), "{out}");
        let zip_path = dir.path().join("보고서.zip");
        assert!(zip_path.exists());

        let listed = ZipExtract
            .execute(&json!({"path": zip_path.to_string_lossy(), "list_only": true}))
            .unwrap();
        assert!(listed.contains("요약.txt"), "{listed}");

        let extract_to = dir.path().join("풀기");
        ZipExtract
            .execute(&json!({"path": zip_path.to_string_lossy(), "output_dir": extract_to.to_string_lossy()}))
            .unwrap();
        let text = std::fs::read_to_string(extract_to.join("보고서").join("요약.txt")).unwrap();
        assert_eq!(text, "내용입니다");
        assert!(extract_to
            .join("보고서")
            .join("하위")
            .join("자료.md")
            .exists());
    }

    #[test]
    fn zip_multiple_files_via_comma() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let out_zip = dir.path().join("두개.zip");

        let out = ZipCreate
            .execute(&json!({
                "paths": format!("{}, {}", a.to_string_lossy(), b.to_string_lossy()),
                "output_path": out_zip.to_string_lossy()
            }))
            .unwrap();
        assert!(out.contains("파일 2개"), "{out}");
    }

    #[test]
    fn extract_blocks_zip_slip() {
        let dir = tempdir().unwrap();
        let zip_path = dir.path().join("evil.zip");
        {
            let mut zw = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = SimpleFileOptions::default();
            zw.start_file("../escape.txt", opts).unwrap();
            zw.write_all(b"bad").unwrap();
            zw.finish().unwrap();
        }
        let extract_to = dir.path().join("out");
        ZipExtract
            .execute(&json!({"path": zip_path.to_string_lossy(), "output_dir": extract_to.to_string_lossy()}))
            .unwrap();
        assert!(
            !dir.path().join("escape.txt").exists(),
            "zip slip 차단 실패"
        );
    }
}
