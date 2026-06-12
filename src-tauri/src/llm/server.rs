use crate::config::AppConfig;
use anyhow::{bail, Context, Result};
use std::time::Duration;
use tokio::process::{Child, Command};

/// llama-server 사이드카 프로세스 관리자.
pub struct LlamaServer {
    child: Option<Child>,
    pub base_url: String,
}

impl LlamaServer {
    pub fn new() -> Self {
        Self { child: None, base_url: String::new() }
    }

    /// 서버를 띄우고 /health 가 200을 줄 때까지 대기한다 (모델 로드 포함).
    pub async fn start(&mut self, cfg: &AppConfig) -> Result<()> {
        self.stop().await;

        if !std::path::Path::new(&cfg.server_exe).exists() {
            bail!("llama-server 실행 파일이 없습니다: {}", cfg.server_exe);
        }
        if !std::path::Path::new(&cfg.model_path).exists() {
            bail!("모델 파일이 없습니다: {}", cfg.model_path);
        }

        let mut cmd = Command::new(&cfg.server_exe);
        cmd.args([
            "-m", &cfg.model_path,
            "--port", &cfg.port.to_string(),
            "--host", "127.0.0.1",
            "-ngl", &cfg.n_gpu_layers.to_string(),
            "-c", &cfg.ctx_size.to_string(),
            "--jinja",
            "--no-webui",
        ]);
        // 디바이스가 지정된 경우에만 --device 부착.
        // Windows=Vulkan0 명시, macOS Metal·Linux=빈 값 → 인자 생략 후 자동 선택(-ngl 오프로드).
        // (빈 값으로 `--device ""` 를 넘기면 llama-server 가 즉시 죽는다)
        if !cfg.device.trim().is_empty() {
            cmd.args(["--device", &cfg.device]);
        }
        if cfg.reasoning_budget == 0 {
            // 사고 비활성화: 강제 사고 종료 후 즉시 EOS 가 나오는 불안정 경로 자체를 제거.
            // Qwen3.5-2B 기준 사고 없이도 한국어 툴콜 5/5, 호출당 1~5초 (bench 참고)
            cmd.args(["--reasoning", "off"]);
        } else {
            cmd.args(["--reasoning-budget", &cfg.reasoning_budget.to_string()]);
            // 예산 강제 종료 직후 모델이 EOS 로 끝내버리지 않도록 본문/행동으로 유도
            cmd.args([
                "--reasoning-budget-message",
                "생각할 시간이 끝났다. 지금 바로 도구를 호출하거나 한국어로 최종 답변한다.",
            ]);
        }
        #[cfg(windows)]
        {
            // 릴리즈 빌드에서 콘솔 창이 뜨지 않도록
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        // 서버 출력은 파일로 (문제 추적용)
        let log_path = crate::logging::llama_server_log_file();
        if let Ok(log) = std::fs::File::create(&log_path) {
            if let Ok(log2) = log.try_clone() {
                cmd.stdout(std::process::Stdio::from(log));
                cmd.stderr(std::process::Stdio::from(log2));
            }
        }
        cmd.kill_on_drop(true);
        let child = cmd.spawn().context("llama-server 실행 실패")?;
        self.child = Some(child);
        self.base_url = format!("http://127.0.0.1:{}", cfg.port);

        // 모델 로드 대기 (최대 120초)
        let client = reqwest::Client::new();
        let health = format!("{}/health", self.base_url);
        for _ in 0..120 {
            if let Some(child) = &mut self.child {
                if let Ok(Some(status)) = child.try_wait() {
                    bail!("llama-server 가 즉시 종료됨 (exit: {status})");
                }
            }
            if let Ok(resp) = client.get(&health).timeout(Duration::from_secs(2)).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        self.stop().await;
        bail!("llama-server 가 120초 내에 준비되지 않음")
    }

    pub async fn is_healthy(&self) -> bool {
        if self.base_url.is_empty() {
            return false;
        }
        let client = reqwest::Client::new();
        matches!(
            client
                .get(format!("{}/health", self.base_url))
                .timeout(Duration::from_secs(2))
                .send()
                .await,
            Ok(resp) if resp.status().is_success()
        )
    }

    pub async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }
    }
}
