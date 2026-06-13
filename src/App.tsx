import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
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

function App() {
  const { messages, busy, server, config, summary, send, cancel, newChat, loadSession, sessionId } =
    useAgent();
  const [showSettings, setShowSettings] = useState(false);
  const [showSessions, setShowSessions] = useState(true);
  const [draft, setDraft] = useState<string | undefined>(undefined);
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
            <div className="empty-state">
              <div className="empty-mark">
                LOCAL
                <br />
                <em>AGENT</em>
              </div>

              {summary && !summary.is_default_home && !summary.is_empty ? (
                // 상태 ① — 폴더 + 다룰 파일 있음: 요약 + 맞춤 제안
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
                // 상태 ② / ①' — 홈/첫 실행 또는 다룰 파일 없는 폴더: 폴더 선택 유도
                <>
                  <p className="empty-sub">
                    {summary && !summary.is_default_home && summary.is_empty
                      ? "이 폴더에는 처리할 수 있는 파일이 없어요. 다른 폴더를 고르거나 화면을 캡처해 보세요."
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
            prefill={draft}
            onPrefillConsumed={() => setDraft(undefined)}
          />
        </div>
      </div>

      {showSettings && <SettingsPanel onClose={() => setShowSettings(false)} />}
    </div>
  );
}

export default App;
