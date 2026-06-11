use crate::config::AppConfig;
use crate::llm::client::{DeltaKind, LlmClient};
use crate::models::{AgentEvent, ChatMessage};
use crate::tools::{ToolCtx, ToolRegistry};
use anyhow::Result;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// 시스템 프롬프트. prefill 비용을 위해 간결하게 유지한다.
/// 워크스페이스/페르소나가 살아있는 설정을 반영하도록 **매 턴** 재생성된다.
pub fn system_prompt(cfg: &AppConfig) -> String {
    let home = dirs::home_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M (%A)");
    // 도구 인자 규칙(슬래시)과 일치하는 표기로 경로 예시를 보여준다
    let ws = cfg.workspace_path().display().to_string().replace('\\', "/");
    let name = if cfg.agent_name.trim().is_empty() { "Local Agent".to_string() } else { cfg.agent_name.trim().to_string() };
    format!(
        "너는 사용자의 Windows PC에서 동작하는 로컬 에이전트 '{name}'다.\n\
         현재 시각: {now}.\n\n\
         현재 폴더(워크스페이스): {ws}\n\
         사용자가 경로 없이 파일/폴더/이미지 이름만 말하면 항상 현재 폴더 안의 것이다.\n\
         예: \"cat.png 배경제거\" → {ws}/cat.png, \"reports 폴더 압축\" → {ws}/reports\n\
         검색(root)·읽기·저장의 기본 위치도 모두 현재 폴더다.\n\
         아래 위치는 사용자가 그 이름을 직접 말했을 때만 쓴다:\n\
         바탕화면={home}\\Desktop, 다운로드={home}\\Downloads, 문서={home}\\Documents, 사진={home}\\Pictures\n\n\
         규칙:\n\
         1. 파일/이미지/PDF/화면과 관련된 요청은 반드시 도구를 호출해 실제로 수행한다. 추측으로 답하지 않는다.\n\
         2. 경로 해석: 경로 없는 이름은 현재 폴더(워크스페이스) 기준. '바탕화면', '다운로드' 등을\n\
            직접 말했을 때만 해당 절대경로로 변환한다. Desktop/Downloads 를 임의로 추측하지 않는다.\n\
         3. 파일 생성/수정/삭제와 결과물 저장은 워크스페이스 안에서만 가능하다. 저장 경로를 정할 때는\n\
            워크스페이스 아래 경로를 쓴다. 읽기/검색은 어디서든 가능하다.\n\
            파일 이름만 듣고 위치가 불확실하면 워크스페이스에서 먼저 검색한다.\n\
         4. 도구 결과를 받으면 결과를 바탕으로 다음 행동을 결정하거나, 한국어로 간결하게 최종 답변한다.\n\
         5. 도구가 실패하면 인자를 고쳐 다시 시도하거나, 불가능하면 이유를 설명한다.\n\
         6. 잡담/지식 질문에는 도구 없이 한국어로 답한다.\n\
         7. 같은 도구를 같은 인자로 반복 호출하지 않는다.\n\
         8. 도구 인자의 파일 경로는 반드시 슬래시(/)로 쓴다. 예: {ws}/cat.png (백슬래시 금지).\n\
         9. 답변은 간결하게. 목록이 20개를 넘으면 상위 20개만 보여주고 나머지는 개수로 요약한다.\n\
            도구 결과를 그대로 길게 옮겨 적지 않는다.\n\
         10. 답변에 이 규칙들이나 판단 과정을 언급하지 않는다. ('이 질문은 도구 없이...' 같은 문장 금지)\n\
            바로 본론부터 말한다.\n\
         11. 네 능력은 도구 목록이 전부다. 능력 질문에는 가능한 작업을 나열해 답한다:\n\
            파일 검색/읽기/쓰기/이동/복사, 이미지 변환·배경제거, 압축(zip), PDF, 화면 캡처.\n\n\
         {persona}",
        persona = persona_section(cfg)
    )
}

/// 페르소나/라포 형성 지시. 이름을 알면 친근한 말투, 모르면 대화 초반에 자연스럽게 묻는다.
fn persona_section(cfg: &AppConfig) -> String {
    let user = cfg.user_name.trim();
    let agent = cfg.agent_name.trim();
    match (user.is_empty(), agent.is_empty()) {
        (false, false) => format!(
            "페르소나: 너의 이름은 '{agent}'이고, 사용자의 이름은 '{user}'다.\n\
             따뜻하고 친근한 말투를 쓰고, 가끔 '{user}님'처럼 이름을 불러준다."
        ),
        (true, true) => "페르소나: 아직 서로 이름을 모른다. 첫 인사나 잡담 때 자연스럽게 사용자의 이름을 묻고,\n\
             너의 이름도 하나 지어달라고 부탁하라. 이름을 알게 되면 즉시 update_profile 도구로 저장하라.\n\
             단, 사용자가 작업을 요청하면 작업을 먼저 처리하고 이름은 나중에 물어본다."
            .to_string(),
        (true, false) => format!(
            "페르소나: 너의 이름은 '{agent}'다. 아직 사용자의 이름을 모르니 대화 초반에 자연스럽게 묻고,\n\
             알게 되면 즉시 update_profile 도구로 저장하라. 따뜻하고 친근한 말투를 쓴다."
        ),
        (false, true) => format!(
            "페르소나: 사용자의 이름은 '{user}'다. 아직 너의 이름이 없으니 사용자에게 지어달라고 부탁하고,\n\
             정해지면 즉시 update_profile 도구로 저장하라. 따뜻하고 친근한 말투를 쓴다."
        ),
    }
}

/// 턴 단위 도구 라우팅: 사용자 발화에서 의도가 명확한 키워드가 보이면
/// 그 턴 동안 *경쟁* 도구를 숨긴다.
///
/// 배경: Qwen3.5-2B 는 '배경제거/누끼' 복합명사를 remove_background 설명과 매칭하지 못하고
/// image_transform(회전/리사이즈)을 호출하는 강한 편향이 있다. 설명 키워드 보강, 시스템 프롬프트
/// 힌트, few-shot, tool_choice 강제(서버가 미지원)까지 모두 실패 — 경쟁 도구를 목록에서 제거하는
/// 것만이 결정적으로 동작했다 (2026-06-11 라이브 서버 replay 실험, alian tool_domain 패턴).
pub fn tools_to_exclude(user_text: &str) -> Vec<&'static str> {
    const BG_KEYWORDS: &[&str] = &[
        "배경제거", "배경 제거", "배경을 제거", "누끼",
        "배경 빼", "배경을 빼", "배경 없애", "배경을 없애", "배경 지워", "배경을 지워",
    ];
    if BG_KEYWORDS.iter().any(|k| user_text.contains(k)) {
        // 배경제거 의도가 확실 — 회전/리사이즈로 새는 경로를 차단한다.
        // image_info 는 제외하지 않는다: 모델이 '확인 후 배경제거' 2단계로 쓰는 정상 경로
        // (2026-06-11 GT 단발 평가에서 image_info 선택은 누수가 아니라 선조회 패턴으로 확인됨)
        return vec!["image_transform"];
    }
    if is_dictation_write(user_text) {
        // 받아쓰기 쓰기 의도 — 읽기/탐색으로 새는 경로를 차단한다.
        // write_file 설명 보강만으로는 교정 실패(GT 0/5), remove_background 와 동일하게 제외만 동작.
        return vec!["read_file", "list_dir", "search_files", "pdf_extract_text", "move_path", "copy_path", "delete_path"];
    }
    vec![]
}

/// "X에 '...'라고 적어줘" 처럼 사용자가 내용을 그대로 불러주는 쓰기 의도인가?
/// 복합 작업("읽고 요약해서 저장해줘")을 오인하지 않도록, 인용부호 안의 내용은
/// 판정에서 제외하고 읽기 단서가 보이면 라우팅하지 않는다 (멀티스텝 보호).
fn is_dictation_write(user_text: &str) -> bool {
    const WRITE_VERBS: &[&str] = &["적어", "저장", "기록", "작성", "메모", "써"];
    const READ_HINTS: &[&str] = &[
        "읽", "요약", "번역", "분석", "정리", "내용", "보여", "뭐", "무엇", "어떤",
    ];
    let t = strip_quoted(user_text);
    t.contains("라고")
        && WRITE_VERBS.iter().any(|v| t.contains(v))
        && !READ_HINTS.iter().any(|h| t.contains(h))
}

/// 인용부호('...', "...", “...”, ‘...’) 안의 구간을 제거한다 — 불러주는 내용에
/// '요약' 같은 단어가 들어 있어도 의도 판정이 흔들리지 않게.
fn strip_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut closing: Option<char> = None;
    for ch in s.chars() {
        match closing {
            Some(q) if ch == q => closing = None,
            Some(_) => {}
            None => match ch {
                '\'' | '"' => closing = Some(ch),
                '“' => closing = Some('”'),
                '‘' => closing = Some('’'),
                _ => out.push(ch),
            },
        }
    }
    out
}

/// 오래된 턴을 접어 넣는 요약 섹션의 식별 마커. Qwen 챗 템플릿은 system 메시지를
/// 맨 앞 1개만 허용하므로(실서버 검증: 중간 system 은 "System message must be at the
/// beginning" 400) 별도 메시지가 아니라 시스템 프롬프트 본문 끝에 섹션으로 붙인다.
pub const DIGEST_MARKER: &str = "[이전 대화 요약]";

/// 멀티턴에서 원문 그대로 보존하는 최근 사용자 턴 수 (이번 턴 포함)
const KEEP_RECENT_TURNS: usize = 3;

/// 요약 메시지 자체가 비대해지지 않도록 거는 상한 (오래된 줄부터 버림)
const DIGEST_MAX_CHARS: usize = 1600;

/// 진행 없는 라운드(모든 호출이 중복/차단/깨진 인자로 거절)를 이 횟수만큼 연속 만나면
/// 도구를 떼고 최종 답변을 강제한다. 2B 는 거절 피드백이 이력에 쌓일수록 같은 호출을
/// 더 강하게 베끼는 자기강화 루프에 빠진다 — 실로그: write_file 동일 호출 7회 반복으로
/// 라운드 예산(8회)을 다 태우고 "한도 초과" 실패로 끝난 턴이 다수 (2026-06-11 저녁).
const MAX_REJECTED_ROUNDS: u32 = 2;

/// 히스토리에 허용하는 문자 예산. 한국어는 글자당 1~1.5 토큰이라 chars≈tokens 로 보고,
/// ctx 에서 도구 스키마(~4K)+시스템 프롬프트+출력 예약(~2K)을 뺀 값을 쓴다.
/// 영문 위주 도구 결과는 글자당 토큰이 더 적어 보수적(안전) 방향으로만 틀린다.
pub fn history_budget_chars(ctx_size: u32) -> usize {
    (ctx_size as usize).saturating_sub(6000).max(4000)
}

fn message_chars(m: &ChatMessage) -> usize {
    m.content.as_deref().map(|c| c.chars().count()).unwrap_or(0)
        + m.tool_calls
            .iter()
            .flatten()
            .map(|c| c.function.name.len() + c.function.arguments.chars().count())
            .sum::<usize>()
}

fn total_chars(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_chars).sum()
}

/// 매 턴 시작 전 히스토리를 예산 안으로 맞춘다 (작은 모델의 맥락 유지 전략):
/// ① 오래된 도구 결과를 짧게 축약하고,
/// ② 그래도 넘치면 가장 오래된 턴부터 한 줄 요약으로 접어 DIGEST 메시지에 흡수한다.
/// 최근 KEEP_RECENT_TURNS 턴은 항상 원문 보존 — 직전 맥락("그거", "방금 그 파일")이
/// 2B 모델이 실제로 활용할 수 있는 맥락이고, 오래된 턴은 요약된 실마리만 남긴다.
/// LLM 호출 없이 기계적으로 요약한다 (20 t/s 하드웨어에서 요약 호출은 턴당 +10초).
pub fn enforce_history_budget(messages: &mut Vec<ChatMessage>, budget_chars: usize) {
    if total_chars(messages) <= budget_chars {
        return;
    }
    compact_old_tool_results(messages);

    while total_chars(messages) > budget_chars {
        let user_idxs: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == "user")
            .map(|(i, _)| i)
            .collect();
        if user_idxs.len() <= KEEP_RECENT_TURNS {
            break; // 최근 턴만 남음 — 더 접지 않는다 (초과 시 run_turn 의 압축 재시도가 백스톱)
        }
        let (start, end) = (user_idxs[0], user_idxs[1]);
        let line = digest_line(&messages[start..end]);
        messages.drain(start..end);
        append_digest_line(messages, &line);
    }
}

/// 한 턴(user~다음 user 직전)을 "- 질문 → 답" 한 줄로 요약
fn digest_line(turn: &[ChatMessage]) -> String {
    let user = turn
        .first()
        .and_then(|m| m.content.as_deref())
        .unwrap_or("");
    let answer = turn
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && m.content.as_deref().is_some_and(|c| !c.is_empty()))
        .and_then(|m| m.content.as_deref())
        .unwrap_or("(답 없음)");
    let one_line = |s: &str, max: usize| -> String {
        let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
        clip(&collapsed, max)
    };
    format!("- 사용자: {} → 답: {}", one_line(user, 60), one_line(answer, 80))
}

/// 시스템 프롬프트(messages[0]) 끝의 요약 섹션에 줄을 추가한다. 섹션이 없으면 만들고,
/// 상한을 넘으면 오래된 줄부터 버린다.
fn append_digest_line(messages: &mut Vec<ChatMessage>, line: &str) {
    if messages.first().map(|m| m.role != "system").unwrap_or(true) {
        messages.insert(0, ChatMessage::system(String::new()));
    }
    let content = messages[0].content.take().unwrap_or_default();
    let (base, mut lines) = split_digest(&content);
    lines.push(line.to_string());
    // 상한 초과 시 오래된 줄부터 버린다
    while lines.len() > 1
        && lines.iter().map(|l| l.chars().count() + 1).sum::<usize>() > DIGEST_MAX_CHARS
    {
        lines.remove(0);
    }
    let sep = if base.is_empty() { "" } else { "\n\n" };
    messages[0].content = Some(format!("{base}{sep}{DIGEST_MARKER}\n{}", lines.join("\n")));
}

/// 시스템 프롬프트 본문을 (기본 프롬프트, 요약 줄들) 로 분해한다
fn split_digest(content: &str) -> (String, Vec<String>) {
    match content.find(DIGEST_MARKER) {
        Some(pos) => {
            let base = content[..pos].trim_end().to_string();
            let lines = content[pos..].lines().skip(1).map(String::from).collect();
            (base, lines)
        }
        None => (content.to_string(), vec![]),
    }
}

/// 워크스페이스/페르소나/시각이 살아있도록 시스템 프롬프트를 매 턴 재생성하되,
/// 히스토리 예산이 접어 넣은 요약 섹션은 보존한다.
pub fn refresh_system_prompt(messages: &mut [ChatMessage], cfg: &AppConfig) {
    let Some(first) = messages.first_mut() else { return };
    if first.role != "system" {
        return;
    }
    let (_, lines) = split_digest(first.content.as_deref().unwrap_or(""));
    let mut prompt = system_prompt(cfg);
    if !lines.is_empty() {
        prompt = format!("{prompt}\n\n{DIGEST_MARKER}\n{}", lines.join("\n"));
    }
    *first = ChatMessage::system(prompt);
}

/// 턴이 중단(라운드 한도/취소)되어 마지막 assistant 의 tool_calls 에 결과가 없으면
/// 합성 결과를 붙여 이력을 봉합한다. 미응답 툴콜이 남으면 다음 턴에서 모델이
/// 이전 작업을 마저 하느라 새 질문을 무시하는 "한 턴 밀림"이 생긴다 (실로그에서 확인).
fn close_dangling_tool_calls(messages: &mut Vec<ChatMessage>) {
    let Some(last_assistant) = messages.iter().rposition(|m| m.role == "assistant") else {
        return;
    };
    let answered: std::collections::HashSet<String> = messages[last_assistant..]
        .iter()
        .filter(|m| m.role == "tool")
        .filter_map(|m| m.tool_call_id.clone())
        .collect();
    let unanswered: Vec<String> = messages[last_assistant]
        .tool_calls
        .iter()
        .flatten()
        .map(|c| c.id.clone())
        .filter(|id| !answered.contains(id))
        .collect();
    for id in unanswered {
        messages.push(ChatMessage::tool(
            id,
            "(턴이 중단되어 실행되지 않음. 이 작업은 잊고 사용자의 다음 요청에 답하라.)",
        ));
    }
}

/// 사용자 발화 1회를 처리하는 에이전트 루프.
/// 메시지 히스토리를 직접 갱신하며, 진행 상황을 emit 으로 흘린다.
pub async fn run_turn(
    client: &dyn LlmClient,
    registry: &ToolRegistry,
    tool_ctx: &ToolCtx,
    messages: &mut Vec<ChatMessage>,
    session_id: &str,
    max_tool_rounds: u32,
    temperature: f32,
    cancel: &AtomicBool,
    emit: &(dyn Fn(AgentEvent) + Send + Sync),
) -> Result<()> {
    let started = Instant::now();
    // 이번 턴 사용자 발화 기준으로 경쟁 도구를 숨긴다 (작은 모델 도구 선택 보정)
    let user_text = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.clone())
        .unwrap_or_default();
    let excluded = tools_to_exclude(&user_text);
    let tools = registry.schemas_excluding(&excluded);
    // 같은 (도구, 인자) 반복 차단 — 작은 모델의 루프 + 컨텍스트 낭비 방지
    let mut executed: std::collections::HashSet<(String, String)> = Default::default();
    // 빈 완성(사고만 하다 종료)은 샘플링 재시도로 한 번 회복을 시도한다
    let mut empty_retry_left = 1u32;
    // 연속으로 "실행이 하나도 없었던" 라운드 수 — MAX_REJECTED_ROUNDS 에서 강제 마무리
    let mut rejected_rounds = 0u32;

    for round in 0..=max_tool_rounds {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let mut sink = |kind: DeltaKind, text: &str| -> bool {
            let ev = match kind {
                DeltaKind::Thinking => AgentEvent::ThinkingDelta {
                    session_id: session_id.to_string(),
                    delta: text.to_string(),
                },
                DeltaKind::Text => AgentEvent::TextDelta {
                    session_id: session_id.to_string(),
                    delta: text.to_string(),
                },
            };
            emit(ev);
            // false 반환 시 클라이언트가 스트림을 끊는다 — 생성 중에도 ■ 버튼이 즉시 듣게
            !cancel.load(Ordering::Relaxed)
        };
        let mut result = match client.complete(messages, &tools, temperature, &mut sink).await {
            Ok(r) => r,
            // 컨텍스트 초과: 오래된 도구 결과를 압축하고 한 번 더 시도
            Err(e) if e.to_string().contains("exceed") && e.to_string().contains("context") => {
                let compacted = compact_old_tool_results(messages);
                if compacted == 0 {
                    return Err(e);
                }
                client.complete(messages, &tools, temperature, &mut sink).await?
            }
            Err(e) => return Err(e),
        };

        // 모델이 인자 JSON 을 완성하지 못한 툴콜(미종결 문자열 등)을 그대로 이력에 쌓으면
        // 이후 모든 요청에서 서버의 챗 템플릿 렌더링이 깨져 세션이 회복 불능이 된다
        // (2026-06-11 사고: 출력 한도까지 반복 → 따옴표 미종결 → 매 요청 500).
        // 인자를 빈 객체로 치환해 이력을 항상 유효하게 유지하고, 모델에는 오류로 알린다.
        let mut malformed_calls: std::collections::HashSet<String> = Default::default();
        for call in &mut result.tool_calls {
            if serde_json::from_str::<Value>(&call.function.arguments).is_err() {
                malformed_calls.insert(call.id.clone());
                call.function.arguments = "{}".into();
            }
        }

        // 사고만 하다 토큰을 소진하면 본문도 툴콜도 없다 — 재생성 1회 후에도 비면 알린다
        if result.content.is_empty() && result.tool_calls.is_empty() {
            if empty_retry_left > 0 {
                empty_retry_left -= 1;
                continue;
            }
            emit(AgentEvent::Error {
                session_id: session_id.to_string(),
                message: "모델이 응답을 완성하지 못했습니다 (출력 한도 내 사고 초과). 질문을 더 구체적으로 하거나 설정에서 출력 토큰을 늘려보세요.".into(),
            });
            break;
        }

        let content = if result.content.is_empty() { None } else { Some(result.content.clone()) };
        let tool_calls = if result.tool_calls.is_empty() { None } else { Some(result.tool_calls.clone()) };
        messages.push(ChatMessage::assistant(content, tool_calls));

        if result.tool_calls.is_empty() {
            break;
        }
        if round == max_tool_rounds {
            // 라운드 소진: 도구 결과 없이 종료를 알린다
            emit(AgentEvent::Error {
                session_id: session_id.to_string(),
                message: format!("도구 호출 한도({max_tool_rounds}회) 초과로 중단"),
            });
            break;
        }

        let mut any_real_execution = false;
        for call in &result.tool_calls {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            emit(AgentEvent::ToolCallStart {
                session_id: session_id.to_string(),
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                arguments: call.function.arguments.clone(),
            });

            let key = (call.function.name.clone(), call.function.arguments.trim().to_string());
            let (ok, text) = if malformed_calls.contains(&call.id) {
                (false, "오류: 도구 인자가 유효한 JSON이 아니어서 실행하지 않았습니다. \
                         인자를 짧고 정확하게 다시 작성하세요 (경로는 슬래시 사용).".to_string())
            } else if excluded.contains(&call.function.name.as_str()) {
                // 스키마에서 숨겨도 모델이 이전 턴의 호출을 이력에서 베껴 쓸 수 있다
                // (실로그: "배경제거 해봐" → 직전 턴의 image_transform 회전을 그대로 반복).
                // 실행 단계에서도 막아야 라우팅이 완성된다.
                (false, format!(
                    "오류: '{}' 도구는 이번 요청에 사용할 수 없습니다. 제공된 도구 목록에서 다시 선택하세요.",
                    call.function.name
                ))
            } else if !executed.insert(key) {
                (false, "이미 같은 인자로 호출한 도구입니다. 위의 기존 결과를 사용하거나 다른 행동을 취하세요.".to_string())
            } else {
                any_real_execution = true;
                let args: Value = serde_json::from_str(&call.function.arguments)
                    .unwrap_or(Value::Object(Default::default()));
                // 도구는 동기 구현 — 블로킹 실행을 런타임에 알린다
                let output = tokio::task::block_in_place(|| {
                    registry.execute(&call.function.name, &args, tool_ctx)
                });
                match output {
                    Ok(t) => (true, t),
                    Err(e) => (false, format!("오류: {e:#}")),
                }
            };
            emit(AgentEvent::ToolCallEnd {
                session_id: session_id.to_string(),
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                ok,
                result: clip(&text, 2000),
            });
            messages.push(ChatMessage::tool(call.id.clone(), clip(&text, 4000)));
        }

        // 진행 없는 라운드 연속 감지 → 도구를 떼고 지금까지의 결과로 최종 답변을 강제.
        // (거절 피드백만으로는 2B 가 루프를 못 벗어난다 — MAX_REJECTED_ROUNDS 주석 참고)
        if any_real_execution {
            rejected_rounds = 0;
        } else {
            rejected_rounds += 1;
        }
        if rejected_rounds >= MAX_REJECTED_ROUNDS && !cancel.load(Ordering::Relaxed) {
            let no_tools = Value::Array(vec![]);
            let final_result = client.complete(messages, &no_tools, temperature, &mut sink).await?;
            if final_result.content.is_empty() {
                emit(AgentEvent::Error {
                    session_id: session_id.to_string(),
                    message: "같은 도구 호출이 반복되어 작업을 중단했습니다. 요청을 바꿔 다시 시도해보세요.".into(),
                });
            } else {
                messages.push(ChatMessage::assistant(Some(final_result.content), None));
            }
            break;
        }
    }

    close_dangling_tool_calls(messages);
    emit(AgentEvent::TurnEnd {
        session_id: session_id.to_string(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
    Ok(())
}

fn clip(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}\n...(잘림)")
}

/// 마지막 2개를 제외한 도구 결과 메시지를 짧게 압축한다. 압축한 개수를 돌려준다.
/// (컨텍스트 초과 회복용 — 최근 결과는 모델이 아직 참조 중일 수 있어 보존)
fn compact_old_tool_results(messages: &mut [ChatMessage]) -> usize {
    const KEEP_RECENT: usize = 2;
    const COMPACT_TO: usize = 300;
    let tool_idxs: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "tool")
        .map(|(i, _)| i)
        .collect();
    let mut compacted = 0;
    for &i in tool_idxs.iter().rev().skip(KEEP_RECENT) {
        if let Some(content) = &messages[i].content {
            if content.chars().count() > COMPACT_TO {
                messages[i].content = Some(format!(
                    "{}\n...(컨텍스트 절약을 위해 축약됨)",
                    content.chars().take(COMPACT_TO).collect::<String>()
                ));
                compacted += 1;
            }
        }
    }
    compacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::client::DeltaSink;
    use crate::models::{CompletionResult, FunctionCall, ToolCall};
    use std::sync::Mutex;

    /// 호출 순서대로 미리 준비한 응답(또는 오류)을 돌려주는 mock.
    /// 호출마다 받은 도구 스키마 개수를 기록한다 (강제 마무리의 무도구 호출 검증용).
    struct MockClient {
        responses: Mutex<Vec<Result<CompletionResult>>>,
        tool_counts: Mutex<Vec<usize>>,
    }

    impl MockClient {
        fn ok(responses: Vec<CompletionResult>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(Ok).collect()),
                tool_counts: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for MockClient {
        async fn complete(
            &self,
            _messages: &[ChatMessage],
            tools: &Value,
            _temperature: f32,
            sink: DeltaSink<'_>,
        ) -> Result<CompletionResult> {
            self.tool_counts
                .lock()
                .unwrap()
                .push(tools.as_array().map(|a| a.len()).unwrap_or(0));
            let r = self.responses.lock().unwrap().remove(0)?;
            if !r.content.is_empty() {
                sink(DeltaKind::Text, &r.content);
            }
            Ok(r)
        }
    }

    fn noop_ctx() -> ToolCtx {
        ToolCtx::noop(AppConfig::default())
    }

    fn tool_call_result(name: &str, args: Value) -> CompletionResult {
        CompletionResult {
            content: String::new(),
            reasoning: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_0".into(),
                call_type: "function".into(),
                function: FunctionCall { name: name.into(), arguments: args.to_string() },
            }],
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loop_executes_tool_then_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "안녕하세요").unwrap();

        let client = MockClient::ok(vec![
            tool_call_result("read_file", serde_json::json!({"path": file.to_string_lossy()})),
            CompletionResult { content: "파일 내용은 '안녕하세요' 입니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![
            ChatMessage::system(system_prompt(&AppConfig::default())),
            ChatMessage::user("hello.txt 읽어줘"),
        ];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(
            &client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel,
            &|ev| events.lock().unwrap().push(ev),
        )
        .await
        .unwrap();

        // assistant(tool_call) + tool + assistant(final) 이 히스토리에 쌓였는지
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[2].role, "assistant");
        assert_eq!(messages[3].role, "tool");
        assert!(messages[3].content.as_deref().unwrap().contains("안녕하세요"));
        assert_eq!(messages[4].role, "assistant");

        let evs = events.lock().unwrap();
        let kinds: Vec<&str> = evs
            .iter()
            .map(|e| match e {
                AgentEvent::ToolCallStart { .. } => "tool-start",
                AgentEvent::ToolCallEnd { ok: true, .. } => "tool-ok",
                AgentEvent::ToolCallEnd { ok: false, .. } => "tool-err",
                AgentEvent::TextDelta { .. } => "text",
                AgentEvent::TurnEnd { .. } => "end",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["tool-start", "tool-ok", "text", "end"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tool_failure_is_fed_back_to_model() {
        let client = MockClient::ok(vec![
            tool_call_result("read_file", serde_json::json!({"path": "C:\\없는파일.txt"})),
            CompletionResult { content: "파일을 찾을 수 없습니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("읽어줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        assert!(messages.iter().any(|m| m.role == "tool" && m.content.as_deref().unwrap().starts_with("오류:")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn round_limit_stops_infinite_tool_loop() {
        // 항상 도구를 호출하는 모델 — 한도에서 끊겨야 한다
        let responses: Vec<CompletionResult> = (0..10)
            .map(|_| tool_call_result("list_dir", serde_json::json!({"path": "C:\\"})))
            .collect();
        let client = MockClient::ok(responses);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("loop")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 2, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
    }

    /// 같은 (도구, 인자) 재호출은 실행하지 않고 모델에 안내 메시지를 돌려준다
    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_tool_call_is_short_circuited() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.txt"), "x").unwrap();
        let args = serde_json::json!({"path": dir.path().to_string_lossy()});

        let client = MockClient::ok(vec![
            tool_call_result("list_dir", args.clone()),
            tool_call_result("list_dir", args.clone()), // 동일 호출 반복
            CompletionResult { content: "끝".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("목록")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        let oks = evs.iter().filter(|e| matches!(e, AgentEvent::ToolCallEnd { ok: true, .. })).count();
        let errs = evs.iter().filter(|e| matches!(e, AgentEvent::ToolCallEnd { ok: false, .. })).count();
        assert_eq!((oks, errs), (1, 1), "두 번째 호출은 실행 없이 거부돼야 함");
        assert!(messages.iter().any(|m| m.role == "tool"
            && m.content.as_deref().unwrap().contains("이미 같은 인자")));
    }

    /// 같은 호출만 반복하는 라운드가 연속되면 라운드 예산을 태우지 않고
    /// 도구를 뗀 마무리 호출로 최종 답변을 강제한다.
    /// (회귀: 실로그에서 write_file 동일 호출 7회 반복 → 72초 소모 + 한도초과 실패)
    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_only_rounds_force_tool_free_finish() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.txt"), "x").unwrap();
        let args = serde_json::json!({"path": dir.path().to_string_lossy()});
        let client = MockClient::ok(vec![
            tool_call_result("list_dir", args.clone()),
            tool_call_result("list_dir", args.clone()), // 거절 1
            tool_call_result("list_dir", args.clone()), // 거절 2 → 강제 마무리
            CompletionResult { content: "목록 정리: x.txt 1개입니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("목록")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let counts = client.tool_counts.lock().unwrap().clone();
        assert_eq!(counts.len(), 4, "총 4회 호출 (라운드 한도 8을 안 태움)");
        assert!(counts[..3].iter().all(|&c| c > 0), "본 호출들엔 도구 스키마 제공");
        assert_eq!(counts[3], 0, "마무리 호출은 도구 없이 나가야 함");

        let evs = events.lock().unwrap();
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })), "오류 없이 답으로 종료");
        assert_eq!(messages.last().unwrap().content.as_deref(), Some("목록 정리: x.txt 1개입니다."));
    }

    /// 강제 마무리 호출조차 빈 완성이면 사용자에게 오류로 알린다
    #[tokio::test(flavor = "multi_thread")]
    async fn forced_finish_with_empty_content_surfaces_error() {
        let args = serde_json::json!({"path": "C:\\"});
        let responses: Vec<CompletionResult> =
            (0..4).map(|_| tool_call_result("list_dir", args.clone())).collect();
        let client = MockClient::ok(responses);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("loop")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::TurnEnd { .. })));
    }

    /// 미종결 JSON 인자 툴콜은 이력에 빈 객체로 치환 저장되고, 모델에 오류로 안내된다.
    /// (회귀: 깨진 인자가 이력에 남으면 이후 모든 요청의 템플릿 렌더링이 500으로 죽는다)
    #[tokio::test(flavor = "multi_thread")]
    async fn malformed_tool_args_are_sanitized_not_poisoning_history() {
        // 2026-06-11 사고 재현: 닫는 따옴표 없이 출력 한도에서 잘린 인자
        let broken_args = format!(r#"{{"output_path":"C:/x.pdf","paths":["C:\\{}"#, "á".repeat(100));
        let client = MockClient::ok(vec![
            CompletionResult {
                content: String::new(),
                reasoning: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call_0".into(),
                    call_type: "function".into(),
                    function: FunctionCall { name: "images_to_pdf".into(), arguments: broken_args },
                }],
            },
            CompletionResult { content: "인자를 다시 작성하겠습니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("이미지를 pdf로 묶어줘")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        // 이력의 모든 툴콜 인자는 유효한 JSON 이어야 한다 (다음 요청이 살아남는 조건)
        for m in &messages {
            for c in m.tool_calls.iter().flatten() {
                assert!(
                    serde_json::from_str::<Value>(&c.function.arguments).is_ok(),
                    "이력에 깨진 인자가 남음: {}",
                    c.function.arguments
                );
            }
        }
        // 도구는 실행되지 않고, 모델에 JSON 오류 피드백이 전달돼야 한다
        assert!(messages.iter().any(|m| m.role == "tool"
            && m.content.as_deref().unwrap().contains("유효한 JSON이 아니")));
        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::ToolCallEnd { ok: false, .. })));
    }

    /// 컨텍스트 초과 오류가 나면 오래된 도구 결과를 압축하고 재시도한다
    #[tokio::test(flavor = "multi_thread")]
    async fn context_overflow_compacts_and_retries() {
        let long = "가".repeat(5000);
        let client = MockClient {
            responses: Mutex::new(vec![
                Err(anyhow::anyhow!(
                    "llama-server 오류 400: request exceeds the available context size"
                )),
                Ok(CompletionResult { content: "회복됨".into(), ..Default::default() }),
            ]),
            tool_counts: Mutex::new(vec![]),
        };
        let registry = ToolRegistry::with_default_tools();
        // 압축 대상이 되도록 긴 도구 결과 3개를 히스토리에 심는다
        let mut messages = vec![
            ChatMessage::user("이전 질문"),
            ChatMessage::tool("c1", long.clone()),
            ChatMessage::tool("c2", long.clone()),
            ChatMessage::tool("c3", long.clone()),
            ChatMessage::user("다음 질문"),
        ];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .expect("압축 후 재시도로 회복해야 함");

        assert!(messages[1].content.as_deref().unwrap().contains("축약됨"), "가장 오래된 결과가 압축돼야 함");
        assert!(!messages[3].content.as_deref().unwrap().contains("축약됨"), "최근 2개는 보존");
        assert_eq!(messages.last().unwrap().content.as_deref(), Some("회복됨"));
    }

    /// 빈 완성은 1회 재생성으로 회복을 시도하고, 성공하면 오류 없이 끝난다
    #[tokio::test(flavor = "multi_thread")]
    async fn empty_completion_recovers_with_one_retry() {
        let client = MockClient::ok(vec![
            CompletionResult::default(), // 빈 완성
            CompletionResult { content: "회복된 답변".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("질문")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })), "재시도 성공 시 오류 없어야 함");
        assert_eq!(messages.last().unwrap().content.as_deref(), Some("회복된 답변"));
    }

    /// 재생성까지 비면 사용자에게 오류로 알린다
    #[tokio::test(flavor = "multi_thread")]
    async fn empty_completion_twice_surfaces_error() {
        let client = MockClient::ok(vec![CompletionResult::default(), CompletionResult::default()]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("질문")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::TurnEnd { .. })));
    }

    #[test]
    fn compact_skips_when_nothing_to_compact() {
        let mut messages = vec![ChatMessage::user("짧음"), ChatMessage::tool("c1", "짧은 결과")];
        assert_eq!(compact_old_tool_results(&mut messages), 0);
    }

    #[test]
    fn prompt_includes_workspace() {
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = r"C:\Users\EST\작업방".into();
        let p = system_prompt(&cfg);
        assert!(p.contains("작업방"), "워크스페이스 경로가 프롬프트에 없음");
        assert!(p.contains("워크스페이스 안에서만"));
    }

    /// 경로 없는 이름은 워크스페이스 기준이라는 해석 규칙이 프롬프트 상단에 있어야 한다
    #[test]
    fn prompt_defaults_bare_names_to_workspace() {
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = r"C:\Users\EST\작업방".into();
        let p = system_prompt(&cfg);
        assert!(
            p.contains("현재 폴더(워크스페이스): C:/Users/EST/작업방"),
            "워크스페이스가 슬래시 표기로 상단에 없음"
        );
        assert!(p.contains("경로 없이"), "경로 없는 이름의 기본 해석 규칙 누락");
        assert!(p.contains("직접 말했을 때만"), "홈 폴더는 조건부라는 규칙 누락");
        // 홈 폴더 나열이 워크스페이스 선언보다 뒤에 와야 한다 (앵커링 편향 방지)
        let ws_pos = p.find("현재 폴더(워크스페이스)").unwrap();
        let home_pos = p.find("바탕화면=").unwrap();
        assert!(ws_pos < home_pos, "워크스페이스가 홈 폴더 매핑보다 먼저 나와야 함");
    }

    #[test]
    fn prompt_asks_names_when_unknown() {
        let p = system_prompt(&AppConfig::default());
        assert!(p.contains("update_profile"), "이름 저장 도구 안내 없음");
        assert!(p.contains("지어달라고"), "이름 지어달라는 지시 없음");
    }

    #[test]
    fn prompt_uses_names_when_known() {
        let mut cfg = AppConfig::default();
        cfg.user_name = "태경".into();
        cfg.agent_name = "앨리".into();
        let p = system_prompt(&cfg);
        assert!(p.contains("'앨리'") && p.contains("'태경'"));
        assert!(!p.contains("지어달라고"), "이름이 있는데 또 묻게 함");
    }

    #[test]
    fn bg_keywords_exclude_image_transform() {
        assert_eq!(tools_to_exclude("dog.png를 배경제거 해봐"), vec!["image_transform"]);
        assert_eq!(tools_to_exclude("이 사진 누끼 따줘"), vec!["image_transform"]);
        assert_eq!(tools_to_exclude("배경을 빼서 투명하게"), vec!["image_transform"]);
        assert!(tools_to_exclude("dog.png를 90도 회전시켜줘").is_empty());
        assert!(tools_to_exclude("배경화면 바꿔줘").is_empty(), "배경화면은 배경제거가 아님");
    }

    /// 받아쓰기 쓰기 의도는 읽기/탐색 도구를 숨기고, 복합·읽기 질의는 건드리지 않는다
    #[test]
    fn dictation_write_excludes_read_tools() {
        let excluded = vec!["read_file", "list_dir", "search_files", "pdf_extract_text", "move_path", "copy_path", "delete_path"];
        // GT 실패 5건 전부 라우팅돼야 한다
        assert_eq!(tools_to_exclude("todo.md에 '장보기' 라고 적어줘"), excluded);
        assert_eq!(
            tools_to_exclude("minutes.txt에 \"회의 요약: 배포 일정 확정\"이라고 저장해줘"),
            excluded,
            "따옴표 안 '요약'은 읽기 단서로 치지 않는다"
        );
        assert_eq!(tools_to_exclude("contacts.csv에 \"이름,전화번호\"라고 기록해줘"), excluded);
        assert_eq!(tools_to_exclude("plan.md에 \"보고서 작성, 메일 회신\"이라고 작성해줘"), excluded);
        assert_eq!(tools_to_exclude("idea.txt에 \"신제품 마케팅 아이디어\"라고 적어줘"), excluded);

        // 멀티스텝(읽기→요약→쓰기)과 읽기 질의는 라우팅하면 안 된다
        assert!(tools_to_exclude("report.md를 읽고 요약해서 summary.md에 저장해줘").is_empty());
        assert!(tools_to_exclude("guide.md에 뭐라고 적혀 있어?").is_empty());
        assert!(tools_to_exclude("로그 내용을 정리해서 result.txt라고 저장해줘").is_empty());
        assert!(tools_to_exclude("todo.md 적힌 거 보여줘").is_empty());
    }

    #[test]
    fn strip_quoted_removes_only_quoted_spans() {
        assert_eq!(strip_quoted("a '인용' b"), "a  b");
        assert_eq!(strip_quoted("x \"요약: 내용\" 라고 적어줘"), "x  라고 적어줘");
        assert_eq!(strip_quoted("“스마트” 따옴표도 ‘처리’"), " 따옴표도 ");
        assert_eq!(strip_quoted("따옴표 없음"), "따옴표 없음");
    }

    #[test]
    fn schemas_excluding_hides_tool() {
        let registry = ToolRegistry::with_default_tools();
        let all = serde_json::to_string(&registry.schemas()).unwrap();
        assert!(all.contains("image_transform") && all.contains("remove_background"));
        let filtered =
            serde_json::to_string(&registry.schemas_excluding(&["image_transform"])).unwrap();
        assert!(!filtered.contains("\"image_transform\""));
        assert!(filtered.contains("remove_background"));
    }

    /// 능력 규칙(11)은 부정어 없이 긍정문으로만 작성돼야 한다.
    /// 2B는 규칙 속 "배경제거 ... 못 한다" 같은 토큰열을 그대로 베껴 거짓 진술을
    /// 만든다 — 부정문 규칙은 환각을 오히려 악화시킴 (2026-06-11 실서버 A/B: 1/3 → 3/3,
    /// 긍정문 나열은 유도질문 환각 0/5).
    #[test]
    fn capability_rule_is_positive_only() {
        let p = system_prompt(&AppConfig::default());
        let start = p.find("11.").expect("능력 규칙 존재");
        let end = p.find("페르소나").unwrap_or(p.len());
        let rule = &p[start..end];
        assert!(rule.contains("배경제거"), "가능 작업 나열에 배경제거 포함");
        for bad in ["못 ", "못하", "할 수 없", "불가능", "금지"] {
            assert!(!rule.contains(bad), "능력 규칙에 부정어가 들어가면 안 됨: {bad}");
        }
    }

    #[test]
    fn prompt_asks_only_missing_name() {
        let mut cfg = AppConfig::default();
        cfg.agent_name = "앨리".into();
        let p = system_prompt(&cfg);
        assert!(p.contains("'앨리'"));
        assert!(p.contains("사용자의 이름을 모르니"));
    }

    // ── 맥락 유지: 히스토리 예산 ────────────────────────────────────────────

    /// user/assistant 쌍 n 턴짜리 히스토리 (각 답변 길이 지정)
    fn history_with_turns(n: usize, answer_len: usize) -> Vec<ChatMessage> {
        let mut msgs = vec![ChatMessage::system("시스템 프롬프트")];
        for i in 0..n {
            msgs.push(ChatMessage::user(format!("질문{i}: 파일{i} 처리해줘")));
            msgs.push(ChatMessage::assistant(Some(format!("답변{i}: {}", "가".repeat(answer_len))), None));
        }
        msgs
    }

    #[test]
    fn budget_noop_when_under() {
        let mut msgs = history_with_turns(3, 50);
        let before = msgs.len();
        enforce_history_budget(&mut msgs, 100_000);
        assert_eq!(msgs.len(), before);
        assert!(!msgs[0].content.as_deref().unwrap().contains(DIGEST_MARKER));
    }

    #[test]
    fn budget_folds_oldest_turns_into_digest_keeping_recent() {
        let mut msgs = history_with_turns(6, 500); // 턴당 ~500자 × 6턴
        enforce_history_budget(&mut msgs, 2200); // 최근 3턴 + 여유만 들어가는 예산

        // 시스템 프롬프트 끝에 요약 섹션이 생기고, 접힌 턴의 실마리가 들어있다
        // (Qwen 템플릿은 system 메시지를 맨 앞 1개만 허용 — 별도 메시지 금지)
        let system_count = msgs.iter().filter(|m| m.role == "system").count();
        assert_eq!(system_count, 1, "system 메시지는 항상 1개여야 함");
        let prompt = msgs[0].content.as_deref().unwrap();
        assert!(prompt.starts_with("시스템 프롬프트"), "기본 프롬프트가 앞에 보존");
        assert!(prompt.contains(DIGEST_MARKER));
        assert!(prompt.contains("질문0"), "가장 오래된 턴이 요약에 있어야 함: {prompt}");

        // 최근 3개 사용자 턴은 원문 보존
        let users: Vec<&str> =
            msgs.iter().filter(|m| m.role == "user").map(|m| m.content.as_deref().unwrap()).collect();
        assert_eq!(users.len(), KEEP_RECENT_TURNS);
        assert!(users[0].starts_with("질문3"));
        assert!(users[2].starts_with("질문5"));
    }

    #[test]
    fn budget_never_folds_recent_turns_even_if_still_over() {
        let mut msgs = history_with_turns(3, 5000); // 최근 3턴만으로 이미 예산 초과
        enforce_history_budget(&mut msgs, 1000);
        let users = msgs.iter().filter(|m| m.role == "user").count();
        assert_eq!(users, 3, "최근 턴은 절대 접지 않는다");
    }

    #[test]
    fn budget_compacts_tool_results_before_folding() {
        // 오래된 긴 도구 결과만 축약해도 예산에 들어가는 경우 — 턴은 접지 않는다
        let mut msgs = vec![
            ChatMessage::system("시스템"),
            ChatMessage::user("질문0"),
            ChatMessage::tool("c0", "가".repeat(3000)),
            ChatMessage::assistant(Some("답0".into()), None),
            ChatMessage::user("질문1"),
            ChatMessage::tool("c1", "나".repeat(100)),
            ChatMessage::assistant(Some("답1".into()), None),
            ChatMessage::user("질문2"),
            ChatMessage::tool("c2", "다".repeat(100)),
            ChatMessage::assistant(Some("답2".into()), None),
            ChatMessage::user("질문3"),
        ];
        enforce_history_budget(&mut msgs, 1500);
        assert!(msgs[2].content.as_deref().unwrap().contains("축약됨"), "오래된 도구 결과가 먼저 축약");
        assert_eq!(msgs.iter().filter(|m| m.role == "user").count(), 4, "턴은 유지");
    }

    #[test]
    fn digest_accumulates_and_respects_cap() {
        let mut msgs = history_with_turns(40, 400);
        enforce_history_budget(&mut msgs, 1500);
        let prompt = msgs[0].content.as_deref().unwrap();
        let section = &prompt[prompt.find(DIGEST_MARKER).expect("요약 섹션 존재")..];
        let lines_chars: usize =
            section.lines().skip(1).map(|l| l.chars().count() + 1).sum();
        assert!(lines_chars <= DIGEST_MAX_CHARS, "요약 상한 준수: {lines_chars}");
        // 상한 때문에 가장 오래된 줄은 버려지고 비교적 최근에 접힌 줄이 남는다
        assert!(section.contains("질문3"), "마지막으로 접힌 턴은 남아야 함");
    }

    /// 시스템 프롬프트를 매 턴 재생성해도 요약 섹션은 보존돼야 한다
    #[test]
    fn refresh_system_prompt_preserves_digest() {
        let mut msgs = history_with_turns(6, 500);
        enforce_history_budget(&mut msgs, 2200);
        assert!(msgs[0].content.as_deref().unwrap().contains("질문0"));

        let mut cfg = AppConfig::default();
        cfg.workspace_dir = r"C:\Users\EST\작업방".into();
        refresh_system_prompt(&mut msgs, &cfg);

        let prompt = msgs[0].content.as_deref().unwrap();
        assert!(prompt.contains("작업방"), "새 설정이 반영된 프롬프트");
        assert!(prompt.contains(DIGEST_MARKER) && prompt.contains("질문0"), "요약 섹션 보존");
        // 요약이 없으면 마커도 붙지 않는다
        let mut fresh = vec![ChatMessage::system("프롬프트"), ChatMessage::user("질문")];
        refresh_system_prompt(&mut fresh, &cfg);
        assert!(!fresh[0].content.as_deref().unwrap().contains(DIGEST_MARKER));
    }

    // ── 맥락 유지: 미응답 툴콜 봉합 ─────────────────────────────────────────

    /// 라운드 한도로 중단된 턴: 마지막 assistant 의 tool_calls 에 합성 결과가 붙어
    /// 이력이 [assistant(tool_calls) → tool] 쌍으로 닫혀야 한다 ("한 턴 밀림" 회귀 방지)
    #[tokio::test(flavor = "multi_thread")]
    async fn round_limit_closes_dangling_tool_calls() {
        let responses: Vec<CompletionResult> = (0..5)
            .map(|i| tool_call_result("list_dir", serde_json::json!({"path": format!("C:\\{i}")})))
            .collect();
        let client = MockClient::ok(responses);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("loop")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 2, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        let answered: std::collections::HashSet<&str> = messages
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect();
        for m in &messages {
            for c in m.tool_calls.iter().flatten() {
                assert!(answered.contains(c.id.as_str()), "미응답 툴콜이 남음: {}", c.id);
            }
        }
        assert_eq!(messages.last().unwrap().role, "tool");
        assert!(messages.last().unwrap().content.as_deref().unwrap().contains("중단"));
    }

    // ── 맥락 유지: 제외 도구 실행 차단 ──────────────────────────────────────

    /// 턴 라우팅으로 스키마에서 제외한 도구는 (모델이 이력에서 베껴 호출해도) 실행되지 않는다
    #[tokio::test(flavor = "multi_thread")]
    async fn excluded_tool_call_is_blocked_at_execution() {
        let client = MockClient::ok(vec![
            tool_call_result("image_transform", serde_json::json!({"path": "C:/x.png", "rotate": 90})),
            CompletionResult { content: "다른 도구를 쓰겠습니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("dog.png를 배경제거 해봐")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        assert!(
            messages.iter().any(|m| m.role == "tool"
                && m.content.as_deref().unwrap().contains("사용할 수 없습니다")),
            "제외 도구 호출이 차단 메시지로 거부돼야 함"
        );
        let evs = events.lock().unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::ToolCallEnd { ok: false, .. })));
    }
}
