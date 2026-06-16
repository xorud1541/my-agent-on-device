mod archive;
mod capture;
mod fs_tools;
mod image_ai;
mod image_tools;
mod pdf_make;
mod pdf_tools;
mod search;
pub mod workspace;

use crate::config::AppConfig;
use crate::models::AgentEvent;
use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 도구 실행 컨텍스트. 워크스페이스/페르소나 등 살아있는 설정과,
/// 설정 변경을 영속화·방송하는 통로를 도구에 전달한다.
pub struct ToolCtx {
    pub config: Arc<Mutex<AppConfig>>,
    persist: Arc<dyn Fn(&AppConfig) -> Result<()> + Send + Sync>,
    notify: Arc<dyn Fn(AgentEvent) + Send + Sync>,
}

impl ToolCtx {
    pub fn new(
        config: Arc<Mutex<AppConfig>>,
        persist: Arc<dyn Fn(&AppConfig) -> Result<()> + Send + Sync>,
        notify: Arc<dyn Fn(AgentEvent) + Send + Sync>,
    ) -> Self {
        Self {
            config,
            persist,
            notify,
        }
    }

    /// 영속화/방송 없는 컨텍스트 — 테스트와 단독 실행용
    pub fn noop(config: AppConfig) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            persist: Arc::new(|_| Ok(())),
            notify: Arc::new(|_| {}),
        }
    }

    pub fn workspace(&self) -> PathBuf {
        self.config.lock().unwrap().workspace_path()
    }

    /// 설정을 갱신하고 저장한 뒤 ConfigChanged 를 방송한다 (도구 → UI 동기화 경로)
    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> Result<AppConfig> {
        let snapshot = {
            let mut cfg = self.config.lock().unwrap();
            f(&mut cfg);
            cfg.clone()
        };
        (self.persist)(&snapshot)?;
        (self.notify)(AgentEvent::ConfigChanged {
            config: snapshot.clone(),
        });
        Ok(snapshot)
    }
}

/// 에이전트가 호출할 수 있는 도구. 구현은 동기 — 에이전트 루프에서 spawn_blocking 으로 돌린다.
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON Schema (OpenAI function parameters 규격)
    fn parameters(&self) -> Value;
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String>;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn with_default_tools() -> Self {
        Self {
            tools: vec![
                Box::new(fs_tools::ListDir),
                Box::new(fs_tools::ReadFile),
                Box::new(fs_tools::WriteFile),
                Box::new(fs_tools::MovePath),
                Box::new(fs_tools::RenameFile),
                Box::new(fs_tools::CopyPath),
                Box::new(fs_tools::DeletePath),
                Box::new(search::SearchFiles),
                Box::new(image_tools::ImageInfo),
                Box::new(image_tools::ImageTransform),
                Box::new(image_ai::RemoveBackground),
                Box::new(pdf_tools::PdfExtractText),
                Box::new(pdf_make::ImagesToPdf),
                Box::new(capture::ScreenCapture),
                Box::new(archive::ZipCreate),
                Box::new(archive::ZipExtract),
                Box::new(workspace::SetWorkspace),
            ],
        }
    }

    /// OpenAI `tools` 배열로 직렬화
    pub fn schemas(&self) -> Value {
        self.schemas_excluding(&[])
    }

    /// 일부 도구를 제외한 `tools` 배열. 작은 모델의 도구 선택 혼동을 막기 위해
    /// 턴 단위로 경쟁 도구를 숨기는 라우팅(agent::tools_to_exclude)에 쓴다.
    pub fn schemas_excluding(&self, excluded: &[&str]) -> Value {
        Value::Array(
            self.tools
                .iter()
                .filter(|t| !excluded.contains(&t.name()))
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name(),
                            "description": t.description(),
                            "parameters": t.parameters(),
                        }
                    })
                })
                .collect(),
        )
    }

    /// `allowed` 화이트리스트에 든 도구만(그중 `excluded` 는 제외) 직렬화한다.
    /// B 백스톱: RAG 턴에 조회(읽기) 도구만 남겨, 게이트 오판 시 모델이 회복하게 한다.
    pub fn schemas_only(&self, allowed: &[&str], excluded: &[&str]) -> Value {
        Value::Array(
            self.tools
                .iter()
                .filter(|t| allowed.contains(&t.name()) && !excluded.contains(&t.name()))
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name(),
                            "description": t.description(),
                            "parameters": t.parameters(),
                        }
                    })
                })
                .collect(),
        )
    }

    pub fn execute(&self, name: &str, args: &Value, ctx: &ToolCtx) -> Result<String> {
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.execute(args, ctx),
            None => anyhow::bail!("알 수 없는 도구: {name}"),
        }
    }
}

/// 인자 추출 헬퍼
pub(crate) fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("필수 인자 누락: {key}"))
}

pub(crate) fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub(crate) fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

pub(crate) fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

/// "파일 없음" 오류 메시지를 만든다. 주변에서 같은/비슷한 이름을 찾으면 그 경로로
/// 재시도하라는 힌트를, 못 찾으면 "경로를 추측하지 말고 사용자에게 위치를 물어보라"는
/// 지시를 담는다. 2B 는 시스템 프롬프트 규칙(13)보다 직전 도구 결과의 지시를 더 잘
/// 따른다 — 실패 인지→재계획/질문 흐름을 에러 텍스트가 직접 이끈다 (2026-06-12).
pub(crate) fn not_found_msg(requested: &str, ws: &std::path::Path) -> String {
    let hint = not_found_hint(requested, ws);
    if !hint.is_empty() {
        return format!("파일 없음: {requested}.{hint}");
    }
    // 유사 이름도 없으면 현재 폴더의 실제 목록을 후보로 보여준다 — 이름이 크게 다른
    // 파일("cat.png" 요청, 실제 "새 파일 1.png")로 회복할 1라운드 기회를 만든다
    // (2026-06-12 S1-t3: 목록 없이 즉시 질문 종결 → 바로 옆 파일을 두고 사용자에게 물음)
    let names: Vec<String> = std::fs::read_dir(ws)
        .map(|rd| {
            rd.flatten()
                .take(10)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        format!(
            "파일 없음: {requested}. 주변 폴더에서도 같은 이름을 찾지 못했고 현재 폴더는 비어 \
             있습니다. 다른 경로를 추측해 재시도하지 말고, 사용자에게 파일의 정확한 위치(폴더)를 물어보세요."
        )
    } else {
        format!(
            "파일 없음: {requested}. 현재 폴더에는 다음이 있습니다: {}. 요청한 파일이 이 중에 \
             보이면 그 이름으로 다시 시도하고, 없으면 다른 경로를 추측하지 말고 사용자에게 \
             파일의 정확한 위치(폴더)를 물어보세요.",
            names.join(", ")
        )
    }
}

/// 요청 경로에 파일이 없을 때 워크스페이스 일대(부모 1단계 + 하위 깊이 2)에서 같은/비슷한
/// 이름을 찾아 힌트 문장을 만든다. 2B 에게 "파일 없음"은 막다른 골목이라, 에러가
/// 올바른 경로를 직접 알려줘야 다음 라운드에서 복구한다
/// (2026-06-12 실로그: 오염된 워크스페이스에서 pngs.zip 못 찾음 → 환각 zip 경로로 배회).
/// 유사 일치는 확장자가 같고 어간이 한쪽을 포함할 때만 — 2B 의 토큰 융합 오타
/// ("pngs.zip" → "pngs.pngs.zip")를 잡는다. 정확 일치가 항상 우선.
pub(crate) fn not_found_hint(requested: &str, ws: &std::path::Path) -> String {
    let req_path = std::path::Path::new(requested);
    let Some(name) = req_path.file_name() else {
        return String::new();
    };
    let req_stem = req_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let req_ext = req_path
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // 부모를 먼저 본다 — "워크스페이스가 하위로 좁혀진" 오염 케이스가 실제 사고였다
    let mut queue: Vec<(std::path::PathBuf, u8)> = Vec::new();
    if let Some(parent) = ws.parent() {
        queue.push((parent.to_path_buf(), 1));
    }
    queue.push((ws.to_path_buf(), 0));

    let mut fuzzy: Option<std::path::PathBuf> = None;
    let mut scanned = 0usize;
    while let Some((dir, depth)) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            scanned += 1;
            if scanned > 500 {
                break; // 거대 폴더에서 비용 폭주 방지 — 그때까지의 유사 후보는 유효
            }
            let p = e.path();
            if p == req_path || !p.is_file() {
                if p.is_dir() && depth < 2 {
                    queue.push((p, depth + 1));
                }
                continue;
            }
            if p.file_name() == Some(name) {
                return format!(
                    " 같은 이름의 파일 발견: {} — 이 경로로 다시 시도하세요.",
                    p.to_string_lossy().replace('\\', "/")
                );
            }
            if fuzzy.is_none() {
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let ext = p
                    .extension()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if !req_ext.is_empty()
                    && ext == req_ext
                    && stem.chars().count() >= 3
                    && (req_stem.contains(&stem) || stem.contains(&req_stem))
                {
                    fuzzy = Some(p);
                }
            }
        }
    }
    match fuzzy {
        Some(p) => format!(
            " 비슷한 파일 발견: {} — 이 경로로 다시 시도하세요.",
            p.to_string_lossy().replace('\\', "/")
        ),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B 백스톱: RAG 턴에 조회 도구만 노출할 때 쓰는 화이트리스트 직렬화.
    /// allowed 에 든 것만, 그중 excluded 는 빼고, 등록 순서를 유지한다.
    #[test]
    fn schemas_only_exposes_allowed_minus_excluded() {
        let reg = ToolRegistry::with_default_tools();
        let v = reg.schemas_only(
            &["list_dir", "read_file", "search_files"],
            &["search_files"],
        );
        let names: Vec<String> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["list_dir", "read_file"]);
        // 쓰기성 도구는 화이트리스트에 없으니 절대 노출되면 안 된다
        assert!(!names.contains(&"write_file".to_string()));
        assert!(!names.contains(&"delete_path".to_string()));
    }

    /// 주변에서 못 찾은 "파일 없음"은 막다른 골목이 아니라 사용자 질문으로 이어져야 한다.
    /// 2B 는 프롬프트 규칙보다 직전 도구 결과의 지시를 더 잘 따른다 (2026-06-12).
    #[test]
    fn not_found_msg_instructs_asking_user_when_no_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let msg = not_found_msg(&dir.path().join("유니콘.pdf").to_string_lossy(), dir.path());
        assert!(msg.contains("물어보세요"), "{msg}");
        assert!(msg.contains("추측"), "재시도 배회 금지 지시 없음: {msg}");
    }

    /// 유사 이름조차 없으면 현재 폴더의 실제 파일 목록을 후보로 제시한다
    /// (2026-06-12 S1-t3: "cat.png" 요청, 실제 "새 파일 1.png" — 목록이 회복 단서)
    #[test]
    fn not_found_msg_lists_current_folder_candidates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("새 파일 1.png"), "img").unwrap();
        let msg = not_found_msg(&dir.path().join("cat.png").to_string_lossy(), dir.path());
        assert!(msg.contains("새 파일 1.png"), "{msg}");
        assert!(msg.contains("다시 시도"), "{msg}");
        assert!(msg.contains("물어보세요"), "{msg}");
    }

    /// 주변에 같은 이름이 있으면 질문 대신 그 경로로 재시도를 지시한다
    #[test]
    fn not_found_msg_prefers_location_hint_over_question() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(dir.path().join("report.pdf"), "p").unwrap();
        let msg = not_found_msg(&ws.join("report.pdf").to_string_lossy(), &ws);
        assert!(msg.contains("다시 시도하세요"), "{msg}");
        assert!(!msg.contains("물어보세요"), "{msg}");
    }

    /// 오타 경로도 유사 일치로 힌트를 준다 (2026-06-12 실로그: 모델이 "pngs.zip"을
    /// "pngs.pngs.zip"으로 융합 — 정확 일치만으로는 2B 오타를 못 잡는다)
    #[test]
    fn not_found_hint_matches_fuzzy_typo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pngs.zip"), "z").unwrap();
        let typo = dir
            .path()
            .join("pngs.pngs.zip")
            .to_string_lossy()
            .to_string();
        let hint = not_found_hint(&typo, dir.path());
        assert!(hint.contains("pngs.zip"), "{hint}");
        assert!(hint.contains("다시 시도"), "{hint}");
    }

    #[test]
    fn not_found_hint_fuzzy_requires_same_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pngs.txt"), "t").unwrap();
        // 확장자가 다르면 유사 일치하지 않는다 (오인 유도 방지)
        let hint = not_found_hint(
            &dir.path().join("pngs.pngs.zip").to_string_lossy(),
            dir.path(),
        );
        assert!(hint.is_empty(), "{hint}");
    }

    /// 2B 라우팅 정확도를 위해 도구 표면을 좁게 유지한다:
    /// 이름변경은 rename_file 전담, update_profile 은 제거됨 (2026-06-12 오라우팅 사고).
    #[test]
    fn registry_has_rename_file_and_no_update_profile() {
        let names: Vec<String> = ToolRegistry::with_default_tools()
            .schemas()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"rename_file".to_string()), "{names:?}");
        assert!(!names.contains(&"update_profile".to_string()), "{names:?}");
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// 워크스페이스를 지정한 테스트용 컨텍스트
    pub fn ctx_with_workspace(ws: &std::path::Path) -> ToolCtx {
        let cfg = AppConfig {
            workspace_dir: ws.to_string_lossy().into_owned(),
            ..AppConfig::default()
        };
        ToolCtx::noop(cfg)
    }
}
