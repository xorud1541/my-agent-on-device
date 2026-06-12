import { invoke } from "@tauri-apps/api/core";
import { FormEvent, KeyboardEvent, useRef, useState } from "react";
import { RegionOverlay } from "./RegionOverlay";

interface Attachment {
  path: string;
  thumb: string;
}

interface FullCapture {
  path: string;
  data_url: string;
  width: number;
  height: number;
}

interface Props {
  busy: boolean;
  disabled: boolean;
  onSend: (text: string, attachments: Attachment[]) => void;
  onCancel: () => void;
}

export function Composer({ busy, disabled, onSend, onCancel }: Props) {
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [pending, setPending] = useState<FullCapture | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  const canSend = (text.trim().length > 0 || attachments.length > 0) && !busy && !disabled;

  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    if (!canSend) return;
    onSend(text.trim(), attachments);
    setText("");
    setAttachments([]);
    if (taRef.current) taRef.current.style.height = "auto";
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      submit();
    }
  };

  const capture = async () => {
    if (capturing || busy) return;
    setCaptureError(null);
    setCapturing(true);
    try {
      // 앱 숨김 → 전체 캡처 → 복귀. 영역 선택은 앱 내 모달에서 이어진다.
      const full = await invoke<FullCapture>("capture_screenshot");
      setPending(full);
    } catch (err) {
      setCaptureError(String(err));
    } finally {
      setCapturing(false);
    }
  };

  const removeAt = (i: number) => setAttachments((a) => a.filter((_, idx) => idx !== i));

  return (
    <div className="composer-wrap">
      {pending && (
        <RegionOverlay
          src={pending.data_url}
          fullPath={pending.path}
          onDone={(att) => {
            setAttachments((a) => [...a, att]);
            setPending(null);
          }}
          onCancel={() => setPending(null)}
        />
      )}
      {captureError && <div className="capture-error">캡처 실패: {captureError}</div>}
      {attachments.length > 0 && (
        <div className="composer-attachments">
          {attachments.map((a, i) => (
            <div key={a.path} className="attach-chip">
              <img src={a.thumb} alt="첨부 이미지" />
              <button type="button" className="attach-remove" title="제거" onClick={() => removeAt(i)}>
                ✕
              </button>
            </div>
          ))}
        </div>
      )}
      <form className="composer" onSubmit={submit}>
        <button
          type="button"
          className="capture-btn"
          title="스크린샷 첨부"
          onClick={capture}
          disabled={disabled || busy || capturing}
        >
          {capturing ? "…" : "📷"}
        </button>
        <textarea
          ref={taRef}
          rows={1}
          value={text}
          placeholder={disabled ? "모델 로딩 중…" : "무엇을 도와드릴까요? (📷 로 화면을 첨부할 수 있어요)"}
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
          <button type="submit" className="send-btn" title="보내기" disabled={!canSend}>
            ↑
          </button>
        )}
      </form>
      <div className="composer-hint">Enter 전송 · Shift+Enter 줄바꿈 · 로컬에서만 동작 — 데이터가 PC를 떠나지 않습니다</div>
    </div>
  );
}
