import { useEffect, useRef, useState } from "react";

export function ThinkingBlock({ text, done }: { text: string; done: boolean }) {
  // 진행 중엔 펼침, 끝나면 자동 접힘 (사용자가 손대면 그 상태 유지)
  const [open, setOpen] = useState(true);
  const touched = useRef(false);
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (done && !touched.current) setOpen(false);
  }, [done]);

  // 스트리밍 중 자동 스크롤
  useEffect(() => {
    if (!done && bodyRef.current) {
      bodyRef.current.scrollTop = bodyRef.current.scrollHeight;
    }
  }, [text, done]);

  return (
    <div className={`think ${open ? "open" : ""}`}>
      <button
        className="think-head"
        onClick={() => {
          touched.current = true;
          setOpen((v) => !v);
        }}
      >
        <span className="chev">▶</span>
        {done ? <span>생각 완료</span> : <span className="shimmer">생각하는 중…</span>}
      </button>
      {open && (
        <div className="think-body" ref={bodyRef}>
          {text}
        </div>
      )}
    </div>
  );
}
