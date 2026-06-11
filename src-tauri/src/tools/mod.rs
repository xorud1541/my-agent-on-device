mod archive;
mod capture;
mod fs_tools;
mod image_ai;
mod image_tools;
mod pdf_make;
mod pdf_tools;
mod profile;
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
                Box::new(profile::UpdateProfile),
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
