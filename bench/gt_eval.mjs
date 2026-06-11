#!/usr/bin/env node
// alian GT(Ground-Truth) single-step 데이터셋으로 local-agent의 도구 선택·인자 추출을 평가한다.
// 떠 있는 앱 사이드카(127.0.0.1:8736)와 실제 앱의 시스템 프롬프트/스키마(dump.json)를 그대로 사용.
// 사용법: node gt_eval.mjs [perTool=5]
//   - GT 도구 중 local-agent에 대응 도구가 없는 것(open_file, create_dir, blur/remove_objects, pdf_merge/split)은 스킵
//   - 채점: tool_ok = 매핑된 도구 선택 / args_ok = explicit_args 값들이 인자 JSON에 포함(공백제거·소문자)
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// GT_BASE 로 다른 서버(예: 후보 모델을 띄운 8737)를 겨냥할 수 있다 — 모델 A/B 평가용
const BASE = process.env.GT_BASE ?? "http://127.0.0.1:8736";
const GT_PATH = "C:/repo/alian/docs/Ground-Truth/gt_single.json";
const PER_TOOL = Number(process.argv[2] ?? 5);

const here = dirname(fileURLToPath(import.meta.url));
const dump = JSON.parse(readFileSync(join(here, "dump.json"), "utf8"));
const gt = JSON.parse(readFileSync(GT_PATH, "utf8"));

// GT(alian) 도구 → local-agent 대응 도구 (없으면 스킵)
const MAP = {
  list_dir: ["list_dir"],
  read_file: ["read_file"],
  write_file: ["write_file"],
  rename_file: ["move_path"],
  copy_file: ["copy_path"],
  move_file: ["move_path"],
  image_rotate: ["image_transform"],
  image_convert_format: ["image_transform"],
  image_remove_background: ["remove_background"],
  image_to_pdf: ["images_to_pdf"],
  archive_zip: ["zip_create"],
  archive_unzip: ["zip_extract"],
};

// agent::tools_to_exclude 와 동일한 턴 단위 라우팅 재현
const BG_KEYWORDS = ["배경제거", "배경 제거", "배경을 제거", "누끼",
  "배경 빼", "배경을 빼", "배경 없애", "배경을 없애", "배경 지워", "배경을 지워"];
const WRITE_VERBS = ["적어", "저장", "기록", "작성", "메모", "써"];
const READ_HINTS = ["읽", "요약", "번역", "분석", "정리", "내용", "보여", "뭐", "무엇", "어떤"];
const stripQuoted = (s) => s.replace(/'[^']*'|"[^"]*"|“[^”]*”|‘[^’]*’/g, "");
const isDictationWrite = (q) => {
  const t = stripQuoted(q);
  return t.includes("라고") && WRITE_VERBS.some((v) => t.includes(v))
    && !READ_HINTS.some((h) => t.includes(h));
};
const toolsFor = (q) => {
  if (BG_KEYWORDS.some((k) => q.includes(k)))
    return dump.tools.filter((t) => t.function.name !== "image_transform");
  if (isDictationWrite(q))
    return dump.tools.filter((t) => !["read_file", "list_dir", "search_files", "pdf_extract_text", "move_path", "copy_path", "delete_path"].includes(t.function.name));
  return dump.tools;
};

const norm = (v) => String(v).toLowerCase().replace(/\s+/g, "");

const ONLY = process.argv[3]; // 특정 GT 도구만 재평가 (예: image_remove_background)
const skipped = [...new Set(gt.map((c) => c.expected_tool))].filter((t) => !MAP[t]);
const cases = Object.keys(MAP)
  .filter((t) => !ONLY || t === ONLY)
  .flatMap((t) => gt.filter((c) => c.expected_tool === t).slice(0, PER_TOOL));
console.log(`평가 ${cases.length}건 (도구당 ${PER_TOOL}) | 스킵: ${skipped.join(", ")}`);

const stats = {}; // gtTool -> {n, tool, args, ms[]}
for (const [i, c] of cases.entries()) {
  const body = {
    model: "default",
    messages: [
      { role: "system", content: dump.system },
      { role: "user", content: c.query },
    ],
    tools: toolsFor(c.query), tool_choice: "auto",
    temperature: 0.4, repeat_penalty: 1.1, max_tokens: 1024, stream: false,
  };
  const t0 = Date.now();
  let name = "(없음)", args = "";
  try {
    const r = await fetch(`${BASE}/v1/chat/completions`, {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const j = await r.json();
    const tc = j.choices?.[0]?.message?.tool_calls?.[0];
    if (tc) ({ name, arguments: args } = tc.function);
    else name = `(툴콜없음:${(j.choices?.[0]?.message?.content ?? JSON.stringify(j.error ?? j)).slice(0, 60)})`;
  } catch (e) { name = `(ERR:${e})`; }
  const ms = Date.now() - t0;

  const toolOk = (MAP[c.expected_tool] ?? []).includes(name);
  const nargs = norm(args);
  const argsOk = toolOk && Object.values(c.explicit_args ?? {}).every((v) => nargs.includes(norm(v)));
  const s = (stats[c.expected_tool] ??= { n: 0, tool: 0, args: 0, ms: [] });
  s.n++; s.tool += toolOk; s.args += argsOk; s.ms.push(ms);
  console.log(`[${String(i + 1).padStart(3)}/${cases.length}] ${toolOk ? (argsOk ? "PASS" : "tool✓args✗") : "FAIL"} ${(ms / 1000).toFixed(1)}s | ${c.expected_tool} → ${name} | ${c.query.slice(0, 40)} | ${args.slice(0, 100)}`);
}

console.log("\n도구별 결과 (GT도구: 도구선택 / 인자값추출):");
let T = 0, A = 0, N = 0, MS = 0;
for (const [t, s] of Object.entries(stats)) {
  const avg = s.ms.reduce((a, b) => a + b, 0) / s.ms.length / 1000;
  console.log(`  ${t.padEnd(24)} ${s.tool}/${s.n}  ${s.args}/${s.n}  평균 ${avg.toFixed(1)}s`);
  T += s.tool; A += s.args; N += s.n; MS += s.ms.reduce((a, b) => a + b, 0);
}
console.log(`\n총계: Tool Accuracy ${T}/${N} (${(100 * T / N).toFixed(0)}%) | Args ${A}/${N} (${(100 * A / N).toFixed(0)}%) | 평균 ${(MS / N / 1000).toFixed(1)}s/건`);
