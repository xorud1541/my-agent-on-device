import type { AssistantMessage, ChatMessage, Segment, UiMessage } from "../types";

/**
 * 저장된 세션의 ChatMessage 이력을 화면용 UiMessage 로 복원한다.
 * - system(시스템 프롬프트/요약 메시지)은 화면에 보이지 않으므로 건너뜀
 * - thinking 은 이력에 저장되지 않으므로 복원 대상이 아님
 * - 도구 결과의 성공/실패 플래그는 저장되지 않아 본문 접두사("오류"/중단 안내)로 추정
 */
export function chatToUi(messages: ChatMessage[]): UiMessage[] {
  const resultById = new Map<string, string>();
  for (const m of messages) {
    if (m.role === "tool" && m.tool_call_id) resultById.set(m.tool_call_id, m.content ?? "");
  }

  const out: UiMessage[] = [];
  let current: AssistantMessage | null = null;
  for (const m of messages) {
    if (m.role === "user") {
      // 백엔드가 붙인 '[첨부 이미지: ...]' 마커는 표시용 텍스트에서 제거
      const raw = m.content ?? "";
      const text = raw.replace(/\n\n\[첨부 이미지: [^\]]*\]\s*$/, "");
      const images = (m.images ?? []).map((path) => ({ path, thumb: "" }));
      out.push({ role: "user", text, images: images.length ? images : undefined });
      current = null;
    } else if (m.role === "assistant") {
      if (!current) {
        // 복원된 턴은 완료 상태 — elapsedMs 0 은 "측정 없음"으로 취급해 배지를 숨긴다
        current = { role: "assistant", segments: [], elapsedMs: 0 };
        out.push(current);
      }
      const segs: Segment[] = [];
      for (const c of m.tool_calls ?? []) {
        const result = resultById.get(c.id);
        const failed = result === undefined || result.startsWith("오류") || result.startsWith("(턴이 중단");
        segs.push({
          kind: "tool",
          callId: c.id,
          name: c.function.name,
          arguments: c.function.arguments,
          status: failed ? "error" : "ok",
          result,
        });
      }
      if (m.content) segs.push({ kind: "text", text: m.content });
      current.segments = [...current.segments, ...segs];
    }
  }
  return out;
}
