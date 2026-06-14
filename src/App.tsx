import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Composer } from "./components/Composer";
import { MessageView } from "./components/MessageView";
import { SessionsSidebar } from "./components/SessionsSidebar";
import { SettingsPanel } from "./components/SettingsPanel";
import { useAgent } from "./hooks/useAgent";

/** 경로 마지막 폴더명 (헤더 칩 표시용) */
function lastSegment(p: string) {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

/** 마크다운 특수문자를 이스케이프 — 폴더명을 볼드 안에 안전하게 넣기 위해. */
function escapeMd(s: string): string {
  return s.replace(/[\\`*_{}\[\]()#+\-.!|]/g, "\\$&");
}

/** 빈 화면에 띄울 합성 어시스턴트 말풍선 텍스트(마크다운). 결정적 — 모델 호출 없음. */
function introBubble(summary: import("./types").WorkspaceSummary | null): string {
  // 상태 ② — 홈/첫 실행/요약 로딩 전(null)
  if (!summary || summary.is_default_home) {
    return "안녕하세요! 사진 배경 제거·정리, 이미지를 PDF로 묶기, 화면 캡처 같은 일을 이 PC 안에서만 도와드려요.\n\n먼저 작업할 폴더를 골라주세요.";
  }
  // 상태 ①' — 폴더 지정 + 다룰 파일 없음
  if (summary.is_empty) {
    return `📁 **${escapeMd(summary.folder_name)}** 폴더에는 아직 다룰 수 있는 파일(이미지·PDF·zip)이 없어요.\n\n다른 폴더를 고르거나, "화면 캡처해줘"라고 말해보세요.`;
  }
  // 상태 ① — 폴더 + 파일 있음: 요약 + 예시 발화
  const counts = [
    summary.images && `이미지 ${summary.images}장`,
    summary.pdfs && `PDF ${summary.pdfs}개`,
    summary.zips && `zip ${summary.zips}개`,
  ]
    .filter(Boolean)
    .join(", ");
  const examples = summary.suggestions.map((s) => `- "${s}"`).join("\n");
  return `📁 **${escapeMd(summary.folder_name)}** 폴더에 ${counts}가 있어요.\n\n예를 들어 이렇게 말해보세요:\n\n${examples}`;
}

function App() {
  const { messages, busy, server, config, summary, send, cancel, newChat, loadSession, sessionId } =
    useAgent();
  const [showSettings, setShowSettings] = useState(false);
  const [showSessions, setShowSessions] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);

  // 새 콘텐츠가 생기면 맨 아래로
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  const pickWorkspace = async () => {
    if (!config) return;
    const dir = await invoke<string | null>("pick_folder", {
      initialDir: config.workspace_dir || null,
    });
    if (dir) await invoke("set_config", { newConfig: { ...config, workspace_dir: dir } });
  };

  const statusLabel =
    server.status === "ready"
      ? server.detail || "ready"
      : server.status === "loading"
        ? `로딩 중 ${server.detail}`
        : `중단됨 ${server.detail}`;

  return (
    <div className="app">
      <header className="header">
        <span className="wordmark">
          {config?.agent_name ? (
            config.agent_name
          ) : (
            <>
              LOCAL<em>·</em>AGENT
            </>
          )}
        </span>
        <span className="led-status" title={server.detail}>
          <span className={`led ${server.status}`} />
          {statusLabel}
        </span>
        {config && (
          <button
            className="ws-chip"
            title={`워크스페이스: ${config.workspace_dir}\n(클릭해서 변경)`}
            onClick={() => setShowSettings(true)}
          >
            📁 {lastSegment(config.workspace_dir)}
          </button>
        )}
        <div className="header-actions">
          <button className="icon-btn" onClick={newChat}>
            + 새 대화
          </button>
          <button
            className={`icon-btn ${showSessions ? "on" : ""}`}
            title="대화 목록 열기/닫기"
            onClick={() => setShowSessions((v) => !v)}
          >
            ☰ 목록
          </button>
          <button className="icon-btn" onClick={() => setShowSettings(true)}>
            설정
          </button>
        </div>
      </header>

      <div className="body">
        {showSessions && (
          <SessionsSidebar activeId={sessionId} busy={busy} onLoad={loadSession} />
        )}
        <div className="main">
          <div className="chat-scroll" ref={scrollRef}>
            <div className="chat-inner">
          {messages.length === 0 ? (
            <div className="msg-assistant intro-bubble">
              <div className="prose">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{introBubble(summary)}</ReactMarkdown>
              </div>
              {(!summary || summary.is_default_home || summary.is_empty) && (
                <button className="suggestion" onClick={pickWorkspace}>
                  📁 작업할 폴더 선택
                </button>
              )}
            </div>
          ) : (
            messages.map((m, i) => <MessageView key={i} msg={m} />)
          )}
            </div>
          </div>

          <Composer
            busy={busy}
            disabled={server.status !== "ready"}
            onSend={send}
            onCancel={cancel}
          />
        </div>
      </div>

      {showSettings && <SettingsPanel onClose={() => setShowSettings(false)} />}
    </div>
  );
}

export default App;
