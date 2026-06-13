import { invoke } from "@tauri-apps/api/core";
import { FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";

interface Attachment {
  path: string;
  thumb: string;
}

interface Props {
  busy: boolean;
  disabled: boolean;
  onSend: (text: string, attachments: Attachment[]) => void;
  onCancel: () => void;
  /** 제안 칩 클릭 시 입력창에 채울 텍스트. 채운 뒤 onPrefillConsumed 로 비운다. */
  prefill?: string;
  onPrefillConsumed?: () => void;
}

/** 영역 선택 프레임 아이콘 — 모서리 브래킷 + 중앙 점 (앱 톤에 맞춘 미니멀 스트로크) */
function CaptureIcon() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M4 9V6a2 2 0 0 1 2-2h3" />
      <path d="M15 4h3a2 2 0 0 1 2 2v3" />
      <path d="M20 15v3a2 2 0 0 1-2 2h-3" />
      <path d="M9 20H6a2 2 0 0 1-2-2v-3" />
      <circle cx="12" cy="12" r="2.4" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function Composer({ busy, disabled, onSend, onCancel, prefill, onPrefillConsumed }: Props) {
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
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
      // 앱 숨김 → 자체 오버레이(별도 프로세스)에서 화면 음영+드래그 → 선택 영역만 반환
      const r = await invoke<{ path: string; thumb_data_url: string } | null>("capture_region");
      if (r) setAttachments((a) => [...a, { path: r.path, thumb: r.thumb_data_url }]);
    } catch (err) {
      setCaptureError(String(err));
    } finally {
      setCapturing(false);
    }
  };

  const removeAt = (i: number) => setAttachments((a) => a.filter((_, idx) => idx !== i));

  return (
    <div className="composer-wrap">
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
          className={capturing ? "capture-btn busy" : "capture-btn"}
          title="화면 영역 캡처"
          onClick={capture}
          disabled={disabled || busy || capturing}
        >
          <CaptureIcon />
        </button>
        <textarea
          ref={taRef}
          rows={1}
          value={text}
          placeholder={disabled ? "모델 로딩 중…" : "무엇을 도와드릴까요? (왼쪽 버튼으로 화면을 첨부할 수 있어요)"}
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
