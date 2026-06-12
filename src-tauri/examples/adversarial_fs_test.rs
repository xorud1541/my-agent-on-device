//! 진단용: 실행 중인 llama-server(8736)에 파일 도구를 적대적 멀티턴으로 괴롭힌다.
//! - 멀티턴 맥락 오염(stale parroting), 이름변경 오라우팅, 없는 파일/충돌 함정,
//!   이동 vs 이름변경 경계, 받아쓰기+이름변경 복합을 검증.
//! 판정은 답변 텍스트가 아니라 샌드박스 파일시스템의 최종 상태로 한다.
//! 사용: cargo run --example adversarial_fs_test
use local_agent_lib::agent::{run_turn, system_prompt};
use local_agent_lib::config::AppConfig;
use local_agent_lib::llm::client::HttpLlmClient;
use local_agent_lib::models::{AgentEvent, ChatMessage};
use local_agent_lib::tools::{ToolCtx, ToolRegistry};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

struct TurnLog {
    calls: Vec<(String, bool)>, // (도구이름, ok)
    answer: String,
    errors: Vec<String>,
}

async fn turn(
    client: &HttpLlmClient,
    registry: &ToolRegistry,
    ctx: &ToolCtx,
    messages: &mut Vec<ChatMessage>,
    user: &str,
) -> TurnLog {
    println!("  USER: {user}");
    messages.push(ChatMessage::user(user.to_string()));
    let events: Mutex<Vec<AgentEvent>> = Mutex::new(Vec::new());
    let started = std::time::Instant::now();
    run_turn(client, registry, ctx, messages, "adv", 8, 0.2, &AtomicBool::new(false), &|ev| {
        match &ev {
            AgentEvent::ToolCallStart { name, arguments, .. } => {
                println!("    CALL {name} {}", arguments.chars().take(110).collect::<String>());
            }
            AgentEvent::ToolCallEnd { name, ok, result, .. } => {
                println!("      -> {name} ok={ok} {}", result.chars().take(90).collect::<String>());
            }
            AgentEvent::Error { message, .. } => println!("    ERROR {message}"),
            _ => {}
        }
        events.lock().unwrap().push(ev);
    })
    .await
    .expect("run_turn 실패");

    let events = events.into_inner().unwrap();
    let calls: Vec<(String, bool)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallEnd { name, ok, .. } => Some((name.clone(), *ok)),
            _ => None,
        })
        .collect();
    let errors: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect();
    let answer = messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && m.content.as_deref().is_some_and(|c| !c.is_empty()))
        .and_then(|m| m.content.clone())
        .unwrap_or_default();
    println!(
        "    ANSWER({}s): {}",
        started.elapsed().as_secs(),
        answer.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(140).collect::<String>()
    );
    TurnLog { calls, answer, errors }
}

fn sandbox(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("la-adversarial").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn check(results: &mut Vec<(String, bool)>, label: &str, ok: bool) {
    println!("  {} {label}", if ok { "✅" } else { "❌" });
    results.push((label.to_string(), ok));
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let client = HttpLlmClient::new("http://127.0.0.1:8736".into(), 1024);
    let registry = ToolRegistry::with_default_tools();
    let mut results: Vec<(String, bool)> = Vec::new();

    // ── S1: 어제(2026-06-12 #9) 실패 재현 — 멀티턴 stale parroting ──────────
    println!("\n━━ S1: 이름변경 후 '방금 바꾼 파일' 참조 (stale parroting) ━━");
    {
        let ws = sandbox("s1");
        std::fs::write(ws.join("cat.png"), "img")?;
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg.clone());
        let mut msgs = vec![ChatMessage::system(system_prompt(&cfg))];

        let t1 = turn(&client, &registry, &ctx, &mut msgs, "cat.png 이름을 '새 파일 1.png'로 바꿔줘").await;
        check(&mut results, "S1-t1 rename_file 사용", t1.calls.iter().any(|(n, ok)| n == "rename_file" && *ok));
        check(&mut results, "S1-t1 파일 상태", ws.join("새 파일 1.png").exists() && !ws.join("cat.png").exists());

        let t2 = turn(&client, &registry, &ctx, &mut msgs, "오 잘하는데?").await;
        check(&mut results, "S1-t2 잡담에 도구 안 씀", t2.calls.is_empty());

        // 어제 실패 지점: 모델이 t1의 cat.png 호출을 복제하면 원본이 없어 실패한다
        let t3 = turn(&client, &registry, &ctx, &mut msgs, "방금 바꾼 파일을 새파일.png로 이름 변경해봐").await;
        check(&mut results, "S1-t3 최종 파일 새파일.png 존재", ws.join("새파일.png").exists());
        check(&mut results, "S1-t3 한도초과/루프 없음", t3.errors.is_empty());
    }

    // ── S2: 오라우팅 — '이름변경' 단어 + 사용자 호칭 기억 요청 ──────────────
    println!("\n━━ S2: '이름변경' 오라우팅 + 제거된 update_profile 유령 호출 ━━");
    {
        let ws = sandbox("s2");
        std::fs::write(ws.join("dog.png"), "img")?;
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg.clone());
        let mut msgs = vec![ChatMessage::system(system_prompt(&cfg))];

        // 어제 #5 재현: 능력 질문에 update_profile 을 쏘던 케이스 (이제 도구 자체가 없음)
        let t1 = turn(&client, &registry, &ctx, &mut msgs, "이름변경도 가능해?").await;
        check(&mut results, "S2-t1 실패한 도구 호출 없음", t1.calls.iter().all(|(_, ok)| *ok));

        // 유령 호출 유도: 모델이 학습된 update_profile 패턴을 뱉으면 실행 단계에서 거부된다
        let t2 = turn(&client, &registry, &ctx, &mut msgs, "내 이름은 태경이야. 기억해줘").await;
        check(&mut results, "S2-t2 update_profile 유령 호출 없음", !t2.calls.iter().any(|(n, _)| n == "update_profile"));

        let t3 = turn(&client, &registry, &ctx, &mut msgs, "그럼 dog.png 이름을 멍멍이.png로 바꿔").await;
        check(&mut results, "S2-t3 rename 동작", ws.join("멍멍이.png").exists() && !ws.join("dog.png").exists());
    }

    // ── S3: 함정 — 없는 파일, 이미 있는 대상 ────────────────────────────────
    println!("\n━━ S3: 없는 파일 / 대상 충돌 함정 ━━");
    {
        let ws = sandbox("s3");
        std::fs::write(ws.join("a.txt"), "1")?;
        std::fs::write(ws.join("b.txt"), "2")?;
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg.clone());
        let mut msgs = vec![ChatMessage::system(system_prompt(&cfg))];

        let t1 = turn(&client, &registry, &ctx, &mut msgs, "유니콘.png를 말.png로 이름 바꿔줘").await;
        check(&mut results, "S3-t1 없는 파일: 한도초과 없이 종료", t1.errors.is_empty());
        check(&mut results, "S3-t1 거짓 성공 주장 없음", !t1.answer.contains("변경 완료") || !t1.calls.iter().any(|(_, ok)| *ok));

        let t2 = turn(&client, &registry, &ctx, &mut msgs, "a.txt를 b.txt로 바꿔봐").await;
        let b_intact = std::fs::read_to_string(ws.join("b.txt")).unwrap_or_default() == "2";
        check(&mut results, "S3-t2 기존 b.txt 미파괴", b_intact);
        check(&mut results, "S3-t2 한도초과/루프 없음", t2.errors.is_empty());
    }

    // ── S4: 이동 vs 이름변경 경계 ───────────────────────────────────────────
    println!("\n━━ S4: 이동 vs 이름변경 경계 (의도 비틀기) ━━");
    {
        let ws = sandbox("s4");
        std::fs::write(ws.join("report.txt"), "r")?;
        std::fs::create_dir_all(ws.join("backup"))?;
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg.clone());
        let mut msgs = vec![ChatMessage::system(system_prompt(&cfg))];

        let t1 = turn(&client, &registry, &ctx, &mut msgs, "report.txt를 backup 폴더로 옮겨줘").await;
        check(&mut results, "S4-t1 move_path 선택", t1.calls.iter().any(|(n, ok)| n == "move_path" && *ok));
        check(&mut results, "S4-t1 파일 위치", ws.join("backup").join("report.txt").exists());

        // 짓궂은 표현: "폴더로 이름 바꿔" — rename_file 은 구분자를 거부하므로
        // 모델이 move_path 로 복구하거나, 같은 폴더 내 이름변경으로 해석해야 한다
        let t2 = turn(&client, &registry, &ctx, &mut msgs, "방금 옮긴 파일을 final이라는 이름으로 바꿔줘. 확장자는 유지하고").await;
        check(&mut results, "S4-t2 final.txt 존재", ws.join("backup").join("final.txt").exists());
        check(&mut results, "S4-t2 한도초과/루프 없음", t2.errors.is_empty());
    }

    // ── S5: 받아쓰기 + 이름변경 복합 (dictation 라우팅이 rename_file 을 숨기는 턴) ──
    println!("\n━━ S5: 받아쓰기 쓰기 + 이름변경 복합 ━━");
    {
        let ws = sandbox("s5");
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg.clone());
        let mut msgs = vec![ChatMessage::system(system_prompt(&cfg))];

        let t1 = turn(&client, &registry, &ctx, &mut msgs, "memo.txt에 '회의 3시'라고 적어줘").await;
        check(&mut results, "S5-t1 write_file 성공", ws.join("memo.txt").exists());

        let t2 = turn(&client, &registry, &ctx, &mut msgs, "그 파일 이름을 회의메모.txt로 바꿔").await;
        check(&mut results, "S5-t2 rename 성공", ws.join("회의메모.txt").exists() && !ws.join("memo.txt").exists());
        check(&mut results, "S5-t2 한도초과/루프 없음", t2.errors.is_empty());

        // 복합 한 턴: dictation 라우터가 이 턴에 rename_file 을 숨긴다 — 어떻게 동작하나?
        let t3 = turn(&client, &registry, &ctx, &mut msgs, "note.txt에 '안녕'이라고 적고, 파일 이름을 인사.txt로 바꿔줘").await;
        let wrote = ws.join("note.txt").exists() || ws.join("인사.txt").exists();
        check(&mut results, "S5-t3 쓰기는 수행됨", wrote);
        println!("  (관찰) S5-t3 인사.txt 존재={} — dictation 라우팅과 복합 의도의 충돌 관찰용", ws.join("인사.txt").exists());
    }

    // ── S6: 일괄 이름변경 — write_file 대체 행동 + 거짓 완료 보고 (2026-06-12 실로그 재현) ──
    println!("\n━━ S6: 일괄 이름변경 거짓 보고 (write_file 대체 행동) ━━");
    {
        let ws = sandbox("s6");
        for n in ["cat.png", "dog.png", "woman.png"] {
            std::fs::write(ws.join(n), "img")?;
        }
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg.clone());
        let mut msgs = vec![ChatMessage::system(system_prompt(&cfg))];

        // 실로그 #6 재현: write_file 로 목록 txt 를 만들고 "변경 완료"라고 거짓말하던 발화
        let t1 = turn(&client, &registry, &ctx, &mut msgs, "이미지들 이름을 cat1, cat2, cat3 으로 변경해봐").await;
        check(&mut results, "S6-t1 write_file 대체 행동 없음", !t1.calls.iter().any(|(n, _)| n == "write_file"));
        check(&mut results, "S6-t1 rename_file 실사용", t1.calls.iter().any(|(n, ok)| n == "rename_file" && *ok));
        let renamed = ["cat1.png", "cat2.png", "cat3.png", "cat1", "cat2", "cat3"]
            .iter()
            .filter(|n| ws.join(n).exists())
            .count();
        println!("  (관찰) 실제 변경된 파일 수: {renamed}/3");

        // 실로그 #3 재현: 추궁했을 때 도구 결과 없이 "변경됐다" 반복하는지
        let t2 = turn(&client, &registry, &ctx, &mut msgs, "이름 변경했어? 진짜로?").await;
        let claims_done = t2.answer.contains("변경되었") || t2.answer.contains("변경했");
        check(
            &mut results,
            "S6-t2 거짓 완료 주장 없음",
            renamed > 0 || !claims_done,
        );
    }

    // ── S7: 압축 해제 + 일괄 이름변경 복합 (2026-06-12 앱 실패 재현) ─────────
    // 실로그: zip_extract 성공 후 같은 호출만 베끼다 강제중단 — rename 까지 못 감
    println!("\n━━ S7: 압축 해제 후 이름변경 복합 (루프 도구 동적 숨김 검증) ━━");
    {
        let ws = sandbox("s7");
        let f = std::fs::File::create(ws.join("archive.zip"))?;
        let mut zw = zip::ZipWriter::new(f);
        use std::io::Write as _;
        for (n, c) in [("photo.png", "img"), ("note.txt", "txt")] {
            zw.start_file(n, zip::write::SimpleFileOptions::default())?;
            zw.write_all(c.as_bytes())?;
        }
        zw.finish()?;
        let mut cfg = AppConfig::default();
        cfg.workspace_dir = ws.to_string_lossy().into_owned();
        let ctx = ToolCtx::noop(cfg.clone());
        let mut msgs = vec![ChatMessage::system(system_prompt(&cfg))];

        let t1 = turn(&client, &registry, &ctx, &mut msgs,
            "압축된 파일 풀어서 거기 있는 파일들 이름을 오늘날짜로 이름 변경해줘").await;
        check(&mut results, "S7-t1 압축 해제 수행", t1.calls.iter().any(|(n, ok)| n == "zip_extract" && *ok));
        check(&mut results, "S7-t1 rename_file 까지 도달", t1.calls.iter().any(|(n, ok)| n == "rename_file" && *ok));
        check(&mut results, "S7-t1 강제중단 없음", t1.errors.is_empty());
        // 해제 폴더 안에 원래 이름이 아닌 파일이 생겼는지 (날짜 형식은 모델 재량으로 둔다)
        let extracted = ws.join("archive");
        let renamed_any = std::fs::read_dir(&extracted)
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .any(|e| {
                        let n = e.file_name().to_string_lossy().into_owned();
                        n != "photo.png" && n != "note.txt"
                    })
            })
            .unwrap_or(false);
        check(&mut results, "S7-t1 실제 이름이 바뀐 파일 존재", renamed_any);
    }

    // ── 결과 요약 ────────────────────────────────────────────────────────────
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let pass = results.iter().filter(|(_, ok)| *ok).count();
    for (label, ok) in &results {
        println!("{} {label}", if *ok { "PASS" } else { "FAIL" });
    }
    println!("총 {pass}/{}", results.len());
    Ok(())
}
