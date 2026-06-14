// 백엔드 AgentEvent(serde kebab-case tag)와 1:1 대응
export type AgentEvent =
  | { type: "thinking-delta"; session_id: string; delta: string }
  | { type: "text-delta"; session_id: string; delta: string }
  | { type: "tool-call-start"; session_id: string; call_id: string; name: string; arguments: string }
  | { type: "tool-call-end"; session_id: string; call_id: string; name: string; ok: boolean; result: string }
  | { type: "turn-end"; session_id: string; elapsed_ms: number }
  | { type: "error"; session_id: string; message: string }
  | { type: "sources"; session_id: string; sources: string[] }
  | { type: "server-status"; status: "loading" | "ready" | "down"; detail: string }
  | {
      type: "localsearch-status";
      status: "indexing" | "ready" | "disabled" | "error";
      detail: string;
    }
  | { type: "config-changed"; config: AppConfig };

// 어시스턴트 턴은 발생 순서대로 쌓이는 세그먼트의 나열이다
export type Segment =
  | { kind: "thinking"; text: string; done: boolean }
  | { kind: "text"; text: string }
  | {
      kind: "tool";
      callId: string;
      name: string;
      arguments: string;
      status: "running" | "ok" | "error";
      result?: string;
    }
  | { kind: "error"; message: string };

export interface UserMessage {
  role: "user";
  text: string;
}

export interface AssistantMessage {
  role: "assistant";
  segments: Segment[];
  /** 진행 중이면 undefined, 끝나면 소요 ms */
  elapsedMs?: number;
  /** RAG 근거로 사용된 출처 문서 파일명 (말풍선 하단 표시) */
  sources?: string[];
}

export type UiMessage = UserMessage | AssistantMessage;

// 백엔드 ChatMessage(OpenAI 호환)와 1:1 대응 — 세션 복원에 사용
export interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool";
  content?: string | null;
  tool_calls?: { id: string; type: string; function: { name: string; arguments: string } }[] | null;
  tool_call_id?: string | null;
}

// 백엔드 sessions::SessionMeta 와 1:1 대응
export interface SessionMeta {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  turns: number;
}

export interface ModelEntry {
  name: string;
  path: string;
  size_bytes: number;
}

export interface AppConfig {
  server_exe: string;
  model_path: string;
  port: number;
  device: string;
  n_gpu_layers: number;
  ctx_size: number;
  max_tool_rounds: number;
  temperature: number;
  max_output_tokens: number;
  reasoning_budget: number;
  workspace_dir: string;
  user_name: string;
  agent_name: string;
  removebg_model: string;
  localsearch_enabled: boolean;
  localsearch_bin: string;
  localsearch_models_dir: string;
  localsearch_db_dir: string;
  localsearch_port: number;
  ort_dylib: string;
}

export type ServerStatus = { status: "loading" | "ready" | "down"; detail: string };
export type LocalsearchStatus = {
  status: "indexing" | "ready" | "disabled" | "error";
  detail: string;
};
