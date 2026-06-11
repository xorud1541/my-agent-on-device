use super::{opt_bool, req_str, Tool};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const MAX_READ_BYTES: u64 = 64 * 1024;
// 컨텍스트 예산 보호: 도구 결과가 LLM 컨텍스트에 그대로 들어간다
const MAX_LIST_ENTRIES: usize = 100;

pub struct ListDir;

impl Tool for ListDir {
    fn name(&self) -> &'static str {
        "list_dir"
    }
    fn description(&self) -> &'static str {
        "디렉토리의 파일/폴더 목록을 조회한다. 이름, 종류, 크기, 수정시각 포함."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "조회할 디렉토리 절대경로 (예: C:\\Users\\EST\\Downloads)" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let path = req_str(args, "path")?;
        let mut lines = Vec::new();
        let mut total = 0usize;
        let entries = fs::read_dir(path).with_context(|| format!("디렉토리 열기 실패: {path}"))?;
        for entry in entries.flatten() {
            total += 1;
            if lines.len() >= MAX_LIST_ENTRIES {
                continue; // 전체 개수는 계속 센다
            }
            let meta = entry.metadata();
            let kind = if entry.path().is_dir() { "dir" } else { "file" };
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .ok()
                .and_then(|m| m.modified().ok())
                .map(fmt_time)
                .unwrap_or_default();
            lines.push(format!(
                "{kind}\t{size}\t{modified}\t{}",
                entry.file_name().to_string_lossy()
            ));
        }
        if lines.is_empty() {
            return Ok("(빈 디렉토리)".into());
        }
        let mut out = format!("type\tsize\tmodified\tname\n{}", lines.join("\n"));
        if total > lines.len() {
            out.push_str(&format!("\n...(총 {total}개 중 {}개만 표시)", lines.len()));
        }
        Ok(out)
    }
}

pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> &'static str {
        "텍스트 파일 내용을 읽는다 (최대 64KB)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "파일 절대경로" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let path = req_str(args, "path")?;
        let meta = fs::metadata(path).with_context(|| format!("파일 없음: {path}"))?;
        let bytes = fs::read(path)?;
        let take = bytes.len().min(MAX_READ_BYTES as usize);
        let mut text = String::from_utf8_lossy(&bytes[..take]).into_owned();
        if meta.len() > MAX_READ_BYTES {
            text.push_str(&format!(
                "\n...(잘림: 전체 {} bytes 중 {} bytes만 표시)",
                meta.len(),
                take
            ));
        }
        Ok(text)
    }
}

pub struct WriteFile;

impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn description(&self) -> &'static str {
        "파일에 텍스트를 쓴다. 기존 파일은 overwrite=true 일 때만 덮어쓴다."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "파일 절대경로" },
                "content": { "type": "string", "description": "쓸 내용" },
                "overwrite": { "type": "boolean", "description": "기존 파일 덮어쓰기 허용 (기본 false)" }
            },
            "required": ["path", "content"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let path = req_str(args, "path")?;
        let content = req_str(args, "content")?;
        let overwrite = opt_bool(args, "overwrite").unwrap_or(false);
        if Path::new(path).exists() && !overwrite {
            bail!("파일이 이미 존재함: {path}. 덮어쓰려면 overwrite=true 로 다시 호출.");
        }
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(format!("저장 완료: {path} ({} bytes)", content.len()))
    }
}

pub struct MovePath;

impl Tool for MovePath {
    fn name(&self) -> &'static str {
        "move_path"
    }
    fn description(&self) -> &'static str {
        "파일/폴더를 이동하거나 이름을 바꾼다."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": { "type": "string", "description": "원본 절대경로" },
                "to": { "type": "string", "description": "대상 절대경로" }
            },
            "required": ["from", "to"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let from = req_str(args, "from")?;
        let to = req_str(args, "to")?;
        if Path::new(to).exists() {
            bail!("대상이 이미 존재함: {to}");
        }
        if let Some(parent) = Path::new(to).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(from, to).with_context(|| format!("이동 실패: {from} -> {to}"))?;
        Ok(format!("이동 완료: {from} -> {to}"))
    }
}

pub struct CopyPath;

impl Tool for CopyPath {
    fn name(&self) -> &'static str {
        "copy_path"
    }
    fn description(&self) -> &'static str {
        "파일을 복사한다."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": { "type": "string", "description": "원본 파일 절대경로" },
                "to": { "type": "string", "description": "대상 절대경로" }
            },
            "required": ["from", "to"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let from = req_str(args, "from")?;
        let to = req_str(args, "to")?;
        if Path::new(to).exists() {
            bail!("대상이 이미 존재함: {to}");
        }
        if let Some(parent) = Path::new(to).parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = fs::copy(from, to).with_context(|| format!("복사 실패: {from} -> {to}"))?;
        Ok(format!("복사 완료: {from} -> {to} ({bytes} bytes)"))
    }
}

pub struct DeletePath;

impl Tool for DeletePath {
    fn name(&self) -> &'static str {
        "delete_path"
    }
    fn description(&self) -> &'static str {
        "파일/폴더를 휴지통으로 보낸다 (영구 삭제 아님)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "삭제할 절대경로" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let path = req_str(args, "path")?;
        if !Path::new(path).exists() {
            bail!("경로 없음: {path}");
        }
        trash::delete(path).with_context(|| format!("휴지통 이동 실패: {path}"))?;
        Ok(format!("휴지통으로 이동 완료: {path}"))
    }
}

pub(crate) fn fmt_time(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let p = path.to_string_lossy().to_string();

        let out = WriteFile
            .execute(&json!({"path": p, "content": "안녕"}))
            .unwrap();
        assert!(out.contains("저장 완료"));

        let text = ReadFile.execute(&json!({"path": p})).unwrap();
        assert_eq!(text, "안녕");
    }

    #[test]
    fn write_refuses_overwrite_without_flag() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt").to_string_lossy().to_string();
        WriteFile
            .execute(&json!({"path": p, "content": "1"}))
            .unwrap();
        assert!(WriteFile
            .execute(&json!({"path": p, "content": "2"}))
            .is_err());
        WriteFile
            .execute(&json!({"path": p, "content": "2", "overwrite": true}))
            .unwrap();
    }

    #[test]
    fn list_dir_shows_entries() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("x.txt"), "x").unwrap();
        let out = ListDir
            .execute(&json!({"path": dir.path().to_string_lossy()}))
            .unwrap();
        assert!(out.contains("x.txt"));
    }

    #[test]
    fn move_and_copy() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt").to_string_lossy().to_string();
        let b = dir.path().join("b.txt").to_string_lossy().to_string();
        let c = dir.path().join("c.txt").to_string_lossy().to_string();
        std::fs::write(&a, "데이터").unwrap();
        CopyPath.execute(&json!({"from": a, "to": b})).unwrap();
        MovePath.execute(&json!({"from": b, "to": c})).unwrap();
        assert!(std::path::Path::new(&c).exists());
        assert!(!std::path::Path::new(&b).exists());
    }
}
