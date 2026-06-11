// 백엔드 AgentEvent(serde kebab-case tag)와 1:1 대응
export type AgentEvent =
  | { type: "thinking-delta"; session_id: string; delta: string }
  | { type: "text-delta"; session_id: string; delta: string }
  | { type: "tool-call-start"; session_id: string; call_id: string; name: string; arguments: string }
  | { type: "tool-call-end"; session_id: string; call_id: string; name: string; ok: boolean; result: string }
  | { type: "turn-end"; session_id: string; elapsed_ms: number }
  | { type: "error"; session_id: string; message: string }
  | { type: "server-status"; status: "loading" | "ready" | "down"; detail: string };

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
}

export type UiMessage = UserMessage | AssistantMessage;

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
}

export type ServerStatus = { status: "loading" | "ready" | "down"; detail: string };
