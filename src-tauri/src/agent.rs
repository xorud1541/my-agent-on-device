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
            파일 검색/읽기/쓰기/이동/이름변경/복사, 이미지 변환·배경제거, 압축(zip), PDF, 화면 캡처.\n\
         12. 파일을 이동/이름변경한 뒤 옛 경로는 더 이상 없다. '방금 그 파일'은 마지막 도구 결과에\n\
            나온 새 경로를 쓴다. 이전 도구 호출을 그대로 복사하지 말고, 경로가 불확실하면 list_dir 로 먼저 확인한다.\n\
         13. 워크스페이스에서 파일을 못 찾으면 다른 폴더를 임의로 검색하지 말고 사용자에게 위치를 묻는다.\n\
         14. 작업 완료는 해당 도구의 성공 결과를 받았을 때만 말한다. 이름변경은 rename_file 의\n\
            '이름 변경 완료' 결과가 근거다. 파일에 목록을 적는 것(write_file)은 이름변경이 아니다.\n\n\
         {persona}",
        persona = persona_section(cfg)
    )
}

/// 페르소나/라포 형성 지시. 이름을 알면 친근한 말투, 모르면 대화 초반에 자연스럽게 묻는다.
fn persona_section(cfg: &AppConfig) -> String {
    // 설정에 '태경님'처럼 님까지 저장된 경우 이중 호칭(태경님님) 방지 — 페르소나가 님을 붙인다
    let user = cfg.user_name.trim();
    let user = user.strip_suffix("님").unwrap_or(user).trim();
    let agent = cfg.agent_name.trim();
    match (user.is_empty(), agent.is_empty()) {
        (false, false) => format!(
            "페르소나: 너의 이름은 '{agent}'이고, 사용자의 이름은 '{user}'다.\n\
             따뜻하고 친근한 말투를 쓰고, 가끔 '{user}님'처럼 이름을 불러준다."
        ),
        (true, true) => "페르소나: 아직 서로 이름을 모른다. 첫 인사나 잡담 때 자연스럽게 사용자의 이름을 묻고,\n\
             너의 이름도 하나 지어달라고 부탁하라. 이름은 설정 패널에서 저장할 수 있다고 안내하라.\n\
             단, 사용자가 작업을 요청하면 작업을 먼저 처리하고 이름은 나중에 물어본다."
            .to_string(),
        (true, false) => format!(
            "페르소나: 너의 이름은 '{agent}'다. 아직 사용자의 이름을 모르니 대화 초반에 자연스럽게 묻고,\n\
             이름은 설정 패널에서 저장할 수 있다고 안내하라. 따뜻하고 친근한 말투를 쓴다."
        ),
        (false, true) => format!(
            "페르소나: 사용자의 이름은 '{user}'다. 아직 너의 이름이 없으니 사용자에게 지어달라고 부탁하고,\n\
             이름은 설정 패널에서 저장할 수 있다고 안내하라. 따뜻하고 친근한 말투를 쓴다."
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
    let mut out: Vec<&'static str> = if BG_KEYWORDS.iter().any(|k| user_text.contains(k)) {
        // 배경제거 의도가 확실 — 회전/리사이즈로 새는 경로를 차단한다.
        // image_info 는 제외하지 않는다: 모델이 '확인 후 배경제거' 2단계로 쓰는 정상 경로
        // (2026-06-11 GT 단발 평가에서 image_info 선택은 누수가 아니라 선조회 패턴으로 확인됨)
        vec!["image_transform"]
    } else if is_dictation_write(user_text) {
        // 받아쓰기 쓰기 의도 — 읽기/탐색으로 새는 경로를 차단한다.
        // write_file 설명 보강만으로는 교정 실패(GT 0/5), remove_background 와 동일하게 제외만 동작.
        vec!["read_file", "list_dir", "search_files", "pdf_extract_text", "move_path", "rename_file", "copy_path", "delete_path"]
    } else {
        // 이름변경/압축풀기 의도는 합성 가능 ("압축풀고 이름 변경해줘" 복합 발화)
        let mut v = vec![];
        if is_rename_intent(user_text) {
            // 대체 행동 차단 (2026-06-12 실로그): write_file 로 변경 목록 txt 를 쓰거나
            // image_transform 으로 사본을 만들어 놓고 "변경 완료"라고 주장한다
            v.push("write_file");
            v.push("image_transform");
        }
        if is_extract_intent(user_text) {
            // zip_create 대체 행동 차단 (2026-06-12 실로그: 풀 zip 이 없자 새 zip 을 만들어버림)
            v.push("zip_create");
        } else if is_delete_intent(user_text) {
            // 삭제 의도에서 '압축' 토큰에 끌려 zip_extract 로 새는 경로 차단
            // (2026-06-12 실로그: "압축파일 모두 지워봐" → 폴더를 zip_extract).
            // zip_create 는 "압축해서 원본 지워줘" 복합을 위해 남겨둔다.
            v.push("zip_extract");
        } else if is_compress_intent(user_text) {
            // 압축 생성 의도에서 만든 zip 을 곧장 zip_extract 로 되푸는 배회 차단
            // (2026-06-12 R6 실측: a.zip 생성 직후 해제 + 환각 b.zip 해제 시도)
            v.push("zip_extract");
        }
        v
    };
    // set_workspace 는 사용자가 워크스페이스를 직접 말한 턴에만 노출한다.
    // 부작용이 턴을 넘어 지속되는 유일한 도구라 오발사 비용이 가장 크다
    // (2026-06-12 실로그: 모델이 임의 호출 → 다음 턴의 베어네임 경로 해석 전부 붕괴).
    const WS_KEYWORDS: &[&str] = &["워크스페이스", "작업 폴더", "작업폴더", "기본 폴더"];
    if !WS_KEYWORDS.iter().any(|k| user_text.contains(k)) {
        out.push("set_workspace");
    }
    // screen_capture 도 양성 게이트: 화면/캡처를 직접 말한 턴에만 노출.
    // (2026-06-12 실로그: "귀여워" 잡담에 4K 화면 캡처 발사 — 잡담·파일 턴에서
    //  정당한 트리거가 없는 도구는 보이는 것 자체가 오발사 위험)
    const CAPTURE_KEYWORDS: &[&str] = &["화면", "캡처", "캡쳐", "스크린", "찍"];
    if !CAPTURE_KEYWORDS.iter().any(|k| user_text.contains(k)) {
        out.push("screen_capture");
    }
    out
}

/// "압축 풀어/해제" 류의 압축 해제 의도인가?
fn is_extract_intent(user_text: &str) -> bool {
    user_text.contains("압축") && (user_text.contains("풀") || user_text.contains("해제"))
}

/// "압축해/압축하" 류의 압축 생성 의도인가? (해제 의도는 is_extract_intent 가 먼저 가로챈다.
/// "압축파일 안에 뭐 있어?" 같은 내용 조회는 동사가 없어 매칭되지 않는다 — list_only 보존)
fn is_compress_intent(user_text: &str) -> bool {
    user_text.contains("압축해") || user_text.contains("압축하")
}

/// "지워/삭제" 류의 삭제 의도인가?
fn is_delete_intent(user_text: &str) -> bool {
    const DELETE_VERBS: &[&str] = &["지워", "지우", "삭제"];
    DELETE_VERBS.iter().any(|v| user_text.contains(v))
}

/// "이름을 X로 바꿔/변경" 류의 파일 이름변경 의도인가?
/// 받아쓰기 마커("라고")가 있으면 쓰기가 본업인 복합 요청이므로 제외하지 않는다.
fn is_rename_intent(user_text: &str) -> bool {
    const RENAME_VERBS: &[&str] = &["바꿔", "바꾸", "변경"];
    user_text.contains("이름")
        && RENAME_VERBS.iter().any(|v| user_text.contains(v))
        && !user_text.contains("라고")
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

/// 성공이 하나도 없는 라운드(모든 호출이 실패/중복/차단/깨진 인자)를 이 횟수만큼 연속
/// 만나면 도구를 떼고 최종 답변을 강제한다. 2B 는 거절 피드백이 이력에 쌓일수록 같은
/// 호출을 더 강하게 베끼는 자기강화 루프에 빠진다 — 실로그: write_file 동일 호출 7회
/// 반복으로 라운드 예산을 다 태우고 "한도 초과" 실패로 끝난 턴이 다수 (2026-06-11 저녁).
/// 정책(2026-06-12): 실패하면 빨리 멈추고, 성공하면 계속 — 성공 라운드는 예산을 깎지
/// 않으며 절대 상한은 hard_cap(= max_tool_rounds × HARD_CAP_FACTOR)이 담당한다.
const MAX_REJECTED_ROUNDS: u32 = 2;

/// 파일시스템 상태를 바꾸지 않는 읽기 전용 도구들. 쓰기 도구가 성공하면 이 도구들의
/// 중복 호출 기록을 무효화한다 — 상태가 바뀐 뒤의 같은 인자 재조회는 중복이 아니라
/// 검증이다 (2026-06-12 실로그: delete ×3 후 list_dir 재확인이 중복 차단됨).
const READ_ONLY_TOOLS: &[&str] = &["list_dir", "search_files", "read_file", "image_info", "pdf_extract_text"];

/// 성공이 이어지는 턴에 허용하는 절대 라운드 상한 배수 (폭주 백스톱).
/// 실로그(2026-06-12): 8회 전부 성공하며 일하던 턴이 고정 한도에 잘림 → 성공은 계속
/// 허용하되, 완료 후에도 멈추지 않는 폭주(적대 S5-t3)는 이 상한이 막는다.
const HARD_CAP_FACTOR: u32 = 3;

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

/// 본문에 섞이는 도구 호출 마크업의 시작 표지
const MARKUP_MARKERS: &[&str] = &["<tool_call", "<function="];

/// 본문 텍스트에 섞여 나온 도구 호출 마크업("<tool_call>", "<function=")을 걷어낸다.
/// 서버가 파싱하지 못한 잘못된 형식의 호출 시도는 텍스트로 흘러들어오는데,
/// 그대로 두면 사용자 답변에 노출된다 (2026-06-12 적대 테스트 S3에서 확인).
/// 마크업 이후는 전부 호출 시도이므로 앞부분의 진짜 답변만 남긴다.
fn strip_tool_markup(content: &mut String) {
    let cut = MARKUP_MARKERS.iter().filter_map(|m| content.find(m)).min();
    if let Some(pos) = cut {
        content.truncate(pos);
        let trimmed = content.trim_end().len();
        content.truncate(trimmed);
    }
}

/// 본문 스트림에서 도구 마크업이 시작되면 그 지점부터 UI 방출을 차단한다.
/// strip_tool_markup 은 저장 단계에서만 동작 — 스트리밍 델타는 이미 UI 로 나간 뒤라
/// "<tool_call> <function=...>" 이 사용자 답변에 그대로 보였다 (2026-06-12 기획자 테스트).
/// 마커가 델타 경계에서 쪼개질 수 있으므로 마커 접두사일 수 있는 꼬리는 보류한다.
#[derive(Default)]
struct MarkupStreamGuard {
    pending: String,
    blocked: bool,
}

impl MarkupStreamGuard {
    /// 델타를 받아 지금 안전하게 방출할 수 있는 텍스트를 돌려준다
    fn push(&mut self, delta: &str) -> String {
        if self.blocked {
            return String::new();
        }
        self.pending.push_str(delta);
        if let Some(pos) = MARKUP_MARKERS.iter().filter_map(|m| self.pending.find(m)).min() {
            self.blocked = true;
            let out = self.pending[..pos].trim_end().to_string();
            self.pending.clear();
            return out;
        }
        let hold = marker_prefix_suffix_len(&self.pending);
        let cut = self.pending.len() - hold; // 보류 꼬리는 ASCII 마커 접두사라 경계 안전
        let out = self.pending[..cut].to_string();
        self.pending.drain(..cut);
        out
    }

    /// 스트림 종료: 보류 꼬리를 돌려주고 다음 완성을 위해 초기화한다
    fn finish(&mut self) -> String {
        let out = if self.blocked { String::new() } else { std::mem::take(&mut self.pending) };
        self.pending.clear();
        self.blocked = false;
        out
    }
}

/// 문자열 끝부분이 마커의 접두사와 일치하는 최대 길이 (스트림 경계 보류용)
fn marker_prefix_suffix_len(s: &str) -> usize {
    let max_check = MARKUP_MARKERS
        .iter()
        .map(|m| m.len() - 1)
        .max()
        .unwrap_or(0)
        .min(s.len());
    for k in (1..=max_check).rev() {
        if let Some(suffix) = s.get(s.len() - k..) {
            if MARKUP_MARKERS.iter().any(|m| m.starts_with(suffix)) {
                return k;
            }
        }
    }
    0
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

/// 시스템 메시지(messages[0])에 RAG 근거 블록이 합쳐져 있는가.
fn rag_active(messages: &[ChatMessage]) -> bool {
    messages
        .first()
        .and_then(|m| m.content.as_deref())
        .map(|c| c.contains(crate::localsearch::RAG_MARKER))
        .unwrap_or(false)
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
    // RAG 근거가 들어온 턴이면 정보검색 도구를 숨겨, 주입된 문서로 답하게 한다
    let rag_on = rag_active(messages);
    // 같은 (도구, 인자) 반복 차단 — 작은 모델의 루프 + 컨텍스트 낭비 방지
    let mut executed: std::collections::HashSet<(String, String)> = Default::default();
    // 중복 호출이 거절된 도구 — 다음 라운드부터 스키마에서 숨긴다. 2B 는 보이는 도구를
    // 계속 베끼므로(거절 피드백 무효) 베낄 대상을 치워야 다음 단계로 진행한다.
    // (2026-06-12 실로그: zip 해제 후 zip_extract 만 반복하다 rename 못 가고 강제중단)
    let mut looping_tools: std::collections::HashSet<String> = Default::default();
    // 빈 완성(사고만 하다 종료)은 샘플링 재시도로 한 번 회복을 시도한다
    let mut empty_retry_left = 1u32;
    // 연속으로 "성공이 하나도 없었던" 라운드 수 — MAX_REJECTED_ROUNDS 에서 강제 마무리
    let mut rejected_rounds = 0u32;
    // 이번 턴에 쓰기성 도구가 성공했는가 — 합성 폴백이 "전부 실패"와 "일부 완료 후
    // 실패"를 구분해 거짓 없는 문장을 고르는 근거 (2026-06-12 R6: 압축 성공 후
    // 배회 실패로 끝난 턴에 "파일을 찾지 못했어요" 질문만 나가 사용자를 혼란시킴)
    let mut turn_had_write_success = false;
    // 성공이 이어지는 한 계속 진행하되, 폭주는 절대 상한에서 끊는다
    let hard_cap = max_tool_rounds.saturating_mul(HARD_CAP_FACTOR).max(max_tool_rounds);

    for round in 0..=hard_cap {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        // 라운드마다 재구성: 발화 기반 제외 + 이번 턴에서 루프가 감지된 도구
        let tools = if rag_on {
            // RAG 근거가 들어온 턴은 도구를 제공하지 않는다 — 2B 가 내용 질문에도 도구로
            // 빠지는 것을 차단하고 주입된 [참고 문서]로만 답하게 한다.
            // (2026-06-14 실로그: 검색도구만 숨기니 images_to_pdf 를 엉뚱하게 호출)
            Value::Array(Vec::new())
        } else {
            let mut hidden = excluded.clone();
            hidden.extend(looping_tools.iter().map(String::as_str));
            registry.schemas_excluding(&hidden)
        };

        // 본문 마크업 방출 차단 가드 — 라운드 내 모든 완성(본 호출/재시도/마무리)이
        // 공유하고, 각 완성이 끝날 때 finish() 로 보류 꼬리를 방출하며 초기화한다.
        let markup_guard = std::sync::Mutex::new(MarkupStreamGuard::default());
        let mut sink = |kind: DeltaKind, text: &str| -> bool {
            match kind {
                DeltaKind::Thinking => emit(AgentEvent::ThinkingDelta {
                    session_id: session_id.to_string(),
                    delta: text.to_string(),
                }),
                DeltaKind::Text => {
                    let safe = markup_guard.lock().unwrap().push(text);
                    if !safe.is_empty() {
                        emit(AgentEvent::TextDelta {
                            session_id: session_id.to_string(),
                            delta: safe,
                        });
                    }
                }
            }
            // false 반환 시 클라이언트가 스트림을 끊는다 — 생성 중에도 ■ 버튼이 즉시 듣게
            !cancel.load(Ordering::Relaxed)
        };
        let flush_stream_tail = || {
            let tail = markup_guard.lock().unwrap().finish();
            if !tail.is_empty() {
                emit(AgentEvent::TextDelta {
                    session_id: session_id.to_string(),
                    delta: tail,
                });
            }
        };
        let mut result = match client.complete(messages, &tools, temperature, &mut sink).await {
            Ok(r) => r,
            // 컨텍스트 초과: 오래된 도구 결과를 압축하고 한 번 더 시도
            Err(e) if e.to_string().contains("exceed") && e.to_string().contains("context") => {
                let compacted = compact_old_tool_results(messages);
                if compacted == 0 {
                    return Err(e);
                }
                flush_stream_tail();
                client.complete(messages, &tools, temperature, &mut sink).await?
            }
            Err(e) => return Err(e),
        };
        flush_stream_tail();

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

        // 모델이 도구 호출 문법을 본문 텍스트로 뱉으면(서버가 파싱 못 한 잘못된 형식)
        // 사용자 답변으로 노출하지 않는다. 마크업 앞의 진짜 텍스트만 남기고,
        // 남는 게 없으면 빈 완성으로 취급해 아래의 재시도 경로를 그대로 태운다.
        if result.tool_calls.is_empty() {
            strip_tool_markup(&mut result.content);
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
        if round == hard_cap {
            // 절대 상한 소진: 도구 결과 없이 종료를 알린다
            emit(AgentEvent::Error {
                session_id: session_id.to_string(),
                message: format!("도구 호출 한도({hard_cap}회) 초과로 중단"),
            });
            break;
        }

        let mut any_success = false;
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
                looping_tools.insert(call.function.name.clone());
                (false, "이미 같은 인자로 호출한 도구입니다. 위의 기존 결과를 사용하거나 다른 행동을 취하세요.".to_string())
            } else {
                let mut args: Value = serde_json::from_str(&call.function.arguments)
                    .unwrap_or(Value::Object(Default::default()));
                // 발화/경로 기반 인자 보정 — 2B 의 알려진 실수를 실행 직전에 흡수
                absorb_relative_path_args(&mut args, &tool_ctx.workspace());
                fix_resize_axis(&user_text, &call.function.name, &mut args);
                inject_named_output(&user_text, &call.function.name, &mut args);
                // 도구는 동기 구현 — 블로킹 실행을 런타임에 알린다
                let output = tokio::task::block_in_place(|| {
                    registry.execute(&call.function.name, &args, tool_ctx)
                });
                match output {
                    Ok(t) => (true, t),
                    Err(e) => (false, format!("오류: {e:#}")),
                }
            };
            any_success |= ok;
            // 쓰기 도구 성공 = 상태 변화 → 읽기 도구의 중복 기록/루프 숨김을 풀어준다
            if ok && !READ_ONLY_TOOLS.contains(&call.function.name.as_str()) {
                turn_had_write_success = true;
                executed.retain(|(n, _)| !READ_ONLY_TOOLS.contains(&n.as_str()));
                looping_tools.retain(|n| !READ_ONLY_TOOLS.contains(&n.as_str()));
            }
            emit(AgentEvent::ToolCallEnd {
                session_id: session_id.to_string(),
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                ok,
                result: clip(&text, 2000),
            });
            messages.push(ChatMessage::tool(call.id.clone(), clip(&text, 4000)));
        }

        // 성공 없는 라운드 연속 감지 → 도구를 떼고 지금까지의 결과로 최종 답변을 강제.
        // 실패(도구 에러 포함)는 빨리 멈추고, 성공하는 동안은 계속 — 2026-06-12 정책.
        // 단, 위치 힌트가 담긴 실패는 회복 정보이므로 카운트를 보류한다 — 모델이 그 경로로
        // 재시도할 라운드를 보장 (실로그: 힌트 도착 직후 조기중단돼 다음 턴에야 회복).
        // (거절 피드백만으로는 2B 가 루프를 못 벗어난다 — MAX_REJECTED_ROUNDS 주석 참고)
        let any_hint = messages
            .iter()
            .rev()
            .take(result.tool_calls.len())
            .any(|m| {
                m.role == "tool"
                    && m.content.as_deref().is_some_and(|c| c.contains("이 경로로 다시 시도하세요"))
            });
        // "주변에서도 못 찾음 — 사용자에게 물어보라"(not_found_msg) 마커가 담긴 실패
        // 라운드는 더 배회시키지 않고 즉시 마무리로 보낸다. 2B 는 에러 속 질문 지시를
        // 무시하고 다른 도구로 샌다 (2026-06-12 R2 실측: list_dir 배회 → 빈 마무리 →
        // 무의미한 중단 안내). 지시는 약하고 도구 노출 제어가 결정적이라는 동일 교훈.
        // "사용자에게 알리세요" 마커(불가능 작업 — 이미지 아님 등 회복 불가)는 즉시
        // 배회를 끊는다 (2026-06-12 R5). 단, "물어보세요"(파일 못 찾음)는 즉시 끊지
        // 않는다 — 에러에 담긴 현재 폴더 후보 목록으로 회복할 1라운드를 보장한다
        // (2026-06-12 S1-t3 회귀: 즉시 종결이 바로 옆 파일을 두고 사용자에게 물었음).
        // 회복 실패 시엔 rejected_rounds 가 마무리하고, 합성 질문 폴백이 동일하게 적용된다.
        let any_tell_user = messages
            .iter()
            .rev()
            .take(result.tool_calls.len())
            .any(|m| {
                m.role == "tool"
                    && m.content
                        .as_deref()
                        .is_some_and(|c| c.contains("사용자에게 알리세요"))
            });
        if any_success {
            rejected_rounds = 0;
        } else if !any_hint {
            rejected_rounds += 1;
        }
        let must_finish = rejected_rounds >= MAX_REJECTED_ROUNDS
            || (!any_success && !any_hint && any_tell_user);
        if must_finish && !cancel.load(Ordering::Relaxed) {
            let no_tools = Value::Array(vec![]);
            let mut final_result = client.complete(messages, &no_tools, temperature, &mut sink).await?;
            flush_stream_tail();
            // 도구를 떼고 물어도 2B 는 마크업을 텍스트로 뱉을 수 있다 (2026-06-12 적대 테스트)
            strip_tool_markup(&mut final_result.content);
            if !final_result.content.is_empty() {
                messages.push(ChatMessage::assistant(Some(final_result.content), None));
            } else {
                // 마무리 호출마저 비면 결정적 폴백을 합성한다 — 모델이 문장을 작문하지
                // 못해도 사용자는 항상 정직한 상태 보고(질문/실패 이유)를 받는다.
                let note = if !turn_had_write_success {
                    synthesize_location_question(messages)
                } else {
                    None // 일부 완료된 턴에 "못 찾았어요" 질문만 나가면 혼란 (R6)
                }
                .or_else(|| synthesize_failure_note(messages, turn_had_write_success));
                match note {
                    Some(q) => {
                        emit(AgentEvent::TextDelta {
                            session_id: session_id.to_string(),
                            delta: q.clone(),
                        });
                        messages.push(ChatMessage::assistant(Some(q), None));
                    }
                    None => emit(AgentEvent::Error {
                        session_id: session_id.to_string(),
                        message: "도구 실행이 진전 없이 반복되어 작업을 중단했습니다. 요청을 바꿔 다시 시도해보세요.".into(),
                    }),
                }
            }
            break;
        }
    }

    close_dangling_tool_calls(messages);
    // 최종 답변 없이 끝난 턴(한도/강제중단/취소)은 명시적 종결 문장을 남긴다.
    // 미완 작업 흔적이 다음 턴을 오염시키는 것을 막는다 (2026-06-12 실로그:
    // 중단된 턴의 이름변경을 다음 턴 "압축 풀기" 요청에서 모델이 멋대로 이어함).
    let ended_without_answer = messages.last().is_some_and(|m| {
        !(m.role == "assistant" && m.content.as_deref().is_some_and(|c| !c.is_empty()))
    });
    if ended_without_answer {
        // 사용자에게도 보일 수 있는 문장이므로(세션 복원/빈 답변 턴) 자연스럽게 쓴다.
        // 모델용 신호("새로 진행")와 사용자용 안내를 겸한다.
        messages.push(ChatMessage::assistant(
            Some("이 작업은 완료되지 않은 채 중단되었습니다. 다음 요청부터 새로 진행합니다.".into()),
            None,
        ));
    }
    emit(AgentEvent::TurnEnd {
        session_id: session_id.to_string(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
    Ok(())
}

/// 상대 경로 인자를 워크스페이스 기준 절대경로로 흡수한다. 2B 는 직전 list_dir 결과가
/// 보여준 이름("1.png")을 그대로 인자에 베껴 쓴다 — 거부하면 배회로 빠지므로 의도가
/// 명백한 상대 이름은 워크스페이스로 해석한다 (2026-06-12 R7 실측: paths=["1.png",...]
/// 검증 실패 → image_info 우회 → 중복 → zip_create 대체 행동까지 연쇄).
fn absorb_relative_path_args(args: &mut Value, ws: &std::path::Path) {
    const PATH_KEYS: &[&str] = &["path", "from", "to", "dir", "root", "output_path", "output_dir"];
    let to_abs = |s: &str| -> Option<String> {
        let t = s.trim();
        if t.is_empty() || !std::path::Path::new(t).is_relative() {
            return None;
        }
        Some(ws.join(t).to_string_lossy().replace('\\', "/"))
    };
    let Some(obj) = args.as_object_mut() else { return };
    for k in PATH_KEYS {
        let fixed = obj.get(*k).and_then(Value::as_str).and_then(to_abs);
        if let Some(p) = fixed {
            obj.insert((*k).to_string(), Value::String(p));
        }
    }
    // paths: 배열(images_to_pdf) 또는 쉼표 문자열(zip_create)
    match obj.get_mut("paths") {
        Some(Value::Array(items)) => {
            for it in items.iter_mut() {
                if let Some(p) = it.as_str().and_then(to_abs) {
                    *it = Value::String(p);
                }
            }
        }
        Some(Value::String(s)) => {
            // 모델이 JSON 배열을 문자열로 감싼 실수도 흡수: "[\"a.png\", \"b.png\"]"
            let parts: Vec<String> = serde_json::from_str::<Vec<String>>(s)
                .unwrap_or_else(|_| s.split(',').map(|p| p.trim().to_string()).collect());
            *s = parts
                .iter()
                .filter(|p| !p.is_empty())
                .map(|p| to_abs(p).unwrap_or_else(|| p.clone()))
                .collect::<Vec<_>>()
                .join(",");
        }
        _ => {}
    }
}

/// 발화-인자 불일치 교정: 사용자가 '가로'만 말했는데 모델이 resize_height 를 준 경우
/// (또는 반대) 축을 바꿔 실행한다. 2B 는 가로/세로 → width/height 의미 매핑에 일관되게
/// 실패하며 스키마 설명 보강으로도 교정되지 않는다 (2026-06-12 R4/R9 실측 — 설명에
/// "'가로 N으로' 요청은 이것"을 명시해도 resize_height 선택, 300px 를 400 으로 확대).
fn fix_resize_axis(user_text: &str, tool: &str, args: &mut Value) {
    if tool != "image_transform" {
        return;
    }
    let (says_w, says_h) = (user_text.contains("가로"), user_text.contains("세로"));
    let Some(obj) = args.as_object_mut() else { return };
    if says_w && !says_h {
        if let Some(v) = obj.remove("resize_height") {
            obj.entry("resize_width").or_insert(v);
        }
    } else if says_h && !says_w {
        if let Some(v) = obj.remove("resize_width") {
            obj.entry("resize_height").or_insert(v);
        }
    }
}

/// 사용자가 출력 파일명(".pdf"/".zip" 토큰)을 말했는데 모델이 output_path 를 생략한
/// 경우 그 이름을 주입한다 (2026-06-12 R7 실측: "album.pdf로 만들어줘" → output_path
/// 생략 → 자동 이름 images.pdf 저장 후 "album.pdf로 저장했다"고 거짓 보고).
fn inject_named_output(user_text: &str, tool: &str, args: &mut Value) {
    let ext = match tool {
        "images_to_pdf" => "pdf",
        "zip_create" => "zip",
        _ => return,
    };
    let Some(obj) = args.as_object_mut() else { return };
    let has_output = obj
        .get("output_path")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty());
    if has_output {
        return;
    }
    let Some(name) = filename_with_ext(user_text, ext) else { return };
    // 입력 인자에 이미 등장하는 이름이면 출력 의도가 아니다 (입력 파일명 오인 방지)
    let inputs = format!(
        "{} {}",
        obj.get("paths").map(|v| v.to_string()).unwrap_or_default(),
        obj.get("dir").map(|v| v.to_string()).unwrap_or_default()
    );
    if inputs.contains(&name) {
        return;
    }
    // 이름만 넣는다 — 도구의 absorb_into_workspace 가 워크스페이스 절대경로로 해석
    obj.insert("output_path".into(), Value::String(name));
}

/// 텍스트에서 ".{ext}" 로 끝나는 파일명 토큰을 추출한다 ("album.pdf로 만들어줘" → "album.pdf")
fn filename_with_ext(text: &str, ext: &str) -> Option<String> {
    let needle = format!(".{ext}");
    let pos = text.find(&needle)?;
    let end = pos + needle.len();
    // 확장자 뒤가 영숫자로 이어지면(예: "a.pdfx") 파일명 토큰이 아니다.
    // 한글 조사("album.pdf로")는 정상적인 후행이므로 ASCII 만 검사한다.
    if text[end..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let head: String = text[..pos]
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-') || ('가'..='힣').contains(c))
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if head.is_empty() {
        return None;
    }
    Some(format!("{head}{needle}"))
}

/// 직근 도구 결과의 "파일 없음: <경로>. ... 물어보세요" 에러에서 파일명을 뽑아
/// 사용자에게 위치를 묻는 문장을 만든다. 강제 마무리가 빈 완성으로 끝났을 때의
/// 결정적 폴백 — 2B 가 질문을 작문하지 못해도 대화가 회복 가능한 상태로 끝난다.
fn synthesize_location_question(messages: &[ChatMessage]) -> Option<String> {
    let marker_errors: Vec<&str> = messages
        .iter()
        .rev()
        .filter(|m| m.role == "tool")
        .filter_map(|m| m.content.as_deref())
        .filter(|c| c.contains("물어보세요"))
        .collect();
    if marker_errors.is_empty() {
        return None;
    }
    // "파일 없음: <경로>. ..." 에서 파일명 추출 (문장 마침표 ". " 가 경로의 끝)
    let extract = |content: &str| -> Option<String> {
        content
            .split("파일 없음:")
            .nth(1)
            .map(|rest| rest.trim_start())
            .and_then(|rest| rest.split(". ").next())
            .and_then(|path| std::path::Path::new(path.trim()).file_name())
            .map(|n| n.to_string_lossy().into_owned())
    };
    // 모델이 인자 파일명을 환각하기도 한다 (2026-06-12 R2: '발표자료.pdf' →
    // 'final_report.pdf' 로 영문화). 사용자가 실제로 말한 이름을 우선해 질문에 쓴다.
    let user_text = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.as_deref())
        .unwrap_or("");
    let user_said = |n: &str| {
        let stem = std::path::Path::new(n)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| n.to_string());
        user_text.contains(n) || user_text.contains(&stem)
    };
    let name = marker_errors
        .iter()
        .filter_map(|c| extract(c))
        .find(|n| user_said(n));
    Some(match name {
        Some(n) => {
            format!("'{n}' 파일을 찾지 못했어요. 어느 폴더에 있는지 알려주시면 다시 시도할게요.")
        }
        None => "요청하신 파일을 찾지 못했어요. 정확한 위치(폴더)를 알려주시겠어요?".into(),
    })
}

/// 마지막 도구 오류에서 정직한 실패 보고를 합성한다 — "완료되지 않은 채 중단" 같은
/// 무의미한 안내 대신 실패 이유가 사용자에게 전달된다 (2026-06-12 R5/R8 실측).
/// 쓰기 성공이 있었던 턴은 "일부 완료"를 명시해 거짓 없는 문장을 유지한다.
fn synthesize_failure_note(messages: &[ChatMessage], had_success: bool) -> Option<String> {
    let err = messages
        .iter()
        .rev()
        .filter(|m| m.role == "tool")
        .filter_map(|m| m.content.as_deref())
        .find(|c| c.starts_with("오류:"))?;
    let reason = clip(&strip_model_directives(err.trim_start_matches("오류:").trim()), 140);
    Some(if had_success {
        format!("요청 중 일부는 완료했지만 마지막 단계는 실패했어요. 이유: {reason}")
    } else {
        format!("요청하신 작업을 완료하지 못했어요. 이유: {reason}")
    })
}

/// 도구 오류 속 모델용 지시("이 사실을 그대로 사용자에게 알리세요" 등)를 걷어내고
/// 사용자에게 보여줄 사실 부분만 남긴다 — 합성 노트가 내부 지시문을 그대로 인용해
/// 사용자에게 노출됐다 (2026-06-12 기획자 테스트 직후 실측).
fn strip_model_directives(reason: &str) -> String {
    const DIRECTIVE_STARTS: &[&str] = &[
        "이 사실을 그대로",
        "다른 경로를 추측해",
        "요청한 파일이 이 중에",
        "사용자에게 파일의 정확한 위치",
        "제공된 도구 목록에서",
    ];
    let cut = DIRECTIVE_STARTS
        .iter()
        .filter_map(|d| reason.find(d))
        .min()
        .unwrap_or(reason.len());
    reason[..cut]
        .trim_end()
        .trim_end_matches(['—', '-', ','])
        .trim_end()
        .to_string()
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

    #[test]
    fn rag_active_detects_marker_in_system_message() {
        let marker = crate::localsearch::RAG_MARKER;
        let with = vec![
            ChatMessage::system(format!("시스템 프롬프트\n\n{marker}\n[#1 문서: a.pdf] 내용")),
            ChatMessage::user("질문"),
        ];
        assert!(rag_active(&with));

        let without = vec![
            ChatMessage::system("시스템 프롬프트"),
            ChatMessage::user("질문"),
        ];
        assert!(!rag_active(&without));
    }
    use crate::llm::client::DeltaSink;
    use crate::models::{CompletionResult, FunctionCall, ToolCall};
    use std::sync::Mutex;

    /// 호출 순서대로 미리 준비한 응답(또는 오류)을 돌려주는 mock.
    /// 호출마다 받은 도구 스키마 개수를 기록한다 (강제 마무리의 무도구 호출 검증용).
    struct MockClient {
        responses: Mutex<Vec<Result<CompletionResult>>>,
        tool_counts: Mutex<Vec<usize>>,
        /// 호출별로 받은 도구 스키마의 이름들 (턴 중 동적 숨김 검증용)
        tool_names: Mutex<Vec<Vec<String>>>,
    }

    impl MockClient {
        fn ok(responses: Vec<CompletionResult>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(Ok).collect()),
                tool_counts: Mutex::new(vec![]),
                tool_names: Mutex::new(vec![]),
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
            self.tool_names.lock().unwrap().push(
                tools
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            );
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

    /// 성공이 이어지는 동안은 라운드 예산(max_tool_rounds)을 넘어 계속 진행한다.
    /// (2026-06-12 실로그: 8회 전부 성공하며 일하던 턴이 고정 한도에 잘림 —
    ///  "실패하면 중단, 성공하면 계속" 정책으로 변경)
    #[tokio::test(flavor = "multi_thread")]
    async fn successful_rounds_extend_beyond_round_budget() {
        let dir = tempfile::tempdir().unwrap();
        // 서로 다른 인자의 성공 호출 6개 — max_tool_rounds=3 을 넘어서도 이어져야 한다
        let mut responses: Vec<CompletionResult> = (0..6)
            .map(|i| {
                let sub = dir.path().join(format!("d{i}"));
                std::fs::create_dir_all(&sub).unwrap();
                tool_call_result("list_dir", serde_json::json!({"path": sub.to_string_lossy()}))
            })
            .collect();
        responses.push(CompletionResult { content: "6개 폴더 확인 끝.".into(), ..Default::default() });
        let client = MockClient::ok(responses);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("폴더들 봐줘")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 3, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(
            !evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })),
            "성공 진행 중엔 한도 에러가 나면 안 됨"
        );
        assert_eq!(messages.last().unwrap().content.as_deref(), Some("6개 폴더 확인 끝."));
    }

    /// 도구가 연속 2라운드 전부 실패하면 한도까지 끌지 않고 그 시점에 마무리한다.
    /// (ask-user 마커가 없는 일반 실패 — zip 아닌 파일에 zip_extract — 로 검증한다.
    ///  '파일 없음+물어보세요' 실패는 1라운드 만에 끝나는 별도 경로가 있다)
    #[tokio::test(flavor = "multi_thread")]
    async fn consecutive_failed_rounds_finish_early() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "1").unwrap();
        std::fs::write(&f2, "2").unwrap();
        let client = MockClient::ok(vec![
            tool_call_result("zip_extract", serde_json::json!({"path": f1.to_string_lossy()})),
            tool_call_result("zip_extract", serde_json::json!({"path": f2.to_string_lossy()})),
            // 강제 마무리(무도구) 응답
            CompletionResult { content: "파일을 찾지 못했습니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("읽어줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        let counts = client.tool_counts.lock().unwrap().clone();
        assert_eq!(counts.len(), 3, "실패 2라운드 후 즉시 마무리 (8회까지 안 끌어야 함)");
        assert_eq!(counts[2], 0, "마무리 호출은 도구 없이");
        assert_eq!(messages.last().unwrap().content.as_deref(), Some("파일을 찾지 못했습니다."));
    }

    /// 성공이 이어져도 절대 상한(3×)에서는 멈춘다 — 폭주 백스톱
    #[tokio::test(flavor = "multi_thread")]
    async fn hard_cap_stops_runaway_success() {
        let dir = tempfile::tempdir().unwrap();
        let responses: Vec<CompletionResult> = (0..10)
            .map(|i| {
                let sub = dir.path().join(format!("r{i}"));
                std::fs::create_dir_all(&sub).unwrap();
                tool_call_result("list_dir", serde_json::json!({"path": sub.to_string_lossy()}))
            })
            .collect();
        let client = MockClient::ok(responses);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("계속해")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        // max=2 → 절대 상한 6: 성공만 반복해도 6라운드에서 한도 에러로 끊겨야 한다
        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 2, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let evs = events.lock().unwrap();
        assert!(evs.iter().any(
            |e| matches!(e, AgentEvent::Error { message, .. } if message.contains("한도"))
        ));
        assert!(client.tool_counts.lock().unwrap().len() <= 7, "상한 부근에서 멈춰야 함");
    }

    /// 최종 답변 없이 중단된 턴은 히스토리에 명시적 종결 문장을 남긴다.
    /// (2026-06-12 실로그: 중단된 턴의 이름변경 작업을 다음 턴 "압축 풀기" 요청에서
    ///  모델이 멋대로 이어함 — 미완 흔적이 다음 턴을 오염)
    #[tokio::test(flavor = "multi_thread")]
    async fn aborted_turn_leaves_explicit_closure_message() {
        // 전부 실패하는 호출 2라운드(마커 없는 일반 실패) → 강제 마무리도 빈 응답
        // → 실패 노트 합성으로 정직하게 종결
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "1").unwrap();
        std::fs::write(&f2, "2").unwrap();
        let client = MockClient::ok(vec![
            tool_call_result("zip_extract", serde_json::json!({"path": f1.to_string_lossy()})),
            tool_call_result("zip_extract", serde_json::json!({"path": f2.to_string_lossy()})),
            CompletionResult::default(), // 강제 마무리 응답이 빈 완성
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("읽어줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        let last = messages.last().unwrap();
        assert_eq!(last.role, "assistant", "중단 턴도 assistant 종결로 끝나야 함");
        // 빈 마무리는 이제 실패 노트로 합성된다 — 미완/실패가 명시돼야 한다
        let text = last.content.as_deref().unwrap_or("");
        assert!(
            text.contains("완료하지 못했") || text.contains("완료되지 않"),
            "미완 명시 없음: {text:?}"
        );
    }

    /// 쓰기 도구가 성공하면 읽기 도구의 중복 기록을 무효화한다 — 상태가 바뀌었으니
    /// 같은 인자의 재조회는 중복이 아니라 검증이다.
    /// (2026-06-12 실로그: delete ×3 후 list_dir 재확인이 중복 차단돼 남은 파일을
    ///  못 보고 환각 파일명으로 삭제 시도 → 조기중단)
    #[tokio::test(flavor = "multi_thread")]
    async fn mutation_invalidates_read_tool_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.txt");
        std::fs::write(&file, "x").unwrap();
        let dir_args = serde_json::json!({"path": dir.path().to_string_lossy()});
        let client = MockClient::ok(vec![
            tool_call_result("list_dir", dir_args.clone()),
            tool_call_result("delete_path", serde_json::json!({"path": file.to_string_lossy()})),
            // 삭제 후 같은 인자의 재조회 — 신선한 결과로 실행돼야 한다
            tool_call_result("list_dir", dir_args.clone()),
            CompletionResult { content: "정리 완료, 폴더가 비었습니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("x.txt 지우고 확인해줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        assert!(
            !messages.iter().any(|m| m
                .content
                .as_deref()
                .is_some_and(|c| c.contains("이미 같은 인자"))),
            "변이 후 재조회가 중복으로 거절됨"
        );
        assert_eq!(
            messages.last().unwrap().content.as_deref(),
            Some("정리 완료, 폴더가 비었습니다.")
        );
    }

    /// 위치 힌트("같은 이름의 파일 발견")가 담긴 실패 라운드는 중단 카운터에 세지 않는다.
    /// 힌트는 회복 정보 — 모델이 그 경로로 다시 시도할 기회를 줘야 한다.
    /// (2026-06-12 실로그: r1 힌트 도착 → r2 다른 실패 → 조기중단으로 힌트를 못 써봄.
    ///  다음 턴에서 모델이 히스토리의 힌트를 베껴 성공한 것이 회복 가능성의 증거)
    #[tokio::test(flavor = "multi_thread")]
    async fn hint_round_does_not_count_toward_early_finish() {
        let dir = tempfile::tempdir().unwrap();
        // 워크스페이스는 하위 폴더, 실제 파일은 부모에 (오염 시나리오)
        let ws = dir.path().join("pngs");
        std::fs::create_dir(&ws).unwrap();
        let real = dir.path().join("data.txt");
        std::fs::write(&real, "내용").unwrap();

        let wrong = ws.join("data.txt").to_string_lossy().to_string();
        // r2 실패는 ask-user 마커가 없는 종류여야 한다 (zip 아닌 파일에 zip_extract)
        let dummy = ws.join("dummy.txt");
        std::fs::write(&dummy, "d").unwrap();
        let missing_dir = dummy.to_string_lossy().to_string();
        let client = MockClient::ok(vec![
            // r1: 파일 없음 + 위치 힌트 (read_file 의 not_found_hint 가 부모에서 발견)
            tool_call_result("read_file", serde_json::json!({"path": wrong})),
            // r2: 힌트와 무관한 실패 (zip 아닌 파일에 zip_extract) — 기존 정책이면 여기서 조기중단
            tool_call_result("zip_extract", serde_json::json!({"path": missing_dir})),
            // r3: 힌트의 경로로 재시도 — 이 라운드까지 살아 있어야 한다
            tool_call_result("read_file", serde_json::json!({"path": real.to_string_lossy()})),
            CompletionResult { content: "내용 확인 완료.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg);
        let mut messages = vec![ChatMessage::user("data.txt 읽어줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &ctx, &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        assert_eq!(
            messages.last().unwrap().content.as_deref(),
            Some("내용 확인 완료."),
            "힌트 라운드가 카운트되면 r3 전에 조기중단된다"
        );
    }

    /// "주변에서도 못 찾음 — 물어보세요" 실패는 후보 목록으로 회복할 1라운드를 받고,
    /// 그래도 실패하면 강제 마무리에서 위치 질문이 합성된다 — 모델이 질문을 작문하지
    /// 못해도 사용자는 항상 다음 행동(위치 제공)을 안내받는다.
    /// (즉시 종결은 S1-t3 회귀를 만들었다: 바로 옆 파일을 두고 사용자에게 물음)
    #[tokio::test(flavor = "multi_thread")]
    async fn empty_forced_finish_synthesizes_location_question() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("발표자료.pdf").to_string_lossy().to_string();
        let missing2 = dir.path().join("발표자료2.pdf").to_string_lossy().to_string();
        let client = MockClient::ok(vec![
            tool_call_result("read_file", serde_json::json!({"path": missing})),
            // 회복 라운드도 실패 → rejected_rounds 로 강제 마무리
            tool_call_result("read_file", serde_json::json!({"path": missing2})),
            CompletionResult::default(), // 강제 마무리가 빈 완성
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = dir.path().to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg);
        let mut messages = vec![ChatMessage::user("발표자료.pdf 요약해줘")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &ctx, &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let answer = messages.last().unwrap().content.as_deref().unwrap();
        assert!(answer.contains("발표자료.pdf"), "파일명이 질문에 있어야 함: {answer}");
        assert!(answer.contains("알려주"), "위치 질문이어야 함: {answer}");
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(e, AgentEvent::TextDelta { .. })),
            "합성 질문은 UI 에도 흘러야 함"
        );
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Error { .. })), "에러 대신 질문으로 종료");
    }

    /// 위치 힌트가 있는 실패는 ask-user 마커가 아니다 — 재시도 라운드를 보장한다
    #[tokio::test(flavor = "multi_thread")]
    async fn location_hint_failure_still_allows_retry_round() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(dir.path().join("data.txt"), "내용").unwrap();
        let wrong = ws.join("data.txt").to_string_lossy().to_string();
        let real = dir.path().join("data.txt").to_string_lossy().to_string();
        let client = MockClient::ok(vec![
            tool_call_result("read_file", serde_json::json!({"path": wrong})), // 힌트 실패
            tool_call_result("read_file", serde_json::json!({"path": real})),  // 힌트로 회복
            CompletionResult { content: "내용 확인.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg);
        let mut messages = vec![ChatMessage::user("data.txt 읽어줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &ctx, &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        assert_eq!(messages.last().unwrap().content.as_deref(), Some("내용 확인."));
    }

    /// 마커 없는 실패로 끝난 빈 마무리는 마지막 오류에서 정직한 실패 노트를 합성한다
    /// (2026-06-12 R5: "완료되지 않은 채 중단" 무의미 안내 대신 이유가 전달돼야 함)
    #[tokio::test(flavor = "multi_thread")]
    async fn empty_forced_finish_synthesizes_failure_note() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "1").unwrap();
        std::fs::write(&f2, "2").unwrap();
        let client = MockClient::ok(vec![
            tool_call_result("zip_extract", serde_json::json!({"path": f1.to_string_lossy()})),
            tool_call_result("zip_extract", serde_json::json!({"path": f2.to_string_lossy()})),
            CompletionResult::default(), // 강제 마무리가 빈 완성
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("압축 좀 풀어줘")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let answer = messages.last().unwrap().content.as_deref().unwrap();
        assert!(answer.contains("완료하지 못했어요"), "{answer}");
        assert!(answer.contains("zip"), "실패 이유가 담겨야 함: {answer}");
    }

    /// 불가능 작업("txt 회전") — 이미지 디코드 실패 에러의 '알리세요' 마커가
    /// 1라운드 만에 배회를 끊고 정직한 실패로 종결시킨다 (2026-06-12 R5)
    #[tokio::test(flavor = "multi_thread")]
    async fn impossible_image_op_finishes_in_one_round() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, "메모").unwrap();
        let client = MockClient::ok(vec![
            tool_call_result(
                "image_transform",
                serde_json::json!({"path": txt.to_string_lossy(), "rotate": 90}),
            ),
            CompletionResult::default(), // 강제 마무리가 빈 완성 → 실패 노트 합성
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = dir.path().to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg);
        let mut messages = vec![ChatMessage::user("notes.txt를 90도 회전시켜줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &ctx, &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        let counts = client.tool_counts.lock().unwrap().clone();
        assert_eq!(counts.len(), 2, "알리세요 마커 후 즉시 마무리");
        let answer = messages.last().unwrap().content.as_deref().unwrap();
        assert!(answer.contains("이미지가 아니"), "실패 이유 전달: {answer}");
    }

    /// 압축 생성 의도 턴에는 zip_extract 를 숨긴다 (2026-06-12 R6: 만든 zip 을 곧장
    /// 해제 + 환각 zip 해제 시도). 해제/조회 의도는 건드리지 않는다.
    #[test]
    fn compress_intent_excludes_zip_extract() {
        assert!(tools_to_exclude("photos 폴더 압축해줘").contains(&"zip_extract"));
        assert!(tools_to_exclude("이미지들 압축해서 보관해줘").contains(&"zip_extract"));
        assert!(!tools_to_exclude("백업.zip 압축 풀어줘").contains(&"zip_extract"));
        assert!(!tools_to_exclude("압축 해제해줘").contains(&"zip_extract"));
        assert!(!tools_to_exclude("백업.zip 안에 뭐 있어?").contains(&"zip_extract"));
    }

    /// 상대 경로 인자는 워크스페이스 기준으로 흡수된다 (2026-06-12 R7: list_dir 가
    /// 보여준 "1.png" 를 그대로 베껴 쓴 paths 배열이 검증 실패 → 배회 연쇄)
    #[test]
    fn relative_path_args_are_absorbed_into_workspace() {
        let ws = std::path::Path::new("C:/ws");
        // 단일 path 키
        let mut args = serde_json::json!({"path": "1.png"});
        absorb_relative_path_args(&mut args, ws);
        assert_eq!(args["path"], "C:/ws/1.png");
        // 절대경로는 보존
        let mut args = serde_json::json!({"path": "D:/other/x.png"});
        absorb_relative_path_args(&mut args, ws);
        assert_eq!(args["path"], "D:/other/x.png");
        // 배열 paths (images_to_pdf)
        let mut args = serde_json::json!({"paths": ["1.png", "C:/abs/2.png"]});
        absorb_relative_path_args(&mut args, ws);
        assert_eq!(args["paths"][0], "C:/ws/1.png");
        assert_eq!(args["paths"][1], "C:/abs/2.png");
        // JSON 배열을 문자열로 감싼 실수 (zip_create)
        let mut args = serde_json::json!({"paths": "[\"1.png\", \"2.png\"]"});
        absorb_relative_path_args(&mut args, ws);
        assert_eq!(args["paths"], "C:/ws/1.png,C:/ws/2.png");
        // 쉼표 문자열
        let mut args = serde_json::json!({"paths": "a.png, C:/abs/b.png"});
        absorb_relative_path_args(&mut args, ws);
        assert_eq!(args["paths"], "C:/ws/a.png,C:/abs/b.png");
    }

    /// 사용자가 '가로'만 말했는데 모델이 resize_height 를 주면 축을 교체한다
    /// (2026-06-12 R4/R9: 스키마 설명 보강으로도 교정 실패한 의미 매핑 혼동)
    #[test]
    fn resize_axis_fix_swaps_mismatched_dimension() {
        let mut args = serde_json::json!({"path": "a.png", "resize_height": 800});
        fix_resize_axis("photo.png 가로 800으로 줄여줘", "image_transform", &mut args);
        assert_eq!(args["resize_width"], 800);
        assert!(args.get("resize_height").is_none());

        let mut args = serde_json::json!({"path": "a.png", "resize_width": 600});
        fix_resize_axis("세로 600으로 맞춰줘", "image_transform", &mut args);
        assert_eq!(args["resize_height"], 600);

        // 둘 다 말했거나 아무것도 안 말했으면 건드리지 않는다
        let mut args = serde_json::json!({"resize_height": 800});
        fix_resize_axis("가로 800 세로 600으로", "image_transform", &mut args);
        assert_eq!(args["resize_height"], 800);
        let mut args = serde_json::json!({"resize_height": 800});
        fix_resize_axis("800으로 줄여줘", "image_transform", &mut args);
        assert_eq!(args["resize_height"], 800);
    }

    /// 사용자가 말한 출력 파일명(.pdf/.zip)을 모델이 생략하면 주입한다 (2026-06-12 R7)
    #[test]
    fn named_output_is_injected_when_model_omits_it() {
        let mut args = serde_json::json!({"dir": "C:/ws"});
        inject_named_output("이미지들 묶어서 album.pdf로 만들어줘", "images_to_pdf", &mut args);
        assert_eq!(args["output_path"], "album.pdf");

        // 모델이 이미 지정했으면 존중
        let mut args = serde_json::json!({"dir": "C:/ws", "output_path": "C:/ws/모음.pdf"});
        inject_named_output("이미지들 묶어서 album.pdf로", "images_to_pdf", &mut args);
        assert_eq!(args["output_path"], "C:/ws/모음.pdf");

        // 발화의 .zip 토큰이 입력 인자에 이미 있으면 출력 의도가 아니다
        let mut args = serde_json::json!({"paths": "C:/ws/photos.zip"});
        inject_named_output("photos.zip 다시 압축해줘", "zip_create", &mut args);
        assert!(args.get("output_path").is_none());

        // 파일명 토큰이 없으면 주입하지 않는다
        let mut args = serde_json::json!({"paths": "C:/ws/a.png"});
        inject_named_output("zip으로 압축해줘", "zip_create", &mut args);
        assert!(args.get("output_path").is_none());
    }

    #[test]
    fn filename_extraction_handles_korean_and_particles() {
        assert_eq!(filename_with_ext("앨범사진.pdf로 만들어줘", "pdf").as_deref(), Some("앨범사진.pdf"));
        assert_eq!(filename_with_ext("result-1.zip 으로 묶어", "zip").as_deref(), Some("result-1.zip"));
        assert_eq!(filename_with_ext("pdf로 만들어줘", "pdf"), None);
        assert_eq!(filename_with_ext("a.pdfx 처리해", "pdf"), None);
    }

    #[test]
    fn synthesize_question_extracts_filename() {
        let err = "오류: 파일 없음: C:/ws/발표자료.pdf. 주변 폴더에서도 같은 이름을 찾지 못했습니다. \
                   다른 경로를 추측해 재시도하지 말고, 사용자에게 파일의 정확한 위치(폴더)를 물어보세요.";
        let msgs = vec![ChatMessage::user("발표자료.pdf 요약해줘"), ChatMessage::tool("c1", err)];
        let q = synthesize_location_question(&msgs).unwrap();
        assert!(q.contains("발표자료.pdf"), "{q}");
        assert!(q.contains("알려주"), "{q}");
        // 마커 없는 이력에서는 합성하지 않는다
        assert!(synthesize_location_question(&[ChatMessage::tool("c2", "오류: 기타")]).is_none());
    }

    /// 모델이 인자 파일명을 환각한 경우(사용자 발화에 없는 이름) 환각명을 질문에
    /// 노출하지 않는다 (2026-06-12 R2 실측: '발표자료.pdf' → 'final_report.pdf' 영문화)
    #[test]
    fn synthesize_question_hides_hallucinated_filename() {
        let err = "오류: 파일 없음: C:/ws/final_report.pdf. 주변 폴더에서도 같은 이름을 찾지 못했습니다. \
                   다른 경로를 추측해 재시도하지 말고, 사용자에게 파일의 정확한 위치(폴더)를 물어보세요.";
        let msgs = vec![ChatMessage::user("발표자료.pdf 요약해줘"), ChatMessage::tool("c1", err)];
        let q = synthesize_location_question(&msgs).unwrap();
        assert!(!q.contains("final_report"), "환각 파일명 노출: {q}");
        assert!(q.contains("알려주") || q.contains("?"), "{q}");
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

    /// 모델이 도구 호출 문법을 본문 텍스트로 뱉으면(서버 파싱 실패) 답변에서 걷어낸다.
    /// (2026-06-12 적대 테스트: 강제 마무리 후 "<tool_call> <function=search_files>..." 가
    ///  사용자 답변으로 그대로 노출됨)
    #[tokio::test(flavor = "multi_thread")]
    async fn tool_markup_suffix_is_stripped_from_answer() {
        let client = MockClient::ok(vec![CompletionResult {
            content: "검색 결과가 없습니다. <tool_call> <function=search_files> <parameter=pattern>".into(),
            ..Default::default()
        }]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("유니콘.png 찾아줘")];
        let events = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|ev| {
            events.lock().unwrap().push(ev)
        })
        .await
        .unwrap();

        let answer = messages.last().unwrap().content.as_deref().unwrap();
        assert_eq!(answer, "검색 결과가 없습니다.");
        // 저장뿐 아니라 스트리밍 델타에도 마크업이 새면 안 된다 (2026-06-12 기획자 테스트)
        for ev in events.lock().unwrap().iter() {
            if let AgentEvent::TextDelta { delta, .. } = ev {
                assert!(!delta.contains("<tool_call"), "스트림 누출: {delta}");
                assert!(!delta.contains("<function="), "스트림 누출: {delta}");
            }
        }
    }

    /// 마크업 스트림 가드: 마커 앞 텍스트만 방출하고, 경계에 걸친 마커도 잡는다
    #[test]
    fn markup_stream_guard_blocks_split_markers() {
        // 한 델타에 통째로 온 마커
        let mut g = MarkupStreamGuard::default();
        assert_eq!(g.push("답변입니다. <tool_call> <function=x>"), "답변입니다.");
        assert_eq!(g.push(" 더 많은 마크업"), "");
        assert_eq!(g.finish(), "");

        // 델타 경계에서 쪼개진 마커 ("<tool" + "_call>")
        let mut g = MarkupStreamGuard::default();
        let first = g.push("안녕하세요 <tool");
        assert!(first.starts_with("안녕하세요"), "{first}");
        assert!(!first.contains("<tool"), "마커 접두사는 보류돼야 함: {first}");
        assert_eq!(g.push("_call> <function=list_dir>"), "");
        assert_eq!(g.finish(), "", "차단 후 꼬리 방출 금지");

        // 마커가 아닌 '<' 는 finish 에서 방출된다
        let mut g = MarkupStreamGuard::default();
        let out = g.push("a < b 입니다");
        let tail = g.finish();
        assert_eq!(format!("{out}{tail}"), "a < b 입니다");
        // finish 후 재사용 가능 (다음 완성)
        assert_eq!(g.push("다음 답변"), "다음 답변");
    }

    /// 합성 실패 노트는 도구 오류 속 모델용 지시문을 인용하지 않는다
    /// (2026-06-12 기획자 테스트: "...— 이 사실을 그대로 사용자에게 알리세요"가 노출)
    #[test]
    fn failure_note_strips_model_directives() {
        let msgs = vec![ChatMessage::tool(
            "c1",
            "오류: 이 파일은 PDF가 아니라 이미지(.gif)입니다. 이미지 속 글자를 읽는 기능(OCR)은 \
             없습니다 — 이 사실을 그대로 사용자에게 알리세요.",
        )];
        let note = synthesize_failure_note(&msgs, false).unwrap();
        assert!(note.contains("OCR"), "{note}");
        assert!(!note.contains("알리세요"), "지시문 노출: {note}");
        assert!(!note.contains("이 사실을"), "지시문 노출: {note}");
    }

    /// 본문 전체가 도구 마크업이면 빈 완성으로 취급해 재시도 경로를 태운다
    #[tokio::test(flavor = "multi_thread")]
    async fn pure_tool_markup_answer_is_retried_as_empty() {
        let client = MockClient::ok(vec![
            CompletionResult {
                content: "<tool_call> <function=list_dir> </function>".into(),
                ..Default::default()
            },
            CompletionResult { content: "폴더가 비어 있습니다.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("뭐 있어?")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        assert_eq!(messages.last().unwrap().content.as_deref(), Some("폴더가 비어 있습니다."));
        assert!(
            !messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("<tool_call"))),
            "마크업이 이력에 남으면 안 됨"
        );
    }

    /// 이름변경 의도에서 write_file 로 새는 대체 행동을 차단한다
    /// (2026-06-12 실로그: "이미지들 이름을 생성시간으로 전부 변경해봐" → write_file 로
    ///  변경 목록 txt 를 만들고 "변경 완료"라고 거짓 보고. 사용자 추궁에도 반복.)
    #[test]
    fn rename_intent_excludes_write_file() {
        for t in [
            "이미지들 이름을 생성시간으로 전부 변경해봐",
            "이름을 cat1,2,3 으로 변경해봐",
            "그 파일 이름을 회의메모.txt로 바꿔",
        ] {
            assert!(tools_to_exclude(t).contains(&"write_file"), "{t}");
            // 변환-사본(image_transform)으로 이름변경을 때우는 대체 행동도 차단
            // (2026-06-12 실로그: cat.png → cat_2026-06-12.png 사본 생성 후 "변경 완료" 주장)
            assert!(tools_to_exclude(t).contains(&"image_transform"), "{t}");
        }
        // 받아쓰기("라고")가 섞인 복합 요청은 쓰기가 본업 — write_file 을 숨기면 안 된다
        assert!(
            !tools_to_exclude("note.txt에 '안녕'이라고 적고, 파일 이름을 인사.txt로 바꿔줘")
                .contains(&"write_file"),
        );
        // 사용자 호칭 이야기는 이름변경 의도가 아니다
        assert!(!tools_to_exclude("내 이름은 태경이야. 기억해줘").contains(&"write_file"));
    }

    /// 완료 주장은 도구의 성공 결과에 근거해야 한다는 규칙이 프롬프트에 있어야 한다
    /// (2026-06-12 실로그: list_dir 가 그대로인 파일명을 보여줘도 "변경 완료" 주장)
    #[test]
    fn prompt_has_claim_grounding_rule() {
        let p = system_prompt(&AppConfig::default());
        assert!(p.contains("성공 결과를 받았을 때만"), "완료 주장 근거 규칙 누락");
    }

    /// 파일 상태 추론(stale parroting 방지) + 검색 범위 규칙이 프롬프트에 있어야 한다
    /// (2026-06-12 적대 테스트: '방금 바꾼 파일'에 옛 경로 복제, 못 찾자 Desktop 무단 검색)
    #[test]
    fn prompt_has_file_state_and_search_scope_rules() {
        let p = system_prompt(&AppConfig::default());
        assert!(p.contains("옛 경로"), "이동/이름변경 후 옛 경로 무효 규칙 누락");
        assert!(p.contains("그대로 복사하지"), "이전 도구 호출 복제 금지 규칙 누락");
        assert!(p.contains("위치를 묻는다"), "검색 범위 이탈 방지 규칙 누락");
    }

    /// 같은 호출이 거절된 도구는 그 턴의 다음 라운드부터 스키마에서 숨긴다.
    /// 2B 는 보이는 도구를 계속 베끼므로 베낄 대상을 치워야 다음 단계로 간다.
    /// (2026-06-12 실로그: zip 해제+이름변경 복합 요청 — zip_extract 성공 후 같은 호출만
    ///  반복하다 강제중단, rename_file 까지 가지 못함)
    #[tokio::test(flavor = "multi_thread")]
    async fn duplicated_tool_is_hidden_from_next_round() {
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": dir.path().to_string_lossy()});
        let client = MockClient::ok(vec![
            tool_call_result("list_dir", args.clone()),
            tool_call_result("list_dir", args.clone()), // 중복 → 거절 + 이후 라운드에서 숨김
            CompletionResult { content: "폴더 정리 끝.".into(), ..Default::default() },
        ]);
        let registry = ToolRegistry::with_default_tools();
        let mut messages = vec![ChatMessage::user("정리해줘")];
        let cancel = AtomicBool::new(false);

        run_turn(&client, &registry, &noop_ctx(), &mut messages, "s1", 8, 0.7, &cancel, &|_| {})
            .await
            .unwrap();

        let names = client.tool_names.lock().unwrap().clone();
        assert!(names[0].iter().any(|n| n == "list_dir"), "첫 라운드엔 제공");
        assert!(names[1].iter().any(|n| n == "list_dir"), "중복 발생 전까지 제공");
        assert!(
            !names[2].iter().any(|n| n == "list_dir"),
            "중복 거절 후엔 스키마에서 숨겨야 함"
        );
        assert!(names[2].iter().any(|n| n == "rename_file"), "다른 도구는 그대로");
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
            tool_names: Mutex::new(vec![]),
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
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("작업방");
        std::fs::create_dir(&ws).unwrap();
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let p = system_prompt(&cfg);
        assert!(p.contains("작업방"), "워크스페이스 경로가 프롬프트에 없음");
        assert!(p.contains("워크스페이스 안에서만"));
    }

    /// 경로 없는 이름은 워크스페이스 기준이라는 해석 규칙이 프롬프트 상단에 있어야 한다
    #[test]
    fn prompt_defaults_bare_names_to_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("작업방");
        std::fs::create_dir(&ws).unwrap();
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let p = system_prompt(&cfg);
        let ws_slash = ws.to_string_lossy().replace('\\', "/");
        assert!(
            p.contains(&format!("현재 폴더(워크스페이스): {ws_slash}")),
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
        // update_profile 도구는 제거됨 (파일 이름변경과 오라우팅, 2026-06-12) — 프롬프트에 흔적이 없어야 한다
        assert!(!p.contains("update_profile"), "제거된 도구가 프롬프트에 남음");
        assert!(p.contains("지어달라고"), "이름 지어달라는 지시 없음");
        assert!(p.contains("설정"), "이름 저장 경로(설정 패널) 안내 없음");
    }

    /// 설정에 '태경님'처럼 님까지 저장돼 있어도 이중 호칭(태경님님)이 되지 않는다
    /// (2026-06-12 실로그: 제거된 update_profile 이 남긴 user_name="태경님" → "태경님님")
    #[test]
    fn prompt_strips_trailing_honorific_from_user_name() {
        let mut cfg = AppConfig::default();
        cfg.user_name = "태경님".into();
        cfg.agent_name = "쫄병".into();
        let p = system_prompt(&cfg);
        assert!(p.contains("'태경'"), "님을 뗀 이름이어야 함: {p}");
        assert!(!p.contains("태경님님"), "{p}");
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
        for t in ["dog.png를 배경제거 해봐", "이 사진 누끼 따줘", "배경을 빼서 투명하게"] {
            assert!(tools_to_exclude(t).contains(&"image_transform"), "{t}");
        }
        assert!(!tools_to_exclude("dog.png를 90도 회전시켜줘").contains(&"image_transform"));
        assert!(
            !tools_to_exclude("배경화면 바꿔줘").contains(&"image_transform"),
            "배경화면은 배경제거가 아님"
        );
    }

    /// 받아쓰기 쓰기 의도는 읽기/탐색 도구를 숨기고, 복합·읽기 질의는 건드리지 않는다
    #[test]
    fn dictation_write_excludes_read_tools() {
        let read_tools = ["read_file", "list_dir", "search_files", "pdf_extract_text", "move_path", "rename_file", "copy_path", "delete_path"];
        // GT 실패 5건 전부 라우팅돼야 한다
        for t in [
            "todo.md에 '장보기' 라고 적어줘",
            "minutes.txt에 \"회의 요약: 배포 일정 확정\"이라고 저장해줘", // 따옴표 안 '요약'은 읽기 단서 아님
            "contacts.csv에 \"이름,전화번호\"라고 기록해줘",
            "plan.md에 \"보고서 작성, 메일 회신\"이라고 작성해줘",
            "idea.txt에 \"신제품 마케팅 아이디어\"라고 적어줘",
        ] {
            let ex = tools_to_exclude(t);
            for r in read_tools {
                assert!(ex.contains(&r), "{t} 에서 {r} 제외 누락");
            }
            assert!(!ex.contains(&"write_file"), "{t}");
        }

        // 멀티스텝(읽기→요약→쓰기)과 읽기 질의는 라우팅하면 안 된다
        for t in [
            "report.md를 읽고 요약해서 summary.md에 저장해줘",
            "guide.md에 뭐라고 적혀 있어?",
            "로그 내용을 정리해서 result.txt라고 저장해줘",
            "todo.md 적힌 거 보여줘",
        ] {
            assert!(
                !tools_to_exclude(t).iter().any(|n| read_tools.contains(n)),
                "{t}"
            );
        }
    }

    /// 압축 풀기 의도에선 zip_create 를 숨긴다
    /// (2026-06-12 실로그: 풀 zip 이 없자 zip_create 로 새 zip 을 만드는 대체 행동)
    #[test]
    fn extract_intent_excludes_zip_create() {
        let compound = tools_to_exclude("pngs.zip 압축풀고 거기에 있는 이미지 파일들 오늘날짜로 이름 변경해줘");
        assert!(compound.contains(&"zip_create"), "풀기 의도에서 zip_create 숨김");
        assert!(compound.contains(&"write_file"), "이름변경 의도와 합성돼야 함");
        assert!(tools_to_exclude("백업.zip 압축 해제해줘").contains(&"zip_create"));
        // 압축 생성 의도는 제외하지 않는다
        assert!(!tools_to_exclude("이미지들 압축해줘").contains(&"zip_create"));
    }

    /// 삭제 의도에선 zip_extract 를 숨긴다 (2026-06-12 실로그: "압축파일 모두 지워봐"에
    /// 모델이 '압축' 토큰에 끌려 폴더를 zip_extract — delete_path 시도조차 안 함)
    #[test]
    fn delete_intent_excludes_zip_extract() {
        assert!(tools_to_exclude("현재 워크스페이스에 있는 압축파일 모두 지워봐").contains(&"zip_extract"));
        assert!(tools_to_exclude("cat_nobg.zip 삭제해줘").contains(&"zip_extract"));
        // 풀기 의도가 함께 있으면 풀기가 본업 — 숨기면 안 된다
        assert!(!tools_to_exclude("압축 풀고 원본 압축파일은 지워줘").contains(&"zip_extract"));
        // 압축 생성 복합("압축해서 원본 지워줘")을 위해 zip_create 는 건드리지 않는다
        assert!(!tools_to_exclude("이미지들 압축해서 원본은 지워줘").contains(&"zip_create"));
    }

    /// screen_capture 는 화면/캡처를 직접 언급한 턴에만 노출한다
    /// (2026-06-12 실로그: "귀여워" 잡담에 4K 화면 캡처 발사 — 159초 + 섬뜩한 UX)
    #[test]
    fn screen_capture_hidden_unless_mentioned() {
        assert!(tools_to_exclude("귀여워").contains(&"screen_capture"));
        assert!(tools_to_exclude("이미지들 압축해줘").contains(&"screen_capture"));
        assert!(!tools_to_exclude("화면 캡처해줘").contains(&"screen_capture"));
        assert!(!tools_to_exclude("지금 스크린샷 찍어줘").contains(&"screen_capture"));
        assert!(!tools_to_exclude("화면 찍어서 저장해").contains(&"screen_capture"));
    }

    /// set_workspace 는 사용자가 워크스페이스를 직접 언급한 턴에만 노출한다.
    /// 부작용이 턴을 넘어 지속되는 유일한 도구 — 오발사 비용이 가장 크다.
    /// (2026-06-12 실로그: 모델이 임의 호출 → 다음 턴의 경로 해석 전부 붕괴)
    #[test]
    fn set_workspace_hidden_unless_mentioned() {
        assert!(tools_to_exclude("이미지 파일들 보여줘").contains(&"set_workspace"));
        assert!(tools_to_exclude("이름을 cat1로 바꿔").contains(&"set_workspace"));
        assert!(!tools_to_exclude("워크스페이스를 pngs 폴더로 바꿔줘").contains(&"set_workspace"));
        assert!(!tools_to_exclude("작업 폴더를 바탕화면으로 변경해").contains(&"set_workspace"));
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
        // 능력 규칙(11)만 검사 — 12 이후는 별개 규칙(파일 상태/검색 범위)이라 부정어가 정당하다
        let end = p.find("12.").or_else(|| p.find("페르소나")).unwrap_or(p.len());
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

        let tmp_ws = tempfile::tempdir().unwrap();
        let ws = tmp_ws.path().join("작업방");
        std::fs::create_dir(&ws).unwrap();
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
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
    /// 이력이 [assistant(tool_calls) → tool] 쌍으로 닫혀야 한다 ("한 턴 밀림" 회귀 방지).
    /// 새 정책에서 한도(hard cap)는 성공이 이어질 때만 도달하므로 성공 호출로 채운다.
    #[tokio::test(flavor = "multi_thread")]
    async fn round_limit_closes_dangling_tool_calls() {
        let dir = tempfile::tempdir().unwrap();
        // max=2 → hard cap 6: 성공 호출 7개로 상한을 넘긴다
        let responses: Vec<CompletionResult> = (0..7)
            .map(|i| {
                let sub = dir.path().join(format!("c{i}"));
                std::fs::create_dir_all(&sub).unwrap();
                tool_call_result("list_dir", serde_json::json!({"path": sub.to_string_lossy()}))
            })
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
        // 미응답 툴콜 봉합(tool) 뒤에 명시적 종결 노트(assistant)가 온다
        let tool_msgs: Vec<_> = messages.iter().filter(|m| m.role == "tool").collect();
        assert!(tool_msgs.last().unwrap().content.as_deref().unwrap().contains("중단"));
        let last = messages.last().unwrap();
        assert_eq!(last.role, "assistant", "턴은 항상 assistant 종결로 끝난다");
        assert!(last.content.as_deref().unwrap().contains("완료되지 않"));
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
