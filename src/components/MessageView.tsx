import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { AssistantMessage, UiMessage } from "../types";
import { ThinkingBlock } from "./ThinkingBlock";
import { ToolStep } from "./ToolStep";

function AssistantView({ msg }: { msg: AssistantMessage }) {
  const inProgress = msg.elapsedMs === undefined;
  return (
    <div className="msg-assistant">
      {msg.segments.map((seg, i) => {
        switch (seg.kind) {
          case "thinking":
            return <ThinkingBlock key={i} text={seg.text} done={seg.done} />;
          case "tool":
            return <ToolStep key={seg.callId + i} seg={seg} />;
          case "text":
            return (
              <div key={i} className="prose">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{seg.text}</ReactMarkdown>
              </div>
            );
          case "error":
            return (
              <div key={i} className="error-chip">
                {seg.message}
              </div>
            );
        }
      })}
      {inProgress && msg.segments.length === 0 && (
        <span className="waiting">
          <i />
          <i />
          <i />
        </span>
      )}
      {/* elapsedMs 0 은 복원된 턴(측정값 없음) — 배지를 숨긴다 */}
      {!inProgress && msg.elapsedMs! > 0 && (
        <div className="turn-meta">{(msg.elapsedMs! / 1000).toFixed(1)}s</div>
      )}
    </div>
  );
}

export function MessageView({ msg }: { msg: UiMessage }) {
  if (msg.role === "user") {
    return (
      <div className="msg-user">
        {msg.images && msg.images.length > 0 && (
          <div className="msg-user-images">
            {msg.images.map((im, i) =>
              im.thumb ? (
                <img key={i} className="msg-thumb" src={im.thumb} alt="첨부 이미지" />
              ) : (
                <span key={i} className="msg-thumb-ph">
                  📷 캡처
                </span>
              ),
            )}
          </div>
        )}
        {msg.text && <div className="msg-user-text">{msg.text}</div>}
      </div>
    );
  }
  return <AssistantView msg={msg} />;
}
