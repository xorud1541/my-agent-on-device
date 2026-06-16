# 워크스페이스 인지형 빈 화면 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 빈 화면이 현재 워크스페이스 내용에 grounded된 결정적 맞춤 제안을 보여주고, 폴더 미지정/빈 폴더는 폴더 선택으로 유도한다.

**Architecture:** 읽기 전용 Rust 커맨드 `workspace_summary`가 워크스페이스를 1-depth 스캔해 타입별 개수·플래그·**결정적 제안 문자열**을 반환한다(제안 생성 로직을 백엔드에 둬 Rust 단위테스트로 검증 — 프론트 테스트 러너 없음). 프론트는 플래그로 상태(①/①'/②)를 분기해 렌더만 한다. 제안 클릭은 입력창을 채운다(자동 실행 안 함). system_prompt에 범위 밖 요청 시 솔직 거절+대안 규칙을 추가하고 bench로 회귀 검증한다.

**Tech Stack:** Rust + Tauri 2 (백엔드), React 19 + TypeScript (프론트), `tempfile`(Rust 테스트), `bench/toolcall_test.mjs`(2B 회귀).

---

## File Structure

- Create: `src-tauri/src/workspace_summary.rs` — `WorkspaceSummary` 구조체 + 순수 `summarize()` + 단위테스트
- Modify: `src-tauri/src/lib.rs` — `mod workspace_summary;` 선언 + `generate_handler`에 커맨드 등록
- Modify: `src-tauri/src/commands.rs` — `#[tauri::command] workspace_summary` 얇은 래퍼
- Modify: `src-tauri/src/agent.rs:50-52` — system_prompt 규칙 16(범위 밖 거절+대안) + 프롬프트 테스트
- Modify: `src/types.ts` — `WorkspaceSummary` 인터페이스
- Modify: `src/hooks/useAgent.ts` — summary 조회/구독 + 노출
- Modify: `src/App.tsx` — 빈 화면 상태머신(①/①'/②)
- Modify: `src/components/Composer.tsx` — `prefill` prop(제안 클릭 시 입력창 채우기)

---

## Task 1: 백엔드 `workspace_summary` 순수 로직 + 단위 테스트

**Files:**
- Create: `src-tauri/src/workspace_summary.rs`

- [ ] **Step 1: 실패하는 테스트 작성**

`src-tauri/src/workspace_summary.rs` 파일을 생성하고 아래 전체를 작성한다 (구현 `summarize`는 아직 비어 컴파일/테스트가 실패하도록 `todo!()` 사용):

```rust
//! 워크스페이스를 1-depth 스캔해 타입별 개수 + 결정적 맞춤 제안을 만든다.
//! 제안 생성 로직을 여기(백엔드)에 둬 단위 테스트로 검증한다(프론트 테스트 러너 없음).

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize, PartialEq)]
pub struct WorkspaceSummary {
    pub workspace_dir: String,
    pub folder_name: String,
    pub is_default_home: bool,
    pub is_empty: bool,
    pub images: u32,
    pub pdfs: u32,
    pub zips: u32,
    pub others: u32,
    pub removebg_available: bool,
    /// 상태 ①(폴더 지정 + 파일 있음)에서만 채운다. ②/①' 에서는 빈 목록.
    pub suggestions: Vec<String>,
}

/// 확장자로 파일을 분류한 개수. 하위 폴더는 세지 않는다(1-depth, 파일만).
#[derive(Default, PartialEq, Debug)]
struct Counts {
    images: u32,
    pdfs: u32,
    zips: u32,
    others: u32,
}

fn classify(dir: &Path) -> Counts {
    let mut c = Counts::default();
    let Ok(entries) = std::fs::read_dir(dir) else { return c };
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" => c.images += 1,
            "pdf" => c.pdfs += 1,
            "zip" => c.zips += 1,
            _ => c.others += 1,
        }
    }
    c
}

/// 보유 타입에 매핑된 제안만 결정적으로 만든다. 배경제거는 모델(.ort)이 있을 때만(막다른 길 방지).
fn build_suggestions(c: &Counts, removebg_available: bool) -> Vec<String> {
    let mut s = Vec::new();
    if c.pdfs >= 1 {
        s.push(format!("PDF {}개에서 텍스트 추출", c.pdfs));
    }
    if c.images >= 1 {
        s.push(format!("이미지 {}장을 PDF 한 권으로 묶기", c.images));
        if removebg_available {
            s.push(format!("사진 {}장 배경 제거하기", c.images));
        }
    }
    if c.zips >= 1 {
        s.push("압축 파일 풀기".to_string());
    }
    s.push("화면 캡처해줘".to_string());
    s
}

/// 순수 함수: 경로들을 받아 요약을 만든다(파일시스템 접근만, Tauri 비의존 → 테스트 가능).
pub fn summarize(workspace_dir: &Path, home_dir: &Path, removebg_model: &Path) -> WorkspaceSummary {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    fn touch(dir: &Path, name: &str) {
        File::create(dir.join(name)).unwrap();
    }

    #[test]
    fn images_only_builds_pdf_and_bg_and_capture() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        let model = tempdir().unwrap();
        let model_path = model.path().join("removeBG.ort");
        File::create(&model_path).unwrap();
        touch(ws.path(), "a.png");
        touch(ws.path(), "b.jpg");

        let s = summarize(ws.path(), home.path(), &model_path);
        assert_eq!(s.images, 2);
        assert!(!s.is_empty);
        assert!(!s.is_default_home);
        assert!(s.removebg_available);
        assert_eq!(
            s.suggestions,
            vec![
                "이미지 2장을 PDF 한 권으로 묶기".to_string(),
                "사진 2장 배경 제거하기".to_string(),
                "화면 캡처해줘".to_string(),
            ]
        );
    }

    #[test]
    fn pdf_only_builds_extract_and_capture() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        touch(ws.path(), "report.pdf");
        let s = summarize(ws.path(), home.path(), Path::new("/none.ort"));
        assert_eq!(s.pdfs, 1);
        assert_eq!(
            s.suggestions,
            vec![
                "PDF 1개에서 텍스트 추출".to_string(),
                "화면 캡처해줘".to_string(),
            ]
        );
    }

    #[test]
    fn no_removebg_model_skips_bg_suggestion() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        touch(ws.path(), "a.png");
        let s = summarize(ws.path(), home.path(), Path::new("/does/not/exist.ort"));
        assert!(!s.removebg_available);
        assert!(!s.suggestions.iter().any(|x| x.contains("배경 제거")));
    }

    #[test]
    fn empty_dir_is_empty_and_no_suggestions() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        let s = summarize(ws.path(), home.path(), Path::new("/none.ort"));
        assert!(s.is_empty);
        assert!(s.suggestions.is_empty());
    }

    #[test]
    fn default_home_detected_and_no_suggestions() {
        let home = tempdir().unwrap();
        touch(home.path(), "a.png"); // 파일이 있어도 홈이면 제안 안 만든다
        let s = summarize(home.path(), home.path(), Path::new("/none.ort"));
        assert!(s.is_default_home);
        assert!(s.suggestions.is_empty());
    }

    #[test]
    fn folder_name_is_last_segment() {
        let ws = tempdir().unwrap();
        let home = tempdir().unwrap();
        let s = summarize(ws.path(), home.path(), Path::new("/none.ort"));
        assert_eq!(s.folder_name, ws.path().file_name().unwrap().to_string_lossy());
    }
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cd src-tauri && cargo test --lib workspace_summary 2>&1 | tail -20`
Expected: 컴파일은 되나 `summarize`가 `todo!()`라 모든 테스트가 panic(FAIL).
(만약 `mod workspace_summary;` 미선언으로 컴파일 안 되면 Task 2 Step 1을 먼저 적용.)

- [ ] **Step 3: `summarize` 구현**

`workspace_summary.rs`의 `summarize`를 아래로 교체:

```rust
pub fn summarize(workspace_dir: &Path, home_dir: &Path, removebg_model: &Path) -> WorkspaceSummary {
    let c = classify(workspace_dir);
    let is_default_home = workspace_dir == home_dir;
    let is_empty = c.images + c.pdfs + c.zips == 0;
    let removebg_available = removebg_model.exists();
    let folder_name = workspace_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| workspace_dir.to_string_lossy().into_owned());
    // 상태 ① 에서만 제안을 만든다. 홈/빈 폴더는 프론트가 폴더 선택 UI 를 보여준다.
    let suggestions = if is_default_home || is_empty {
        Vec::new()
    } else {
        build_suggestions(&c, removebg_available)
    };
    WorkspaceSummary {
        workspace_dir: workspace_dir.to_string_lossy().into_owned(),
        folder_name,
        is_default_home,
        is_empty,
        images: c.images,
        pdfs: c.pdfs,
        zips: c.zips,
        others: c.others,
        removebg_available,
        suggestions,
    }
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cd src-tauri && cargo test --lib workspace_summary 2>&1 | tail -20`
Expected: PASS (6 passed).

- [ ] **Step 5: 커밋**

```bash
git add src-tauri/src/workspace_summary.rs
git commit -m "feat: workspace_summary 순수 로직 + 결정적 제안 생성 (단위테스트)"
```

---

## Task 2: `workspace_summary` Tauri 커맨드 + 등록

**Files:**
- Modify: `src-tauri/src/lib.rs:1-8` (mod 선언), `:46-58` (핸들러 등록)
- Modify: `src-tauri/src/commands.rs` (커맨드 추가)

- [ ] **Step 1: 모듈 선언 추가**

`src-tauri/src/lib.rs` 상단 mod 목록(8행 `pub mod tools;` 아래)에 추가:

```rust
pub mod workspace_summary;
```

- [ ] **Step 2: 커맨드 래퍼 추가**

`src-tauri/src/commands.rs` 맨 아래에 추가 (state에서 config를 읽어 순수 함수 호출):

```rust
/// 빈 화면 디스커버빌리티용 — 현재 워크스페이스를 1-depth 스캔한 요약 + 결정적 제안.
/// 읽기 전용이라 워크스페이스 가드 불필요. 모델 미사용·결정적.
#[tauri::command]
pub fn workspace_summary(
    state: State<'_, AppState>,
) -> crate::workspace_summary::WorkspaceSummary {
    let cfg = state.config.lock().unwrap();
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    crate::workspace_summary::summarize(
        &cfg.workspace_path(),
        &home,
        std::path::Path::new(&cfg.removebg_model),
    )
}
```

- [ ] **Step 3: 핸들러 등록**

`src-tauri/src/lib.rs`의 `generate_handler![ ... ]`(58행 `commands::delete_session,` 뒤)에 추가:

```rust
            commands::delete_session,
            commands::workspace_summary,
```

- [ ] **Step 4: 컴파일 확인**

Run: `cd src-tauri && cargo build 2>&1 | tail -15`
Expected: 에러 없이 빌드 성공(경고 무관).

- [ ] **Step 5: 커밋**

```bash
git add src-tauri/src/lib.rs src-tauri/src/commands.rs
git commit -m "feat: workspace_summary Tauri 커맨드 + 핸들러 등록"
```

---

## Task 3: 프론트 타입 동기화

**Files:**
- Modify: `src/types.ts`

- [ ] **Step 1: `WorkspaceSummary` 인터페이스 추가**

`src/types.ts`의 `ModelEntry` 인터페이스(64행) 아래에 추가 (백엔드 `WorkspaceSummary`와 1:1, snake_case 유지):

```typescript
// 백엔드 workspace_summary::WorkspaceSummary 와 1:1 대응
export interface WorkspaceSummary {
  workspace_dir: string;
  folder_name: string;
  is_default_home: boolean;
  is_empty: boolean;
  images: number;
  pdfs: number;
  zips: number;
  others: number;
  removebg_available: boolean;
  suggestions: string[];
}
```

- [ ] **Step 2: 타입체크 확인**

Run: `pnpm exec tsc --noEmit 2>&1 | tail -10`
Expected: 에러 없음(빈 출력).

- [ ] **Step 3: 커밋**

```bash
git add src/types.ts
git commit -m "feat: WorkspaceSummary 프론트 타입 추가"
```

---

## Task 4: useAgent — 요약 조회/구독 + 노출

**Files:**
- Modify: `src/hooks/useAgent.ts`

- [ ] **Step 1: import에 타입 추가**

`src/hooks/useAgent.ts`의 import 타입 목록(types에서)에 `WorkspaceSummary` 추가:

```typescript
import type {
  AgentEvent,
  AppConfig,
  AssistantMessage,
  ChatMessage,
  ServerStatus,
  UiMessage,
  WorkspaceSummary,
} from "../types";
```

- [ ] **Step 2: summary 상태 + 조회 함수 추가**

`const [config, setConfig] = useState<AppConfig | null>(null);`(24행) 바로 아래에 추가:

```typescript
  // 빈 화면 디스커버빌리티 — 현재 워크스페이스 요약(타입별 개수 + 결정적 제안)
  const [summary, setSummary] = useState<WorkspaceSummary | null>(null);
  const refreshSummary = useCallback(() => {
    invoke<WorkspaceSummary>("workspace_summary").then(setSummary).catch(() => {});
  }, []);
```

- [ ] **Step 3: config-changed 시 요약 갱신**

`useAgent.ts`의 `if (ev.type === "config-changed")` 블록(44-47행)을 아래로 교체 (워크스페이스가 바뀌면 요약도 다시 읽는다):

```typescript
      if (ev.type === "config-changed") {
        setConfig(ev.config);
        invoke<WorkspaceSummary>("workspace_summary").then(setSummary).catch(() => {});
        return;
      }
```

- [ ] **Step 4: 초기 1회 조회 + newChat 시 갱신**

초기 설정 로드 useEffect(118-120행)를 아래로 교체:

```typescript
  // 초기 설정 + 워크스페이스 요약 로드 (이후 변경은 config-changed 가 갱신)
  useEffect(() => {
    invoke<AppConfig>("get_config").then(setConfig).catch(() => {});
    refreshSummary();
  }, [refreshSummary]);
```

그리고 `newChat`(181-187행)의 끝부분 `setBusy(false);` 다음 줄에 추가:

```typescript
    setBusy(false);
    refreshSummary();
```

`newChat`의 의존성 배열도 `[]` → `[refreshSummary]` 로 바꾼다.

- [ ] **Step 5: 반환값에 summary 노출**

`useAgent.ts`의 return(198행)에 `summary` 추가:

```typescript
  return { messages, busy, server, config, summary, send, cancel, newChat, loadSession, sessionId };
```

- [ ] **Step 6: 타입체크 확인**

Run: `pnpm exec tsc --noEmit 2>&1 | tail -10`
Expected: 에러 없음.

- [ ] **Step 7: 커밋**

```bash
git add src/hooks/useAgent.ts
git commit -m "feat: useAgent 워크스페이스 요약 조회/구독 + 노출"
```

---

## Task 5: Composer prefill (제안 클릭 = 입력창 채우기)

**Files:**
- Modify: `src/components/Composer.tsx`

- [ ] **Step 1: Props에 prefill 추가**

`src/components/Composer.tsx`의 `interface Props`(9-14행)에 두 필드 추가:

```typescript
interface Props {
  busy: boolean;
  disabled: boolean;
  onSend: (text: string, attachments: Attachment[]) => void;
  onCancel: () => void;
  /** 제안 칩 클릭 시 입력창에 채울 텍스트. 채운 뒤 onPrefillConsumed 로 비운다. */
  prefill?: string;
  onPrefillConsumed?: () => void;
}
```

- [ ] **Step 2: prefill 적용 effect 추가**

`Composer` 함수에서 `useRef`/`useState` 옆에 `useEffect`를 쓰도록 import를 보강하고(상단 `import { FormEvent, KeyboardEvent, useRef, useState } from "react";` → `useEffect` 추가), 시그니처를 `prefill`/`onPrefillConsumed`를 받도록 바꾼 뒤, `const taRef = useRef...` 아래에 추가:

```typescript
export function Composer({ busy, disabled, onSend, onCancel, prefill, onPrefillConsumed }: Props) {
  // ... 기존 useState 들 ...
  const taRef = useRef<HTMLTextAreaElement>(null);

  // 제안 클릭 → 입력창 채우기(자동 실행 안 함). 사용자가 보고 Enter.
  useEffect(() => {
    if (!prefill) return;
    setText(prefill);
    const ta = taRef.current;
    if (ta) {
      ta.focus();
      ta.style.height = "auto";
      ta.style.height = `${Math.min(ta.scrollHeight, 180)}px`;
    }
    onPrefillConsumed?.();
  }, [prefill, onPrefillConsumed]);
```

- [ ] **Step 3: 타입체크 확인**

Run: `pnpm exec tsc --noEmit 2>&1 | tail -10`
Expected: 에러 없음. (App.tsx가 아직 새 prop을 안 넘겨도 optional이라 통과)

- [ ] **Step 4: 커밋**

```bash
git add src/components/Composer.tsx
git commit -m "feat: Composer prefill prop — 제안 클릭 시 입력창 채우기"
```

---

## Task 6: App.tsx 빈 화면 상태머신

**Files:**
- Modify: `src/App.tsx`

- [ ] **Step 1: 정적 SUGGESTIONS 제거 + 상태 연결**

`src/App.tsx`의 정적 배열(8-13행 `const SUGGESTIONS = [...]`)을 **삭제**한다.
`useAgent()` 구조분해(22-23행)에 `summary` 추가하고, prefill 상태를 둔다:

```typescript
  const { messages, busy, server, config, summary, send, cancel, newChat, loadSession, sessionId } =
    useAgent();
  const [showSettings, setShowSettings] = useState(false);
  const [showSessions, setShowSessions] = useState(true);
  const [draft, setDraft] = useState<string | undefined>(undefined);
  const scrollRef = useRef<HTMLDivElement>(null);
```

- [ ] **Step 2: 폴더 선택 핸들러 추가**

`App` 함수 안, `statusLabel` 계산부(34행) 위에 추가 (상태 ②/①'의 "폴더 선택" 버튼용 — 기존 `pick_folder` + `set_config` 재사용, config-changed가 요약을 갱신):

```typescript
  const pickWorkspace = async () => {
    if (!config) return;
    const dir = await invoke<string | null>("pick_folder", {
      initialDir: config.workspace_dir || null,
    });
    if (dir) await invoke("set_config", { newConfig: { ...config, workspace_dir: dir } });
  };
```

파일 상단에 import 추가: `import { invoke } from "@tauri-apps/api/core";`

- [ ] **Step 3: 빈 화면 렌더를 상태머신으로 교체**

`messages.length === 0 ? ( ... )` 의 빈 화면 블록(91-113행, `<div className="empty-state">...`)을 아래로 교체:

```tsx
            <div className="empty-state">
              <div className="empty-mark">
                LOCAL
                <br />
                <em>AGENT</em>
              </div>

              {summary && !summary.is_default_home && !summary.is_empty ? (
                // 상태 ① — 폴더 + 파일 있음: 요약 + 맞춤 제안
                <>
                  <p className="empty-sub">
                    📁 {summary.folder_name} 폴더에{" "}
                    {[
                      summary.images && `🖼 이미지 ${summary.images}`,
                      summary.pdfs && `📄 PDF ${summary.pdfs}`,
                      summary.zips && `🗜 zip ${summary.zips}`,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </p>
                  <div className="suggestions">
                    {summary.suggestions.map((s) => (
                      <button
                        key={s}
                        className="suggestion"
                        onClick={() => setDraft(s)}
                        disabled={server.status !== "ready"}
                      >
                        {s}
                      </button>
                    ))}
                  </div>
                </>
              ) : (
                // 상태 ② / ①' — 홈/첫 실행 또는 빈 폴더: 폴더 선택 유도
                <>
                  <p className="empty-sub">
                    {summary && !summary.is_default_home && summary.is_empty
                      ? "이 폴더는 비어 있어요. 작업할 폴더를 고르거나 화면을 캡처해 보세요."
                      : "사진 배경 제거·정리, 이미지→PDF, 화면 캡처 같은 일을 이 PC 안에서만 도와드려요. 먼저 작업할 폴더를 골라주세요."}
                  </p>
                  <div className="suggestions">
                    <button className="suggestion" onClick={pickWorkspace}>
                      📁 작업할 폴더 선택
                    </button>
                    <button
                      className="suggestion"
                      onClick={() => setDraft("화면 캡처해줘")}
                      disabled={server.status !== "ready"}
                    >
                      화면 캡처해줘
                    </button>
                  </div>
                </>
              )}
            </div>
```

- [ ] **Step 4: Composer에 prefill 배선**

`<Composer ... />`(120-125행)에 prop 추가:

```tsx
          <Composer
            busy={busy}
            disabled={server.status !== "ready"}
            onSend={send}
            onCancel={cancel}
            prefill={draft}
            onPrefillConsumed={() => setDraft(undefined)}
          />
```

- [ ] **Step 5: 타입체크 + 빌드 확인**

Run: `pnpm build 2>&1 | tail -15`
Expected: `tsc && vite build` 성공(에러 없음).

- [ ] **Step 6: 수동 확인 (앱 구동)**

Run: `pnpm tauri dev` 로 앱을 띄우고 확인:
- 워크스페이스가 홈이면 상태 ②("작업할 폴더 선택") 표시
- 폴더 선택 → 이미지/PDF가 있는 폴더면 상태 ①(요약 줄 + 맞춤 제안) 표시
- 빈 폴더면 상태 ①'("이 폴더는 비어 있어요") 표시
- 제안 칩 클릭 → 입력창에 문장이 채워지고 자동 실행되지 않음(Enter로 전송)

- [ ] **Step 7: 커밋**

```bash
git add src/App.tsx
git commit -m "feat: 빈 화면 상태머신 — 워크스페이스 요약 + 맞춤 제안 + 폴더 선택 유도"
```

---

## Task 7: system_prompt 범위 밖 거절 규칙 + bench 회귀 게이트

**Files:**
- Modify: `src-tauri/src/agent.rs:50-52` (규칙 추가), 프롬프트 테스트(파일 하단 `#[cfg(test)]`)

- [ ] **Step 1: 프롬프트 테스트(실패) 추가**

`src-tauri/src/agent.rs` 하단 테스트 모듈에, 프롬프트에 범위 밖 규칙이 있는지 검증하는 테스트를 추가 (기존 프롬프트 테스트들 옆, 예: 1800행대 `완료 주장 근거 규칙` 테스트 근처):

```rust
    #[test]
    fn prompt_has_out_of_scope_honesty_rule() {
        let p = system_prompt(&AppConfig::default());
        assert!(p.contains("할 수 없는 작업"), "범위 밖 거절+대안 규칙 누락");
    }
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cd src-tauri && cargo test --lib prompt_has_out_of_scope 2>&1 | tail -10`
Expected: FAIL (assert 실패 — 아직 문구 없음).

- [ ] **Step 3: 규칙 16 추가**

`src-tauri/src/agent.rs`의 규칙 15 끝(52행 `...해당 도구를 호출한다.\n\n\`)을 아래로 교체 — 규칙 16을 끼우고 `{persona}` 앞 줄바꿈은 유지:

```rust
            배경제거·변환·저장 요청은 그 경로를 인자로 해당 도구를 호출한다.\n\
         16. 도구로 할 수 없는 작업(웹 검색·다운로드, 이메일 발송, 스캔본 OCR, 이미지 생성,\n\
            다른 앱 제어)은 솔직하게 못 한다고 말하고, 가능한 가장 가까운 대안을 한 가지 제안한다.\n\n\
         {persona}",
```

- [ ] **Step 4: 단위 테스트 통과 확인**

Run: `cd src-tauri && cargo test --lib 2>&1 | tail -15`
Expected: PASS (프롬프트 테스트 포함 전체 통과).

- [ ] **Step 5: bench 회귀 검증 (게이트)**

기본 모델로 툴콜 정확도 회귀를 확인한다(규칙 추가가 2B 라우팅을 망가뜨리지 않는지):

Run: `node bench/toolcall_test.mjs ~/.lmstudio/models/lmstudio-community/Qwen3.5-2B-GGUF/Qwen3.5-2B-Q4_K_M.gguf "rule16-regression"`
Expected: 툴콜 정확도가 규칙 추가 전과 동등 수준(눈에 띄는 하락 없음).

> ⚠️ **게이트:** 정확도가 유의하게 떨어지면 규칙 16 문구를 더 짧게 줄이거나(예: "도구로 못 하는 일은 솔직히 말하고 대안을 제시한다." 한 줄), 그래도 회귀가 남으면 이 Task를 되돌리고(규칙 제거) 한계 처리를 도구 결과/UI 단으로 미룬다(선행 검토 §5.5). bench가 통과할 때만 커밋한다.

- [ ] **Step 6: 커밋 (bench 통과 시에만)**

```bash
git add src-tauri/src/agent.rs
git commit -m "feat: system_prompt 규칙 16 — 범위 밖 요청 솔직 거절+대안 (bench 회귀 통과)"
```

---

## Self-Review 결과 (작성자 점검)

- **스펙 커버리지:** 상태머신 ①/①'/②(Task 6), `workspace_summary`(Task 1·2), 제안=입력창 채우기(Task 5·6), 배경제거 `.ort` 게이트(Task 1), 반응형 한계+bench 게이트(Task 7), 타입 동기화(Task 3), config-changed 갱신(Task 4) — 스펙 §2~§6 모두 태스크로 매핑됨.
- **스펙과의 의도적 차이:** 스펙 §3.2의 `buildSuggestions`(프론트)를 **백엔드 `build_suggestions`로 이동**. 이유: 프론트 테스트 러너 부재 — 결정적 로직을 Rust 단위테스트로 검증하려는 스펙 의도를 더 잘 만족.
- **플레이스홀더:** 없음(모든 코드 스텝에 실제 코드/명령/기대출력 명시).
- **타입 일관성:** `WorkspaceSummary` 필드명이 Rust(snake_case)·types.ts·App.tsx 사용처에서 일치. `summary`/`draft`/`prefill`/`onPrefillConsumed` 명칭이 Task 4·5·6에서 일치.
