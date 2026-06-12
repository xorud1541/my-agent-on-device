use super::{opt_bool, req_str, Tool, ToolCtx};
use crate::tools::workspace::ensure_in_workspace;
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
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        // 없는 경로는 os 에러 대신 회복 지시(위치 힌트/사용자 질문)로 답한다
        if !Path::new(path).exists() {
            bail!(crate::tools::not_found_msg(path, &ctx.workspace()));
        }
        // 파일 경로가 오면 거부 대신 그 파일 1개의 정보를 돌려준다 — 거부+리다이렉트는
        // 2B 를 read_file→image_info 연쇄 배회로 몰았다 (2026-06-12 R8 실측)
        if Path::new(path).is_file() {
            let meta = fs::metadata(path);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .ok()
                .and_then(|m| m.modified().ok())
                .map(fmt_time)
                .unwrap_or_default();
            let name = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string());
            return Ok(format!(
                "type\tsize\tmodified\tname\nfile\t{size}\t{modified}\t{name}\n(이 경로는 폴더가 아니라 파일 1개입니다)"
            ));
        }
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
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        // 존재 확인을 먼저 — 힌트가 os 에러 꼬리 없이 문장 끝에 오도록 (2B 주의 분산 방지)
        if !Path::new(path).exists() {
            bail!(crate::tools::not_found_msg(path, &ctx.workspace()));
        }
        // 바이너리 포맷은 전용 도구로 안내한다 — 2B 가 PDF/이미지를 read_file 로 읽어
        // 원시 바이트로 컨텍스트를 오염시키는 배회를 차단 (2026-06-12 R3 실측)
        let ext = Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "pdf" => bail!("PDF 파일입니다. 텍스트를 보려면 pdf_extract_text 를 사용하세요."),
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "tiff" | "tif" => {
                bail!("이미지 파일입니다. 정보를 보려면 image_info 를 사용하세요.")
            }
            "zip" => {
                bail!("압축 파일입니다. 내용을 보려면 zip_extract(list_only=true) 를 사용하세요.")
            }
            _ => {}
        }
        let meta = fs::metadata(path).with_context(|| format!("파일 정보 조회 실패: {path}"))?;
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
        // '~라고 적어줘/저장해줘' 동사를 명시해야 2B 모델이 read_file 과 혼동하지 않는다
        // (2026-06-11 GT 평가: 동사 없는 설명에서 write 의도 3/5 가 read_file 로 오선택)
        "파일에 텍스트를 적어 저장한다 (적어줘/저장해줘/기록해줘/작성해줘/메모해줘). \
         사용자가 불러준 내용을 파일로 만들 때 사용. 기존 파일은 overwrite=true 일 때만 덮어쓴다."
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
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        // 이름만 온 경로("memo.txt")는 워크스페이스로 흡수 (2026-06-12 R7 패턴)
        let path = crate::tools::workspace::absorb_into_workspace(
            req_str(args, "path")?,
            &ctx.workspace(),
        );
        let path = &path.to_string_lossy().into_owned();
        let content = req_str(args, "content")?;
        let overwrite = opt_bool(args, "overwrite").unwrap_or(false);
        ensure_in_workspace(path, &ctx.workspace())?;
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
        "파일/폴더를 다른 폴더로 이동한다. 같은 폴더 안에서 이름만 바꾸려면 rename_file을 쓴다."
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
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let from = req_str(args, "from")?;
        let to = req_str(args, "to")?;
        // 이동은 원본 삭제를 수반하므로 양쪽 모두 워크스페이스 안이어야 한다
        ensure_in_workspace(from, &ctx.workspace())?;
        ensure_in_workspace(to, &ctx.workspace())?;
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

pub struct RenameFile;

impl Tool for RenameFile {
    fn name(&self) -> &'static str {
        "rename_file"
    }
    fn description(&self) -> &'static str {
        "파일/폴더의 이름을 바꾼다 (같은 폴더 안에서). 새 이름에 확장자를 생략하면 원본 확장자가 유지된다. 다른 폴더로 옮기려면 move_path를 쓴다."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "이름을 바꿀 파일/폴더의 절대경로" },
                "new_name": { "type": "string", "description": "새 이름 (확장자 포함, 경로 구분자 금지)" }
            },
            "required": ["path", "new_name"]
        })
    }
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        let new_name = req_str(args, "new_name")?.trim();
        if new_name.is_empty() {
            bail!("new_name이 비어 있음");
        }
        // 폴더를 넘는 변경은 move_path의 영역 — 같은 폴더 안 이름변경으로 의미를 좁힌다
        if new_name.contains('/') || new_name.contains('\\') {
            bail!("new_name에 경로 구분자를 쓸 수 없음. 다른 폴더로 옮기려면 move_path를 사용");
        }
        ensure_in_workspace(path, &ctx.workspace())?;
        let from = Path::new(path);
        if !from.exists() {
            bail!(crate::tools::not_found_msg(path, &ctx.workspace()));
        }
        // 새 이름에 점이 없고 원본이 확장자 있는 파일이면 확장자를 유지한다 —
        // 2B 가 "cat1으로 바꿔" 에서 확장자를 떨어뜨리는 실수를 도구가 흡수 (2026-06-12 실로그).
        // 폴더는 제외: 'v1.0' 폴더의 '0' 을 확장자로 오인하면 안 된다.
        let mut target_name = new_name.to_string();
        if from.is_file() && !new_name.contains('.') {
            if let Some(ext) = from.extension().and_then(|e| e.to_str()) {
                target_name = format!("{new_name}.{ext}");
            }
        }
        let to = from
            .parent()
            .ok_or_else(|| anyhow::anyhow!("부모 폴더를 찾을 수 없음: {path}"))?
            .join(&target_name);
        if to.exists() {
            bail!("같은 이름이 이미 존재함: {}", to.display());
        }
        fs::rename(from, &to).with_context(|| format!("이름 변경 실패: {path} -> {new_name}"))?;
        Ok(format!("이름 변경 완료: {path} -> {}", to.display()))
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
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let from = req_str(args, "from")?;
        let to = req_str(args, "to")?;
        // 복사는 원본을 건드리지 않으므로 대상만 워크스페이스 안이면 된다
        ensure_in_workspace(to, &ctx.workspace())?;
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
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        ensure_in_workspace(path, &ctx.workspace())?;
        if !Path::new(path).exists() {
            bail!(crate::tools::not_found_msg(path, &ctx.workspace()));
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
    use crate::tools::test_support::ctx_with_workspace;
    use tempfile::tempdir;

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let path = dir.path().join("a.txt");
        let p = path.to_string_lossy().to_string();

        let out = WriteFile
            .execute(&json!({"path": p, "content": "안녕"}), &ctx)
            .unwrap();
        assert!(out.contains("저장 완료"));

        let text = ReadFile.execute(&json!({"path": p}), &ctx).unwrap();
        assert_eq!(text, "안녕");
    }

    #[test]
    fn write_refuses_overwrite_without_flag() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let p = dir.path().join("a.txt").to_string_lossy().to_string();
        WriteFile
            .execute(&json!({"path": p, "content": "1"}), &ctx)
            .unwrap();
        assert!(WriteFile
            .execute(&json!({"path": p, "content": "2"}), &ctx)
            .is_err());
        WriteFile
            .execute(&json!({"path": p, "content": "2", "overwrite": true}), &ctx)
            .unwrap();
    }

    /// PDF/이미지/zip 을 read_file 로 읽으면 전용 도구로 안내한다 — 원시 바이트가
    /// 컨텍스트를 오염시키는 배회 차단 (2026-06-12 R3 실측)
    #[test]
    fn read_file_redirects_binary_formats_to_dedicated_tools() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        for (name, tool) in [
            ("a.pdf", "pdf_extract_text"),
            ("b.png", "image_info"),
            ("c.zip", "zip_extract"),
        ] {
            let p = dir.path().join(name);
            std::fs::write(&p, "bin").unwrap();
            let err = ReadFile
                .execute(&json!({"path": p.to_string_lossy()}), &ctx)
                .unwrap_err()
                .to_string();
            assert!(err.contains(tool), "{name}: {err}");
        }
    }

    /// list_dir 에 파일 경로가 오면 거부 대신 그 파일 1개의 정보를 돌려준다 (흡수).
    /// 거부+리다이렉트는 read_file→image_info 연쇄 배회를 유발했다 (2026-06-12 R8)
    #[test]
    fn list_dir_on_file_absorbs_with_single_entry() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let p = dir.path().join("x.txt");
        std::fs::write(&p, "xy").unwrap();
        let out = ListDir
            .execute(&json!({"path": p.to_string_lossy()}), &ctx)
            .unwrap();
        assert!(out.contains("x.txt"), "{out}");
        assert!(out.contains("파일 1개"), "{out}");
    }

    /// 이름만 온 write_file 경로는 워크스페이스로 흡수된다
    #[test]
    fn write_file_absorbs_bare_name_into_workspace() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let out = WriteFile
            .execute(&json!({"path": "memo.txt", "content": "안녕"}), &ctx)
            .unwrap();
        assert!(out.contains("저장 완료"), "{out}");
        assert!(dir.path().join("memo.txt").exists());
    }

    #[test]
    fn list_dir_shows_entries() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        std::fs::write(dir.path().join("x.txt"), "x").unwrap();
        let out = ListDir
            .execute(&json!({"path": dir.path().to_string_lossy()}), &ctx)
            .unwrap();
        assert!(out.contains("x.txt"));
    }

    #[test]
    fn move_and_copy() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let a = dir.path().join("a.txt").to_string_lossy().to_string();
        let b = dir.path().join("b.txt").to_string_lossy().to_string();
        let c = dir.path().join("c.txt").to_string_lossy().to_string();
        std::fs::write(&a, "데이터").unwrap();
        CopyPath
            .execute(&json!({"from": a, "to": b}), &ctx)
            .unwrap();
        MovePath
            .execute(&json!({"from": b, "to": c}), &ctx)
            .unwrap();
        assert!(std::path::Path::new(&c).exists());
        assert!(!std::path::Path::new(&b).exists());
    }

    #[test]
    fn rename_file_renames_in_place() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let a = dir.path().join("cat.png").to_string_lossy().to_string();
        std::fs::write(&a, "img").unwrap();
        let out = RenameFile
            .execute(&json!({"path": a, "new_name": "고양이.png"}), &ctx)
            .unwrap();
        assert!(out.contains("이름 변경 완료"), "{out}");
        assert!(dir.path().join("고양이.png").exists());
        assert!(!dir.path().join("cat.png").exists());
    }

    /// 2B 가 "cat1으로 바꿔" 요청에 확장자를 떨어뜨리는 실수를 도구가 흡수한다
    /// (2026-06-12 실로그/적대 테스트: cat.png → "cat1", report.txt → "final")
    #[test]
    fn rename_file_keeps_extension_when_omitted() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let a = dir.path().join("cat.png").to_string_lossy().to_string();
        std::fs::write(&a, "img").unwrap();
        RenameFile
            .execute(&json!({"path": a, "new_name": "cat1"}), &ctx)
            .unwrap();
        assert!(dir.path().join("cat1.png").exists(), "원본 확장자 유지");
        assert!(!dir.path().join("cat1").exists());
    }

    #[test]
    fn rename_file_respects_explicit_extension() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let a = dir.path().join("cat.png").to_string_lossy().to_string();
        std::fs::write(&a, "img").unwrap();
        RenameFile
            .execute(&json!({"path": a, "new_name": "cat1.jpg"}), &ctx)
            .unwrap();
        assert!(dir.path().join("cat1.jpg").exists(), "명시한 확장자 존중");
    }

    #[test]
    fn rename_file_does_not_append_extension_to_directories() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        // 'v1.0' 폴더의 '0'을 확장자로 오인해 'release.0' 을 만들면 안 된다
        let d = dir.path().join("v1.0");
        std::fs::create_dir(&d).unwrap();
        RenameFile
            .execute(
                &json!({"path": d.to_string_lossy(), "new_name": "release"}),
                &ctx,
            )
            .unwrap();
        assert!(dir.path().join("release").exists());
        assert!(!dir.path().join("release.0").exists());
    }

    /// 파일이 없으면 워크스페이스 일대에서 같은 이름을 찾아 힌트를 준다.
    /// (2026-06-12 실로그: 오염된 워크스페이스에서 파일을 못 찾자 환각 경로로 배회 —
    ///  2B 에게 "파일 없음"은 막다른 골목, 에러가 올바른 경로를 알려줘야 복구한다)
    #[test]
    fn rename_file_not_found_error_includes_location_hint() {
        let dir = tempdir().unwrap();
        // 워크스페이스가 하위 폴더로 좁혀진 상황: 실제 파일은 부모에 있다
        let ws = dir.path().join("pngs");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(dir.path().join("cat.png"), "img").unwrap();

        let ctx = ctx_with_workspace(&ws);
        let wrong = ws.join("cat.png").to_string_lossy().to_string();
        let err = RenameFile
            .execute(&json!({"path": wrong, "new_name": "cat1"}), &ctx)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cat.png"), "{err}");
        let real = dir
            .path()
            .join("cat.png")
            .to_string_lossy()
            .replace('\\', "/");
        assert!(err.contains(&real), "실제 위치 힌트 없음: {err}");
    }

    #[test]
    fn rename_file_rejects_existing_target() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let a = dir.path().join("a.txt").to_string_lossy().to_string();
        std::fs::write(&a, "1").unwrap();
        std::fs::write(dir.path().join("b.txt"), "2").unwrap();
        let err = RenameFile
            .execute(&json!({"path": a, "new_name": "b.txt"}), &ctx)
            .unwrap_err();
        assert!(err.to_string().contains("이미 존재"), "{err}");
    }

    #[test]
    fn rename_file_rejects_separators_in_new_name() {
        // 폴더를 넘는 이름변경은 이동(move_path)의 영역 — 의미가 섞이면 2B 라우팅이 다시 흐려진다
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let a = dir.path().join("a.txt").to_string_lossy().to_string();
        std::fs::write(&a, "1").unwrap();
        assert!(RenameFile
            .execute(&json!({"path": a, "new_name": "sub/b.txt"}), &ctx)
            .is_err());
        assert!(RenameFile
            .execute(&json!({"path": a, "new_name": "sub\\b.txt"}), &ctx)
            .is_err());
    }

    #[test]
    fn rename_file_outside_workspace_rejected() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let ctx = ctx_with_workspace(&ws);
        let outside = dir.path().join("밖.txt").to_string_lossy().to_string();
        std::fs::write(&outside, "x").unwrap();
        let err = RenameFile
            .execute(&json!({"path": outside, "new_name": "안.txt"}), &ctx)
            .unwrap_err();
        assert!(err.to_string().contains("워크스페이스 밖"), "{err}");
    }

    #[test]
    fn write_outside_workspace_rejected() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let ctx = ctx_with_workspace(&ws);
        let outside = dir.path().join("밖.txt").to_string_lossy().to_string();
        let err = WriteFile
            .execute(&json!({"path": outside, "content": "x"}), &ctx)
            .unwrap_err();
        assert!(err.to_string().contains("워크스페이스 밖"), "{err}");
    }

    #[test]
    fn read_outside_workspace_allowed() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let ctx = ctx_with_workspace(&ws);
        let outside = dir.path().join("읽기.txt");
        std::fs::write(&outside, "읽힘").unwrap();
        let out = ReadFile
            .execute(&json!({"path": outside.to_string_lossy()}), &ctx)
            .unwrap();
        assert_eq!(out, "읽힘");
    }
}
