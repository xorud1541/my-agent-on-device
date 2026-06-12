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

/// 전체 캡처 결과. data_url 은 모달에 표시할 **다운스케일 프리뷰**(가벼운 JPEG).
/// 실제 크롭은 path 의 원본(full 해상도)에서 수행한다.
#[derive(Debug, Serialize)]
pub struct FullCapture {
    pub path: String,
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

/// 크롭 영역. 좌표/크기는 표시 이미지에 대한 **정규화 비율(0.0~1.0)** 이라 프리뷰 해상도와 무관하다.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RegionRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 모달 프리뷰의 최대 변 길이. 전체 해상도 base64 를 IPC 로 보내던 병목을 제거하기 위함.
const PREVIEW_MAX: u32 = 1600;

/// 지정한 화면 좌표가 속한 모니터(현재 모니터)를 캡처해 캐시에 저장한다.
/// 원본 PNG 는 크롭용으로 디스크에 두고, 모달 표시용으로는 다운스케일 JPEG 프리뷰만 반환한다.
fn capture_to_cache(
    cache_dir: std::path::PathBuf,
    point: Option<(i32, i32)>,
) -> Result<FullCapture, String> {
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("캐시 폴더 생성 실패: {e}"))?;

    // 현재 모니터: 주어진 좌표가 속한 모니터, 못 찾으면 첫 모니터로 폴백.
    let monitor = match point.and_then(|(x, y)| xcap::Monitor::from_point(x, y).ok()) {
        Some(m) => m,
        None => xcap::Monitor::all()
            .map_err(|e| format!("모니터 조회 실패: {e}"))?
            .into_iter()
            .next()
            .ok_or("사용 가능한 모니터가 없습니다")?,
    };
    let image = monitor.capture_image().map_err(|e| format!("화면 캡처 실패: {e}"))?;
    let (width, height) = (image.width(), image.height());

    let path = cache_dir.join(format!(
        "capture_full_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S_%3f")
    ));
    image.save(&path).map_err(|e| format!("캡처 저장 실패: {e}"))?;

    // 프리뷰: 다운스케일 + JPEG → IPC 페이로드를 수 MB → 수백 KB 로 축소(속도 핵심).
    // 원본(path)은 그대로 두고 크롭은 거기서 하므로 화질 손실 없음.
    let dynimg = image::DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(width, height, image.into_raw())
            .ok_or("캡처 버퍼 변환 실패")?,
    );
    let preview = dynimg.thumbnail(PREVIEW_MAX, PREVIEW_MAX).to_rgb8();
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(preview)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .map_err(|e| format!("프리뷰 인코딩 실패: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.get_ref());

    Ok(FullCapture {
        path: path.to_string_lossy().into_owned(),
        data_url: format!("data:image/jpeg;base64,{b64}"),
        width,
        height,
    })
}

/// 전체 캡처 이미지를 선택 영역(정규화 비율)으로 잘라 캐시에 저장하고, 썸네일과 함께 돌려준다.
fn crop_to_cache(full_path: &str, rect: RegionRect) -> Result<CaptureResult, String> {
    let full = image::open(full_path).map_err(|e| format!("캡처 로드 실패: {e}"))?;
    let (pw, ph) = (full.width(), full.height());
    // 정규화(0~1) → 원본 픽셀. 프리뷰 해상도와 무관하게 정확.
    let clamp01 = |v: f64| v.clamp(0.0, 1.0);
    let x = (clamp01(rect.x) * pw as f64).round() as u32;
    let y = (clamp01(rect.y) * ph as f64).round() as u32;
    let w = ((clamp01(rect.w) * pw as f64).round() as u32).clamp(1, pw - x.min(pw - 1));
    let h = ((clamp01(rect.h) * ph as f64).round() as u32).clamp(1, ph - y.min(ph - 1));
    let cropped = full.crop_imm(x.min(pw - 1), y.min(ph - 1), w, h);

    let path = std::path::Path::new(full_path).with_file_name(format!(
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
        width: cropped.width(),
        height: cropped.height(),
    })
}

/// UI 주도 스크린샷: 앱 숨김 → **현재 모니터** 캡처 → 앱 복귀.
/// 영역 선택은 프론트의 앱 내 모달에서 처리하고, 선택 결과로 crop_capture 를 호출한다.
/// (두 번째 webview 창을 만들면 macOS WebKit 레이어트리 커밋에서 크래시 — 단일 창 유지가 핵심)
#[tauri::command]
pub async fn capture_screenshot(app: AppHandle) -> Result<FullCapture, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("앱 캐시 경로 조회 실패: {e}"))?
        .join("captures");

    let main = app.get_webview_window("main");
    // 현재 모니터 판정용 좌표: 앱 창이 놓인 모니터의 중심(물리 픽셀). 숨기기 전에 구한다.
    let point = main.as_ref().and_then(|w| {
        let mon = w.current_monitor().ok().flatten()?;
        let pos = mon.position();
        let size = mon.size();
        Some((
            pos.x + size.width as i32 / 2,
            pos.y + size.height as i32 / 2,
        ))
    });

    if let Some(w) = &main {
        let _ = w.hide();
    }
    // 창이 화면 프레임에서 빠지도록 짧게 대기 (최소화)
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    let result = tauri::async_runtime::spawn_blocking(move || capture_to_cache(cache_dir, point))
        .await
        .map_err(|e| format!("캡처 태스크 실패: {e}"))?;

    // 성공/실패와 무관하게 창 복구
    if let Some(w) = &main {
        let _ = w.show();
        let _ = w.set_focus();
    }
    result
}

/// 캡처 원본을 선택 영역으로 잘라 첨부용 결과를 돌려준다 (앱 내 모달에서 호출).
#[tauri::command]
pub async fn crop_capture(full_path: String, rect: RegionRect) -> Result<CaptureResult, String> {
    tauri::async_runtime::spawn_blocking(move || crop_to_cache(&full_path, rect))
        .await
        .map_err(|e| format!("크롭 태스크 실패: {e}"))?
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
