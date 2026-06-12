use crate::agent;
use crate::config::AppConfig;
use crate::llm::client::HttpLlmClient;
use crate::models::{AgentEvent, ChatMessage};
use crate::sessions::{SessionMeta, SessionStore};
use crate::AppState;
use base64::Engine as _;
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

#[derive(Debug, Serialize)]
pub struct CaptureResult {
    pub path: String,
    pub thumb_data_url: String,
    pub width: u32,
    pub height: u32,
}

/// 오버레이가 돌려주는 선택 영역. 좌표/크기는 오버레이 뷰포트(논리 px) 기준이며,
/// view_w/view_h(= 오버레이 innerWidth/innerHeight)로 물리 픽셀 비율을 환산한다.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RegionRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub view_w: f64,
    pub view_h: f64,
}

/// 주 모니터 전체를 캡처해 캐시에 저장하고, (경로, 전체 base64 data URL, 물리 폭/높이) 반환.
fn capture_full_to_cache(cache_dir: std::path::PathBuf) -> Result<(String, String, u32, u32), String> {
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("캐시 폴더 생성 실패: {e}"))?;
    let monitors = xcap::Monitor::all().map_err(|e| format!("모니터 조회 실패: {e}"))?;
    // index 0 = 주 모니터 (기존 screen_capture 도구와 동일 관례)
    let monitor = monitors.into_iter().next().ok_or("사용 가능한 모니터가 없습니다")?;
    let image = monitor.capture_image().map_err(|e| format!("화면 캡처 실패: {e}"))?;
    let (width, height) = (image.width(), image.height());

    let path = cache_dir.join(format!(
        "capture_full_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S_%3f")
    ));
    image.save(&path).map_err(|e| format!("캡처 저장 실패: {e}"))?;

    let bytes = std::fs::read(&path).map_err(|e| format!("캡처 읽기 실패: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((
        path.to_string_lossy().into_owned(),
        format!("data:image/png;base64,{b64}"),
        width,
        height,
    ))
}

/// 전체 캡처 이미지를 선택 영역으로 잘라 캐시에 저장하고, 썸네일과 함께 돌려준다.
fn crop_to_cache(
    full_path: String,
    phys_w: u32,
    phys_h: u32,
    rect: RegionRect,
) -> Result<CaptureResult, String> {
    let full = image::open(&full_path).map_err(|e| format!("캡처 로드 실패: {e}"))?;
    // 오버레이 뷰포트(논리 px) → 물리 픽셀 환산
    let sx = phys_w as f64 / rect.view_w.max(1.0);
    let sy = phys_h as f64 / rect.view_h.max(1.0);
    let x = (rect.x * sx).round().clamp(0.0, (phys_w.saturating_sub(1)) as f64) as u32;
    let y = (rect.y * sy).round().clamp(0.0, (phys_h.saturating_sub(1)) as f64) as u32;
    let w = (rect.w * sx).round().clamp(1.0, (phys_w - x) as f64) as u32;
    let h = (rect.h * sy).round().clamp(1.0, (phys_h - y) as f64) as u32;
    let cropped = full.crop_imm(x, y, w, h);

    let path = std::path::Path::new(&full_path).with_file_name(format!(
        "capture_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S_%3f")
    ));
    cropped.save(&path).map_err(|e| format!("크롭 저장 실패: {e}"))?;

    let thumb = cropped.thumbnail(320, 320);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("썸네일 인코딩 실패: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.get_ref());

    Ok(CaptureResult {
        path: path.to_string_lossy().into_owned(),
        thumb_data_url: format!("data:image/png;base64,{b64}"),
        width: w,
        height: h,
    })
}

/// 오버레이가 표시할 전체 스크린샷(base64 data URL)을 가져온다.
#[tauri::command]
pub fn region_get_image(state: State<'_, AppState>) -> Option<String> {
    state.region_image.lock().unwrap().clone()
}

/// 오버레이에서 영역 선택 완료 — capture_screenshot 의 대기를 깨운다.
#[tauri::command]
pub fn region_finish(state: State<'_, AppState>, rect: RegionRect) {
    if let Some(tx) = state.region_tx.lock().unwrap().take() {
        let _ = tx.send(Some(rect));
    }
}

/// 오버레이에서 취소(Esc/닫기) — capture_screenshot 이 None 을 받게 한다.
#[tauri::command]
pub fn region_cancel(state: State<'_, AppState>) {
    if let Some(tx) = state.region_tx.lock().unwrap().take() {
        let _ = tx.send(None);
    }
}

/// UI 주도 스크린샷: 앱 숨김 → 전체 캡처 → 전체 화면 오버레이에서 영역 드래그 →
/// 선택 영역만 크롭. 취소하면 Ok(None). 실패/취소와 무관하게 창은 반드시 복구된다.
#[tauri::command]
pub async fn capture_screenshot(app: AppHandle) -> Result<Option<CaptureResult>, String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("앱 캐시 경로 조회 실패: {e}"))?
        .join("captures");

    let main = app.get_webview_window("main");
    if let Some(w) = &main {
        let _ = w.hide();
    }
    // 창이 화면 프레임에서 빠지도록 짧게 대기
    tokio::time::sleep(std::time::Duration::from_millis(180)).await;

    let cap = tauri::async_runtime::spawn_blocking(move || capture_full_to_cache(cache_dir))
        .await
        .map_err(|e| format!("캡처 태스크 실패: {e}"))?;
    let (full_path, data_url, phys_w, phys_h) = match cap {
        Ok(v) => v,
        Err(e) => {
            if let Some(w) = &main {
                let _ = w.show();
                let _ = w.set_focus();
            }
            return Err(e);
        }
    };

    // 오버레이가 가져갈 전체 이미지 + 선택 결과를 받을 채널 준비
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<RegionRect>>();
    {
        let state = app.state::<AppState>();
        *state.region_image.lock().unwrap() = Some(data_url);
        *state.region_tx.lock().unwrap() = Some(tx);
    }

    // 전체 화면 오버레이 창 (불투명 — 캡처 이미지를 꽉 채워 얼어붙은 화면처럼 보임)
    let overlay = WebviewWindowBuilder::new(
        &app,
        "region-overlay",
        WebviewUrl::App("index.html?overlay=1".into()),
    )
    .title("영역 선택")
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .fullscreen(true)
    .build()
    .map_err(|e| format!("오버레이 생성 실패: {e}"))?;

    // 선택 또는 취소 대기 (최대 120초)
    let rect = match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
        Ok(Ok(r)) => r,
        _ => None, // 타임아웃 또는 채널 드롭 → 취소 취급
    };

    let _ = overlay.close();
    {
        let state = app.state::<AppState>();
        *state.region_image.lock().unwrap() = None;
        state.region_tx.lock().unwrap().take();
    }
    if let Some(w) = &main {
        let _ = w.show();
        let _ = w.set_focus();
    }

    let Some(rect) = rect else {
        return Ok(None); // 취소
    };

    let result = tauri::async_runtime::spawn_blocking(move || {
        crop_to_cache(full_path, phys_w, phys_h, rect)
    })
    .await
    .map_err(|e| format!("크롭 태스크 실패: {e}"))?;
    result.map(Some)
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
            || cfg.reasoning_budget != new_config.reasoning_budget
            || cfg.mmproj_path != new_config.mmproj_path;
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
pub async fn send_message(
    app: AppHandle,
    session_id: String,
    text: String,
    attachments: Vec<String>,
) -> Result<(), String> {
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
        history.push(if attachments.is_empty() {
            ChatMessage::user(text)
        } else {
            ChatMessage::user_with_images(text, attachments.clone())
        });
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
    let (max_rounds, temperature, max_output_tokens, vision_enabled) = {
        let cfg = state.config.lock().unwrap();
        let vision = crate::llm::server::resolve_mmproj(&cfg.model_path, &cfg.mmproj_path).is_some();
        (cfg.max_tool_rounds, cfg.temperature, cfg.max_output_tokens, vision)
    };
    let registry = state.registry.clone();

    let app2 = app.clone();
    let sid = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let client = HttpLlmClient::new(base_url, max_output_tokens, vision_enabled);
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

        let pre_len = messages.len() - 1; // 이번 턴 user 메시지부터 로그에 포함
        let started = std::time::Instant::now();
        let run = agent::run_turn(
            &client, &registry, &tool_ctx, &mut messages, &sid, max_rounds, temperature, &cancel,
            &emit,
        )
        .await;

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
