import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { SessionMeta } from "../types";

interface Props {
  /** 현재 보고 있는 세션 (강조 표시용) */
  activeId: string | null;
  /** 턴 종료 시 목록 갱신 트리거 */
  busy: boolean;
  onLoad: (id: string) => void;
}

function formatWhen(iso: string) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const sameDay = d.toDateString() === new Date().toDateString();
  return sameDay
    ? d.toLocaleTimeString("ko-KR", { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleDateString("ko-KR", { month: "long", day: "numeric" });
}

/** 저장된 대화 목록 — 왼쪽 사이드 패널. 클릭해서 불러오고, 휴지통으로 삭제 */
export function SessionsSidebar({ activeId, busy, onLoad }: Props) {
  const [sessions, setSessions] = useState<SessionMeta[]>([]);

  // 열 때 + 턴이 끝날 때마다 갱신 (턴마다 백엔드가 디스크에 저장하므로)
  useEffect(() => {
    if (!busy) invoke<SessionMeta[]>("list_sessions").then(setSessions);
  }, [busy]);

  const remove = async (e: React.MouseEvent, id: string) => {
    e.stopPropagation();
    if (!confirm("이 대화를 삭제할까요?")) return;
    await invoke("delete_session", { sessionId: id });
    setSessions((prev) => prev.filter((s) => s.id !== id));
  };

  return (
    <aside className="sidebar">
      <div className="sidebar-title">대화 목록</div>
      {sessions.length === 0 ? (
        <p className="sidebar-empty">저장된 대화가 없습니다. 대화를 시작하면 자동으로 저장됩니다.</p>
      ) : (
        <ul className="session-list">
          {sessions.map((s) => (
            <li
              key={s.id}
              className={`session-item ${s.id === activeId ? "active" : ""}`}
              onClick={() => !busy && onLoad(s.id)}
            >
              <div className="session-title">{s.title}</div>
              <div className="session-meta">
                {formatWhen(s.updated_at)} · {s.turns}턴
                <button
                  className="session-delete"
                  title="대화 삭제"
                  onClick={(e) => remove(e, s.id)}
                >
                  🗑
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
