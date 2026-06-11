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
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_exe: default_server_exe(),
            model_path: default_model_path(),
            port: 8736,
            device: "Vulkan0".into(),
            n_gpu_layers: 99,
            ctx_size: 8192,
            max_tool_rounds: 8,
            // 툴콜 인자 JSON 안정성 우선 (높을수록 무이스케이프 경로 등 미스생성 증가)
            temperature: 0.4,
            max_output_tokens: 1024,
        }
    }
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
