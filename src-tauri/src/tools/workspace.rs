use super::{req_str, Tool, ToolCtx};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

/// 상대경로/이름만 온 출력 경로를 워크스페이스 기준 절대경로로 흡수한다.
/// 2B 는 "album.pdf" 처럼 이름만 주는 실수가 잦은데, 거부하면 경로를 고치지 못하고
/// 의도를 무시한 우회로 빠진다 — 의도가 명백하므로(워크스페이스에 저장) 도구가
/// 선의로 해석한다 (2026-06-12 R7 실측: output_path="album.pdf" 거부 → 자동 이름
/// images.pdf 로 우회 저장 → 사용자가 말한 이름이 무시됨).
pub fn absorb_into_workspace(path: &str, ws: &Path) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        ws.join(p)
    }
}

/// 산출물 기본 경로(출력 경로 미지정)를 결정한다.
/// 입력이 워크스페이스 *안*이면 입력 옆(`file_name`), *밖*이면(앱 캐시 캡처 등)
/// 워크스페이스 루트 아래 `file_name` 으로 떨군다.
/// 캐시에서 연 캡처본을 배경제거/변환할 때 기본 출력이 캐시(워크스페이스 밖)로 떨어져
/// `ensure_in_workspace` 에 거부당하던 문제를 원천 차단한다 (2026-06-13).
pub fn default_output_in_workspace(input: &Path, file_name: &str, ws: &Path) -> PathBuf {
    let inside = input
        .parent()
        .map(|d| ensure_in_workspace(&d.to_string_lossy(), ws).is_ok())
        .unwrap_or(false);
    if inside {
        input.with_file_name(file_name)
    } else {
        ws.join(file_name)
    }
}

/// 쓰기성 경로가 워크스페이스 안인지 검사한다. 위반 시 모델이 경로를 고쳐
/// 재시도할 수 있는 한국어 오류를 돌려준다.
///
/// 어휘적 비교 — 출력 경로는 아직 존재하지 않을 수 있어 canonicalize 불가.
/// `..` 는 거부, 구분자(`/`↔`\`)와 대소문자(Windows)는 정규화한다.
pub fn ensure_in_workspace(path: &str, workspace: &Path) -> Result<()> {
    let target = normalize(Path::new(path))?;
    let ws = normalize(workspace)?;
    if ws.is_empty() {
        return Ok(()); // 워크스페이스를 알 수 없으면 제한하지 않음
    }
    if target.len() >= ws.len() && target[..ws.len()] == ws[..] {
        return Ok(());
    }
    bail!(
        "워크스페이스 밖에는 쓸 수 없음: {path}. 파일 생성/수정/삭제는 워크스페이스({}) 안의 경로로 다시 시도하세요.",
        workspace.display()
    )
}

/// 경로를 소문자 컴포넌트 목록으로 정규화. 상대경로/`..` 는 거부.
fn normalize(p: &Path) -> Result<Vec<String>> {
    if p.as_os_str().is_empty() {
        return Ok(vec![]);
    }
    if p.is_relative() {
        bail!("절대경로가 필요함: {}", p.display());
    }
    let mut parts = Vec::new();
    for c in p.components() {
        match c {
            Component::ParentDir => bail!("경로에 .. 사용 불가: {}", p.display()),
            Component::CurDir => {}
            other => parts.push(other.as_os_str().to_string_lossy().to_lowercase()),
        }
    }
    Ok(parts)
}

pub struct SetWorkspace;

impl Tool for SetWorkspace {
    fn name(&self) -> &'static str {
        "set_workspace"
    }
    fn description(&self) -> &'static str {
        "워크스페이스(작업 폴더)를 변경한다. 사용자가 작업 공간을 바꿔달라고 할 때만 사용. 폴더가 없으면 만든다."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "새 워크스페이스 절대경로" }
            },
            "required": ["path"]
        })
    }
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let path = req_str(args, "path")?;
        let p = PathBuf::from(path);
        if p.is_relative() {
            bail!("절대경로가 필요함: {path}");
        }
        std::fs::create_dir_all(&p)
            .map_err(|e| anyhow::anyhow!("워크스페이스 생성 실패: {path} ({e})"))?;
        ctx.update_config(|cfg| cfg.workspace_dir = p.to_string_lossy().into_owned())?;
        Ok(format!("워크스페이스 변경 완료: {path}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::ctx_with_workspace;
    use tempfile::tempdir;

    #[test]
    fn default_output_routes_outside_input_into_workspace() {
        // 캐시(워크스페이스 밖) 입력 → 산출물 기본 경로는 워크스페이스 루트로
        let ws = Path::new("/home/user/ws");
        let got = default_output_in_workspace(
            Path::new("/home/user/Library/Caches/app/captures/x.png"),
            "x_nobg.png",
            ws,
        );
        assert_eq!(got, Path::new("/home/user/ws/x_nobg.png"));
    }

    #[test]
    fn default_output_keeps_inside_input_next_to_it() {
        // 워크스페이스 안 입력 → 기존대로 입력 옆
        let ws = Path::new("/home/user/ws");
        let got = default_output_in_workspace(
            Path::new("/home/user/ws/sub/y.png"),
            "y_edited.png",
            ws,
        );
        assert_eq!(got, Path::new("/home/user/ws/sub/y_edited.png"));
    }

    #[test]
    fn inside_workspace_passes() {
        let ws = Path::new(r"C:\Users\EST\work");
        assert!(ensure_in_workspace(r"C:\Users\EST\work\a.txt", ws).is_ok());
        assert!(ensure_in_workspace(r"C:\Users\EST\work\sub\b.png", ws).is_ok());
        // 워크스페이스 자체도 허용
        assert!(ensure_in_workspace(r"C:\Users\EST\work", ws).is_ok());
    }

    #[test]
    fn outside_workspace_fails_with_korean_hint() {
        let ws = Path::new(r"C:\Users\EST\work");
        let err = ensure_in_workspace(r"C:\Users\EST\Desktop\a.txt", ws).unwrap_err();
        assert!(err.to_string().contains("워크스페이스 밖"), "{err}");
    }

    #[test]
    fn slash_and_case_are_normalized() {
        let ws = Path::new(r"C:\Users\EST\Work");
        assert!(ensure_in_workspace("C:/users/est/work/파일.txt", ws).is_ok());
    }

    #[test]
    fn parent_dir_escape_rejected() {
        let ws = Path::new(r"C:\Users\EST\work");
        assert!(ensure_in_workspace(r"C:\Users\EST\work\..\Desktop\x.txt", ws).is_err());
    }

    #[test]
    fn prefix_name_collision_is_not_inside() {
        // C:\Users\EST\work2 는 C:\Users\EST\work 의 내부가 아니다
        let ws = Path::new(r"C:\Users\EST\work");
        assert!(ensure_in_workspace(r"C:\Users\EST\work2\a.txt", ws).is_err());
    }

    #[test]
    fn relative_path_rejected() {
        let ws = Path::new(r"C:\Users\EST\work");
        assert!(ensure_in_workspace("a.txt", ws).is_err());
    }

    #[test]
    fn set_workspace_creates_dir_and_updates_config() {
        let dir = tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());
        let new_ws = dir.path().join("새작업공간");
        let out = SetWorkspace
            .execute(&json!({"path": new_ws.to_string_lossy()}), &ctx)
            .unwrap();
        assert!(out.contains("변경 완료"), "{out}");
        assert!(new_ws.is_dir());
        assert_eq!(
            ctx.config.lock().unwrap().workspace_dir,
            new_ws.to_string_lossy()
        );
    }
}
