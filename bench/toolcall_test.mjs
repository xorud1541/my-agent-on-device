#!/usr/bin/env node
// 후보 모델별 한국어 툴콜 정확도 + 실측 레이턴시 테스트.
// 사용법: node toolcall_test.mjs <model.gguf> <label>
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const LLAMA = "C:/Users/EST/Downloads/llama-b9334-bin-win-vulkan-x64/llama-server.exe";
const PORT = 8737;
const [model, label] = process.argv.slice(2);
const tools = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "tools.json"), "utf8"));

const SYS =
  "너는 사용자의 PC에서 동작하는 로컬 에이전트다. 사용자의 요청을 수행하기 위해 필요하면 도구를 호출한다. " +
  "도구가 필요한 작업이면 반드시 도구를 호출하고, 잡담에는 도구 없이 한국어로 답한다.";

const CASES = [
  ["파일검색", "내 다운로드 폴더(C:\\Users\\EST\\Downloads)에서 png 이미지들 찾아줘", "search_files"],
  ["PDF", "C:\\docs\\계약서.pdf 내용 요약해줘", "pdf_extract_text"],
  ["캡처", "지금 화면 캡처 좀 해줘", "screen_capture"],
  ["리사이즈", "C:\\img\\photo.jpg 를 가로 800픽셀로 줄여줘", "image_transform"],
  ["잡담", "오늘 기분 어때?", "NO_TOOL"],
];

const srv = spawn(LLAMA, ["-m", model, "--port", String(PORT), "-ngl", "99", "--device", "Vulkan0", "-c", "8192", "--jinja", "--no-webui"], { stdio: "ignore" });
process.on("exit", () => srv.kill());

const t0 = Date.now();
let ready = false;
for (let i = 0; i < 120; i++) {
  try {
    const r = await fetch(`http://127.0.0.1:${PORT}/health`);
    if (r.ok) { ready = true; break; }
  } catch {}
  await new Promise((r) => setTimeout(r, 1000));
}
if (!ready) { console.log(`### ${label} SERVER FAILED TO START`); process.exit(1); }
console.log(`### ${label} ready, model load ${(Date.now() - t0) / 1000}s`);

let pass = 0;
for (const [name, user, expect] of CASES) {
  const body = {
    model: "default",
    messages: [{ role: "system", content: SYS }, { role: "user", content: user }],
    tools, tool_choice: "auto", temperature: 0.2, max_tokens: 2048,
  };
  const t1 = Date.now();
  let got;
  try {
    const resp = await fetch(`http://127.0.0.1:${PORT}/v1/chat/completions`, {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
    });
    const j = await resp.json();
    const m = j.choices?.[0]?.message ?? {};
    const tcs = m.tool_calls ?? [];
    got = tcs.length
      ? tcs.map((t) => `${t.function.name} ${t.function.arguments.replace(/\n/g, " ")}`).join(";")
      : `NO_TOOL: ${(m.content ?? JSON.stringify(j)).slice(0, 120).replace(/\n/g, " ")}`;
  } catch (e) {
    got = `ERR ${e}`;
  }
  const sec = ((Date.now() - t1) / 1000).toFixed(1);
  const ok = expect === "NO_TOOL" ? got.startsWith("NO_TOOL") : got.startsWith(expect);
  if (ok) pass++;
  console.log(`[${ok ? "PASS" : "FAIL"}] ${sec}s | ${name} | expect=${expect} | got=${got.slice(0, 160)}`);
}
console.log(`### ${label} score ${pass}/${CASES.length}`);
srv.kill();
