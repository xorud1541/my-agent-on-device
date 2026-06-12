use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 앱 설정. `%APPDATA%/com.estsoft.local-agent/config.json`에 저장.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// llama-server.exe 경로
    pub server_exe: String,
    /// GGUF 모델 경로
    pub model_path: String,
    /// llama-server 포트
    pub port: u16,
    /// 사용할 디바이스 (iGPU = Vulkan0)
    pub device: String,
    /// GPU 오프로드 레이어 수
    pub n_gpu_layers: i32,
    /// 컨텍스트 길이
    pub ctx_size: u32,
    /// 한 턴에서 허용하는 최대 도구 호출 횟수
    pub max_tool_rounds: u32,
    pub temperature: f32,
    /// LLM 호출당 출력 토큰 상한 (레이턴시 예산: ~20 t/s 기준 1024 ≈ 50초)
    pub max_output_tokens: u32,
    /// 사고(thinking) 토큰 예산. 0 = 사고 끔(기본), N>0 = 예산, -1 = 무제한.
    /// 사고를 켜면 호출당 +10초 이상 느려지고, 예산 강제 종료 시 빈 응답이 날 수 있다.
    pub reasoning_budget: i32,
    /// 워크스페이스(작업 폴더). 쓰기성 도구의 출력 경로는 이 안으로 제한된다.
    pub workspace_dir: String,
    /// 사용자 이름 (빈 문자열 = 아직 모름 → 에이전트가 대화 초반에 묻는다)
    pub user_name: String,
    /// 에이전트 이름 (빈 문자열 = 아직 없음 → 사용자에게 지어달라고 부탁)
    pub agent_name: String,
    /// 배경제거 ONNX 모델 경로
    pub removebg_model: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_exe: default_server_exe(),
            model_path: default_model_path(),
            port: 8736,
            device: "Vulkan0".into(),
            n_gpu_layers: 99,
            // iGPU 공유메모리 여유가 크고(~18GB) 2B 모델 KV 캐시가 작아 16K 가 안전
            ctx_size: 16384,
            max_tool_rounds: 8,
            // 툴콜 인자 JSON 안정성 우선 (높을수록 무이스케이프 경로 등 미스생성 증가)
            temperature: 0.4,
            max_output_tokens: 1024,
            reasoning_budget: 0,
            workspace_dir: default_workspace_dir(),
            user_name: String::new(),
            agent_name: String::new(),
            removebg_model: default_removebg_model(),
        }
    }
}

fn default_workspace_dir() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

fn default_removebg_model() -> String {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".alice")
        .join("models")
        .join("removeBG.ort")
        .to_string_lossy()
        .into_owned()
}

fn default_server_exe() -> String {
    let home = dirs::home_dir().unwrap_or_default();
    home.join("Downloads")
        .join("llama-b9334-bin-win-vulkan-x64")
        .join("llama-server.exe")
        .to_string_lossy()
        .into_owned()
}

fn default_model_path() -> String {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".lmstudio")
        .join("models")
        .join("lmstudio-community")
        .join("Qwen3.5-2B-GGUF")
        .join("Qwen3.5-2B-Q4_K_M.gguf")
        .to_string_lossy()
        .into_owned()
}

fn config_file() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("com.estsoft.local-agent").join("config.json")
}

impl AppConfig {
    /// 워크스페이스 절대경로. 설정이 비어있거나 폴더가 사라졌으면 홈 디렉토리로 폴백.
    /// (2026-06-12 실로그: 워크스페이스가 삭제된 폴더를 가리키면 모든 베어네임 해석이
    ///  허공을 가리켜 턴이 연쇄 실패 — 존재하는 경로만 워크스페이스가 될 수 있다)
    pub fn workspace_path(&self) -> PathBuf {
        if !self.workspace_dir.trim().is_empty() {
            let p = PathBuf::from(&self.workspace_dir);
            if p.is_dir() {
                return p;
            }
        }
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn load() -> Self {
        let path = config_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 설정된 워크스페이스 폴더가 삭제됐으면 존재하는 기본 경로로 폴백한다.
    /// (2026-06-12 실로그: 모델이 set_workspace 로 하위 폴더 지정 → 사용자가 폴더 삭제 →
    ///  이후 모든 베어네임 해석이 허공을 가리켜 세 턴 연속 실패)
    #[test]
    fn workspace_path_falls_back_when_dir_missing() {
        let cfg = AppConfig {
            workspace_dir: "C:/이런폴더는없습니다/진짜로없음".into(),
            ..AppConfig::default()
        };
        let p = cfg.workspace_path();
        assert!(p.exists(), "사라진 워크스페이스는 존재하는 경로로 폴백해야 함: {p:?}");
    }

    #[test]
    fn workspace_path_keeps_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = AppConfig {
            workspace_dir: tmp.path().to_string_lossy().into_owned(),
            ..AppConfig::default()
        };
        assert_eq!(cfg.workspace_path(), tmp.path());
    }
}
