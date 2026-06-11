import { FormEvent, KeyboardEvent, useRef, useState } from "react";

interface Props {
  busy: boolean;
  disabled: boolean;
  onSend: (text: string) => void;
  onCancel: () => void;
}

export function Composer({ busy, disabled, onSend, onCancel }: Props) {
  const [text, setText] = useState("");
  const taRef = useRef<HTMLTextAreaElement>(null);

  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    const trimmed = text.trim();
    if (!trimmed || busy || disabled) return;
    onSend(trimmed);
    setText("");
    if (taRef.current) taRef.current.style.height = "auto";
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      submit();
    }
  };

  return (
    <div className="composer-wrap">
      <form className="composer" onSubmit={submit}>
        <textarea
          ref={taRef}
          rows={1}
          value={text}
          placeholder={disabled ? "모델 로딩 중…" : "무엇을 도와드릴까요? (예: 다운로드 폴더에서 PDF 찾아줘)"}
          onChange={(e) => {
            setText(e.target.value);
            e.target.style.height = "auto";
            e.target.style.height = `${Math.min(e.target.scrollHeight, 180)}px`;
          }}
          onKeyDown={onKeyDown}
        />
        {busy ? (
          <button type="button" className="send-btn stop" title="중단" onClick={onCancel}>
            ■
          </button>
        ) : (
          <button type="submit" className="send-btn" title="보내기" disabled={disabled || !text.trim()}>
            ↑
          </button>
        )}
      </form>
      <div className="composer-hint">Enter 전송 · Shift+Enter 줄바꿈 · 로컬에서만 동작 — 데이터가 PC를 떠나지 않습니다</div>
    </div>
  );
}
