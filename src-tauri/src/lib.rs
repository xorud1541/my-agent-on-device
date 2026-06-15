pub mod agent;
mod commands;
pub mod config;
pub mod llm;
pub mod logging;
pub mod models;
pub mod sessions;
pub mod tools;
pub mod workspace_summary;

use config::AppConfig;
use llm::server::LlamaServer;
use models::ChatMessage;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tools::ToolRegistry;

pub struct AppState {
    /// Arc — 도구 실행 컨텍스트(ToolCtx)와 살아있는 설정을 공유한다
    pub config: Arc<Mutex<AppConfig>>,
    pub server: tokio::sync::Mutex<LlamaServer>,
    pub sessions: Mutex<HashMap<String, Vec<ChatMessage>>>,
    pub cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub registry: Arc<ToolRegistry>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            config: Arc::new(Mutex::new(AppConfig::load())),
            server: tokio::sync::Mutex::new(LlamaServer::new()),
            sessions: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            registry: Arc::new(ToolRegistry::with_default_tools()),
        })
        .setup(|app| {
            // 앱 시작과 동시에 모델 로드 (쿼리 경로에서 로드 시간 제거)
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::start_server_inner(&handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::list_models,
            commands::server_status,
            commands::restart_server,
            commands::new_session,
            commands::send_message,
            commands::capture_region,
            commands::cancel_turn,
            commands::pick_folder,
            commands::list_sessions,
            commands::load_session,
            commands::delete_session,
            commands::workspace_summary,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 앱 종료 시 llama-server 사이드카 정리 (Windows 는 자식 프로세스를 자동 종료하지 않음)
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                if let Ok(mut server) = state.server.try_lock() {
                    tauri::async_runtime::block_on(server.stop());
                };
            }
        });
}
