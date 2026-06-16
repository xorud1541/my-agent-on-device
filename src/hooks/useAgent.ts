import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { chatToUi } from "../lib/restore";
import type {
  AgentEvent,
  AppConfig,
  AssistantMessage,
  ChatMessage,
  ServerStatus,
  LocalsearchStatus,
  UiMessage,
  WorkspaceSummary,
} from "../types";

/**
 * 세션 + 이벤트 스트림을 UI 메시지 목록으로 환원하는 훅.
 * 이벤트는 항상 "마지막 어시스턴트 메시지"에 누적된다.
 */
export function useAgent() {
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [busy, setBusy] = useState(false);
  const [server, setServer] = useState<ServerStatus>({ status: "loading", detail: "" });
  const [localsearch, setLocalsearch] = useState<LocalsearchStatus>({ status: "disabled", detail: "" });
  // 살아있는 설정(워크스페이스/페르소나) — 설정 패널이든 에이전트 도구든
  // 어디서 바뀌어도 config-changed 이벤트로 즉시 갱신된다
  const [config, setConfig] = useState<AppConfig | null>(null);
  // 빈 화면 디스커버빌리티 — 현재 워크스페이스 요약(타입별 개수 + 결정적 제안)
  const [summary, setSummary] = useState<WorkspaceSummary | null>(null);
  const refreshSummary = useCallback(() => {
    invoke<WorkspaceSummary>("workspace_summary").then(setSummary).catch(() => {});
  }, []);
  // ref: 이벤트 필터링용(리스너 클로저에서 최신값 필요) / state: UI 표시용(대화 목록 강조)
  const sessionRef = useRef<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);

  // 어시스턴트 마지막 메시지를 불변 갱신
  const patchAssistant = useCallback((fn: (m: AssistantMessage) => AssistantMessage) => {
    setMessages((prev) => {
      const last = prev[prev.length - 1];
      if (!last || last.role !== "assistant") return prev;
      return [...prev.slice(0, -1), fn(last)];
    });
  }, []);

  useEffect(() => {
    const unlisten = listen<AgentEvent>("agent-event", ({ payload: ev }) => {
      if (ev.type === "server-status") {
        setServer({ status: ev.status, detail: ev.detail });
        return;
      }
      if (ev.type === "localsearch-status") {
        setLocalsearch({ status: ev.status, detail: ev.detail });
        return;
      }
      if (ev.type === "config-changed") {
        setConfig(ev.config);
        invoke<WorkspaceSummary>("workspace_summary").then(setSummary).catch(() => {});
        return;
      }
      if (sessionRef.current && "session_id" in ev && ev.session_id !== sessionRef.current) return;

      switch (ev.type) {
        case "thinking-delta":
          patchAssistant((m) => {
            const segs = [...m.segments];
            const last = segs[segs.length - 1];
            if (last?.kind === "thinking" && !last.done) {
              segs[segs.length - 1] = { ...last, text: last.text + ev.delta };
            } else {
              segs.push({ kind: "thinking", text: ev.delta, done: false });
            }
            return { ...m, segments: segs };
          });
          break;
        case "text-delta":
          patchAssistant((m) => {
            const segs = m.segments.map((s) =>
              s.kind === "thinking" && !s.done ? { ...s, done: true } : s,
            );
            const last = segs[segs.length - 1];
            if (last?.kind === "text") {
              segs[segs.length - 1] = { ...last, text: last.text + ev.delta };
            } else {
              segs.push({ kind: "text", text: ev.delta });
            }
            return { ...m, segments: segs };
          });
          break;
        case "tool-call-start":
          patchAssistant((m) => ({
            ...m,
            segments: [
              ...m.segments.map((s) => (s.kind === "thinking" && !s.done ? { ...s, done: true } : s)),
              { kind: "tool", callId: ev.call_id, name: ev.name, arguments: ev.arguments, status: "running" },
            ],
          }));
          break;
        case "tool-call-end":
          patchAssistant((m) => ({
            ...m,
            segments: m.segments.map((s) =>
              s.kind === "tool" && s.callId === ev.call_id && s.status === "running"
                ? { ...s, status: ev.ok ? "ok" : "error", result: ev.result }
                : s,
            ),
          }));
          break;
        case "error":
          patchAssistant((m) => ({
            ...m,
            segments: [...m.segments, { kind: "error", message: ev.message }],
          }));
          break;
        case "sources":
          patchAssistant((m) => ({ ...m, sources: ev.sources }));
          break;
        case "turn-end":
          patchAssistant((m) => ({
            ...m,
            elapsedMs: ev.elapsed_ms,
            segments: m.segments.map((s) => (s.kind === "thinking" && !s.done ? { ...s, done: true } : s)),
          }));
          setBusy(false);
          break;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [patchAssistant]);

  // 초기 설정 + 워크스페이스 요약 로드 (이후 변경은 config-changed 가 갱신)
  useEffect(() => {
    invoke<AppConfig>("get_config").then(setConfig).catch(() => {});
    refreshSummary();
    // 인덱싱 배너 레이스 보정: 마운트 시 현재 로컬 검색 상태를 조회 (이후는 이벤트가 갱신)
    invoke<[string, string]>("get_localsearch_status")
      .then(([status, detail]) =>
        setLocalsearch({ status: status as LocalsearchStatus["status"], detail }),
      )
      .catch(() => {});
  }, [refreshSummary]);

  // ready 이벤트가 리스너 등록 전에 발행되는 레이스 보정: ready 가 아닐 동안 상태 폴링
  useEffect(() => {
    if (server.status === "ready") return;
    const timer = setInterval(async () => {
      try {
        const status = await invoke<string>("server_status");
        if (status === "ready") {
          const cfg = await invoke<{ model_path: string }>("get_config");
          const model = cfg.model_path.split(/[\\/]/).pop() ?? "";
          setServer({ status: "ready", detail: model });
        }
      } catch {
        // 백엔드 미준비 — 다음 틱에 재시도
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [server.status]);

  const ensureSession = useCallback(async () => {
    if (!sessionRef.current) {
      sessionRef.current = await invoke<string>("new_session");
      setSessionId(sessionRef.current);
    }
    return sessionRef.current;
  }, []);

  const send = useCallback(
    async (text: string, attachments: { path: string; thumb: string }[] = []) => {
      const sessionId = await ensureSession();
      setMessages((prev) => [
        ...prev,
        { role: "user", text, images: attachments.length ? attachments : undefined },
        { role: "assistant", segments: [] },
      ]);
      setBusy(true);
      try {
        await invoke("send_message", {
          sessionId,
          text,
          attachments: attachments.map((a) => a.path),
        });
      } catch (e) {
        patchAssistant((m) => ({
          ...m,
          elapsedMs: 0,
          segments: [...m.segments, { kind: "error", message: String(e) }],
        }));
        setBusy(false);
      }
    },
    [ensureSession, patchAssistant],
  );

  const cancel = useCallback(async () => {
    if (sessionRef.current) {
      await invoke("cancel_turn", { sessionId: sessionRef.current });
    }
  }, []);

  const newChat = useCallback(() => {
    // 진행된 대화는 턴마다 백엔드가 디스크에 저장하므로 여기선 화면만 비운다
    sessionRef.current = null;
    setSessionId(null);
    setMessages([]);
    setBusy(false);
    refreshSummary();
  }, [refreshSummary]);

  /** 저장된 세션을 불러와 화면을 복원하고, 이어서 대화할 수 있게 한다 */
  const loadSession = useCallback(async (id: string) => {
    const history = await invoke<ChatMessage[]>("load_session", { sessionId: id });
    sessionRef.current = id;
    setSessionId(id);
    setMessages(chatToUi(history));
    setBusy(false);
  }, []);

  return { messages, busy, server, localsearch, config, summary, send, cancel, newChat, loadSession, sessionId };
}
