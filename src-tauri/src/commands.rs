use crate::agent;
use crate::config::AppConfig;
use crate::llm::client::HttpLlmClient;
use crate::models::{AgentEvent, ChatMessage};
use crate::sessions::{SessionMeta, SessionStore};
use crate::AppState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Serialize)]
pub struct ModelEntry {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
}

fn emit_event(app: &AppHandle, ev: AgentEvent) {
    let _ = app.emit("agent-event", &ev);
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
pub async fn set_config(app: AppHandle, new_config: AppConfig) -> Result<(), String> {
    let state = app.state::<AppState>();
    let restart_needed = {
        let mut cfg = state.config.lock().unwrap();
        let changed = cfg.model_path != new_config.model_path
            || cfg.server_exe != new_config.server_exe
            || cfg.port != new_config.port
            || cfg.device != new_config.device
            || cfg.n_gpu_layers != new_config.n_gpu_layers
            || cfg.ctx_size != new_config.ctx_size
            || cfg.reasoning_budget != new_config.reasoning_budget;
        *cfg = new_config.clone();
        cfg.save().map_err(|e| e.to_string())?;
        changed
    };
    // UI ↔ 에이전트 루프 동기화: 어디서 바뀌었든 같은 이벤트가 흐른다
    emit_event(&app, AgentEvent::ConfigChanged { config: new_config });
    if restart_needed {
        start_server_inner(&app).await?;
    }
    Ok(())
}

/// 네이티브 폴더 선택 다이얼로그. 취소 시 None.
#[tauri::command]
pub async fn pick_folder(initial_dir: Option<String>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_title("워크스페이스 폴더 선택");
        if let Some(dir) = initial_dir.filter(|d| std::path::Path::new(d).is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_folder().map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())
}

/// ~/.lmstudio/models 아래의 GGUF 목록 (mmproj 프로젝터 파일 제외)
#[tauri::command]
pub fn list_models() -> Vec<ModelEntry> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let root = home.join(".lmstudio").join("models");
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).max_depth(4).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.to_lowercase().ends_with(".gguf") || name.to_lowercase().contains("mmproj") {
            continue;
        }
        out.push(ModelEntry {
            name,
            path: entry.path().to_string_lossy().to_string(),
            size_bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[tauri::command]
pub async fn server_status(state: State<'_, AppState>) -> Result<String, String> {
    let server = state.server.lock().await;
    Ok(if server.is_healthy().await { "ready".into() } else { "down".into() })
}

/// llama-server 기동 (앱 시작/모델 변경 시). 상태를 이벤트로 알린다.
pub async fn start_server_inner(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let cfg = state.config.lock().unwrap().clone();
    let model_name = std::path::Path::new(&cfg.model_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    emit_event(app, AgentEvent::ServerStatus { status: "loading".into(), detail: model_name.clone() });
    let mut server = state.server.lock().await;
    match server.start(&cfg).await {
        Ok(()) => {
            emit_event(app, AgentEvent::ServerStatus { status: "ready".into(), detail: model_name });
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            emit_event(app, AgentEvent::ServerStatus { status: "down".into(), detail: msg.clone() });
            Err(msg)
        }
    }
}

#[tauri::command]
pub async fn restart_server(app: AppHandle) -> Result<(), String> {
    start_server_inner(&app).await
}

/// 로컬 검색 사이드카 기동 (앱 시작 시). 미구성/바이너리 없음이면 조용히 비활성한다.
/// 워크스페이스(사용자 지정 폴더)의 문서를 인덱싱한 뒤 사이드카(serve)를 띄운다.
/// 실패해도 앱 동작에는 지장 없으므로 에러를 전파하지 않는다(검색만 비활성).
pub async fn start_localsearch_inner(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (cfg, workspace) = {
        let c = state.config.lock().unwrap();
        (c.clone(), c.workspace_path())
    };
    let Some(lc) = crate::localsearch::LocalSearchConfig::from_app(&cfg) else {
        return; // 로컬 검색 미구성 → 검색 없이 동작
    };

    // 1) 워크스페이스 문서 인덱싱 — serve 전에 수행해 같은 DB 동시 접근을 피한다.
    //    (홈 전체 등 광범위 폴더는 가드로 건너뜀)
    if crate::localsearch::is_indexable_workspace(&workspace) {
        let lc2 = lc.clone();
        let ws = workspace.to_string_lossy().into_owned();
        match tokio::task::spawn_blocking(move || crate::localsearch::run_index(&lc2, &ws)).await {
            Ok(Ok(s)) => eprintln!("[localsearch] 워크스페이스 인덱싱: {s:?}"),
            Ok(Err(e)) => eprintln!("[localsearch] 워크스페이스 인덱싱 실패: {e:#}"),
            Err(e) => eprintln!("[localsearch] 인덱싱 태스크 조인 실패: {e}"),
        }
    }

    // 2) 사이드카(serve) 기동 → 검색 클라이언트 보관
    let mut server = state.localsearch.lock().await;
    match server.start(&lc).await {
        Ok(client) => {
            *state.search.lock().await = Some(client);
        }
        Err(e) => {
            eprintln!("localsearch 사이드카 기동 실패(검색 비활성): {e:#}");
        }
    }
}

#[tauri::command]
pub fn new_session(state: State<'_, AppState>) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let prompt = agent::system_prompt(&state.config.lock().unwrap());
    state
        .sessions
        .lock()
        .unwrap()
        .insert(id.clone(), vec![ChatMessage::system(prompt)]);
    id
}

/// 저장된 세션 목록 (최근 수정 순)
#[tauri::command]
pub fn list_sessions() -> Vec<SessionMeta> {
    SessionStore::open_default().list()
}

/// 저장된 세션을 메모리로 불러와 이어서 대화할 수 있게 하고, 복원용 이력을 돌려준다
#[tauri::command]
pub fn load_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<ChatMessage>, String> {
    let messages = SessionStore::open_default()
        .load(&session_id)
        .ok_or_else(|| format!("저장된 세션 없음: {session_id}"))?;
    state.sessions.lock().unwrap().insert(session_id, messages.clone());
    Ok(messages)
}

#[tauri::command]
pub fn delete_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    SessionStore::open_default().delete(&session_id).map_err(|e| e.to_string())?;
    state.sessions.lock().unwrap().remove(&session_id);
    Ok(())
}

#[tauri::command]
pub fn cancel_turn(state: State<'_, AppState>, session_id: String) {
    if let Some(flag) = state.cancels.lock().unwrap().get(&session_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// 사용자 발화 처리. 백그라운드 태스크로 에이전트 루프를 돌리고 즉시 반환한다.
#[tauri::command]
pub async fn send_message(app: AppHandle, session_id: String, text: String) -> Result<(), String> {
    let state = app.state::<AppState>();

    let mut messages = {
        let mut sessions = state.sessions.lock().unwrap();
        // 메모리에 없으면 디스크에서 복원 — 백엔드 재시작 후에도 기존 세션을 이어간다
        if !sessions.contains_key(&session_id) {
            if let Some(saved) = SessionStore::open_default().load(&session_id) {
                sessions.insert(session_id.clone(), saved);
            }
        }
        let history = sessions
            .get_mut(&session_id)
            .ok_or_else(|| format!("세션 없음: {session_id}"))?;
        history.push(ChatMessage::user(text.clone()));
        history.clone()
    };
    {
        let cfg = state.config.lock().unwrap();
        // 워크스페이스/페르소나/시각이 살아있도록 시스템 프롬프트를 매 턴 재생성
        // (예산 관리가 접어 넣은 [이전 대화 요약] 섹션은 보존된다)
        agent::refresh_system_prompt(&mut messages, &cfg);
        // 컨텍스트 예산: 오래된 턴은 요약으로 접고 최근 턴만 원문 유지 (작은 모델 맥락 전략)
        agent::enforce_history_budget(&mut messages, agent::history_budget_chars(cfg.ctx_size));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    state.cancels.lock().unwrap().insert(session_id.clone(), cancel.clone());

    let base_url = state.server.lock().await.base_url.clone();
    if base_url.is_empty() {
        return Err("LLM 서버가 아직 준비되지 않음".into());
    }
    let (max_rounds, temperature, max_output_tokens) = {
        let cfg = state.config.lock().unwrap();
        (cfg.max_tool_rounds, cfg.temperature, cfg.max_output_tokens)
    };
    let registry = state.registry.clone();

    let app2 = app.clone();
    let sid = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let client = HttpLlmClient::new(base_url, max_output_tokens);
        let app3 = app2.clone();
        // 턴 내에서 발생한 Error 이벤트도 대화 로그에 남도록 수집
        let turn_errors = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let errs = turn_errors.clone();
        let emit = move |ev: AgentEvent| {
            if let AgentEvent::Error { message, .. } = &ev {
                errs.lock().unwrap().push(message.clone());
            }
            emit_event(&app3, ev);
        };

        // 도구가 설정을 바꾸면(set_workspace) 저장하고 UI 로 방송한다
        let config = app2.state::<AppState>().config.clone();
        let notify_app = app2.clone();
        let tool_ctx = crate::tools::ToolCtx::new(
            config,
            Arc::new(|cfg: &AppConfig| cfg.save()),
            Arc::new(move |ev| emit_event(&notify_app, ev)),
        );

        // RAG 프리훅: 색인이 있으면 발화를 검색해 근거 블록을 system(messages[0]) 에 합친다.
        // 이번 턴 LLM 호출에만 쓰고 세션에는 저장하지 않으므로, run_turn 뒤 원복한다.
        let sys_backup = messages.first().and_then(|m| m.content.clone());
        let rag = match app2.state::<AppState>().search.lock().await.clone() {
            Some(client) => {
                client
                    .rag_context(&text, crate::localsearch::RAG_TOP_K, crate::localsearch::RAG_MIN_COSINE)
                    .await
            }
            None => None,
        };
        if let Some(rc) = &rag {
            // 출처를 UI 로 방송 (말풍선 하단 표시)
            emit(AgentEvent::Sources {
                session_id: sid.clone(),
                sources: rc.sources.clone(),
            });
            if let Some(sys0) = messages.first_mut() {
                sys0.content = Some(format!("{}\n\n{}", sys_backup.as_deref().unwrap_or(""), rc.block));
            }
        }

        let pre_len = messages.len() - 1; // 이번 턴 user 메시지부터 로그에 포함
        let started = std::time::Instant::now();
        let run = agent::run_turn(
            &client, &registry, &tool_ctx, &mut messages, &sid, max_rounds, temperature, &cancel,
            &emit,
        )
        .await;

        // RAG 근거 블록 원복 — 저장/세션에 스테일 컨텍스트가 남지 않도록
        if rag.is_some() {
            if let Some(sys0) = messages.first_mut() {
                sys0.content = sys_backup;
            }
        }

        let mut all_errors: Vec<String> = turn_errors.lock().unwrap().clone();
        if let Err(e) = run.as_ref() {
            all_errors.push(format!("{e:#}"));
        }
        let error_text = if all_errors.is_empty() { None } else { Some(all_errors.join(" | ")) };
        crate::logging::log_turn(
            &sid,
            &messages[pre_len..],
            started.elapsed().as_millis() as u64,
            error_text.as_deref(),
        );

        if let Err(e) = run {
            emit(AgentEvent::Error { session_id: sid.clone(), message: format!("{e:#}") });
            emit(AgentEvent::TurnEnd { session_id: sid.clone(), elapsed_ms: 0 });
        }

        // 턴마다 디스크에 영속화 — 새 대화/앱 재시작 후에도 목록에서 불러올 수 있다
        if let Err(e) = SessionStore::open_default().save(&sid, &messages) {
            eprintln!("세션 저장 실패({sid}): {e:#}");
        }
        let state = app2.state::<AppState>();
        state.sessions.lock().unwrap().insert(sid.clone(), messages);
        state.cancels.lock().unwrap().remove(&sid);
    });

    Ok(())
}
