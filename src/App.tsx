import { useEffect, useRef, useState } from "react";
import { Composer } from "./components/Composer";
import { MessageView } from "./components/MessageView";
import { SettingsPanel } from "./components/SettingsPanel";
import { useAgent } from "./hooks/useAgent";

const SUGGESTIONS = [
  "다운로드 폴더에서 PDF 파일 찾아줘",
  "지금 화면 캡처해줘",
  "바탕화면 이미지 목록 보여줘",
  "사진 폴더에서 가장 최근 이미지를 흑백으로 바꿔줘",
];

function App() {
  const { messages, busy, server, send, cancel, newChat } = useAgent();
  const [showSettings, setShowSettings] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  // 새 콘텐츠가 생기면 맨 아래로
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

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
          LOCAL<em>·</em>AGENT
        </span>
        <span className="led-status" title={server.detail}>
          <span className={`led ${server.status}`} />
          {statusLabel}
        </span>
        <div className="header-actions">
          <button className="icon-btn" onClick={newChat}>
            + 새 대화
          </button>
          <button className="icon-btn" onClick={() => setShowSettings(true)}>
            설정
          </button>
        </div>
      </header>

      <div className="chat-scroll" ref={scrollRef}>
        <div className="chat-inner">
          {messages.length === 0 ? (
            <div className="empty-state">
              <div className="empty-mark">
                LOCAL
                <br />
                <em>AGENT</em>
              </div>
              <p className="empty-sub">
                이 PC 안에서만 동작하는 에이전트입니다. 파일 검색·정리, 이미지 처리, PDF 읽기, 화면
                캡처를 말로 시키세요.
              </p>
              <div className="suggestions">
                {SUGGESTIONS.map((s) => (
                  <button
                    key={s}
                    className="suggestion"
                    onClick={() => send(s)}
                    disabled={server.status !== "ready"}
                  >
                    {s}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            messages.map((m, i) => <MessageView key={i} msg={m} />)
          )}
        </div>
      </div>

      <Composer busy={busy} disabled={server.status !== "ready"} onSend={send} onCancel={cancel} />

      {showSettings && <SettingsPanel onClose={() => setShowSettings(false)} />}
    </div>
  );
}

export default App;
