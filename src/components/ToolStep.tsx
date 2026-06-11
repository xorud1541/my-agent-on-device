import { useState } from "react";
import type { Segment } from "../types";

type ToolSegment = Extract<Segment, { kind: "tool" }>;

/** 도구 인자 JSON 을 한 줄 미리보기로 */
function summarizeArgs(raw: string): string {
  try {
    const obj = JSON.parse(raw);
    return Object.entries(obj)
      .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
      .join("  ");
  } catch {
    return raw;
  }
}

export function ToolStep({ seg }: { seg: ToolSegment }) {
  const [open, setOpen] = useState(false);
  const dot =
    seg.status === "running" ? (
      <span className="tool-dot running" />
    ) : seg.status === "ok" ? (
      <span className="tool-dot ok">✓</span>
    ) : (
      <span className="tool-dot error">✕</span>
    );

  return (
    <div className="tool">
      <button className="tool-head" onClick={() => setOpen((v) => !v)}>
        {dot}
        <span className="tool-name">{seg.name}</span>
        <span className="tool-args">{summarizeArgs(seg.arguments)}</span>
      </button>
      {open && seg.result !== undefined && <div className="tool-result">{seg.result}</div>}
    </div>
  );
}
