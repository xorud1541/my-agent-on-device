#!/usr/bin/env node
// WebView2 CDP로 실제 UI를 구동: 서버 ready 대기 → 제안 버튼 클릭 → 턴 완료 관찰
const CDP = "http://127.0.0.1:9222";

const targets = await (await fetch(`${CDP}/json`)).json();
const page = targets.find((t) => t.type === "page" && t.url.includes("localhost:1420"));
if (!page) {
  console.error("Local Agent 페이지를 못 찾음:", targets.map((t) => t.url));
  process.exit(1);
}
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const j = JSON.parse(m.data);
  if (pending.has(j.id)) {
    pending.get(j.id)(j);
    pending.delete(j.id);
  }
};
await new Promise((r) => (ws.onopen = r));

function evaluate(expr) {
  return new Promise((resolve) => {
    const reqId = ++id;
    pending.set(reqId, (j) => resolve(j.result?.result?.value));
    ws.send(JSON.stringify({ id: reqId, method: "Runtime.evaluate", params: { expression: expr, returnByValue: true } }));
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// 1. 서버 ready LED 대기 (최대 120초 — 모델 로드 포함)
let ready = false;
for (let i = 0; i < 120; i++) {
  if (await evaluate(`!!document.querySelector('.led.ready')`)) { ready = true; break; }
  await sleep(1000);
}
console.log("server-ready:", ready, "| status text:", await evaluate(`document.querySelector('.led-status')?.innerText`));
if (!ready) process.exit(1);

// 2. 제안 칩 클릭 ("지금 화면 캡처해줘" — 부작용이 Pictures/LocalAgent 폴더로 한정됨)
const chips = await evaluate(`[...document.querySelectorAll('.suggestion')].map(b => b.innerText)`);
console.log("suggestions:", chips);
await evaluate(`[...document.querySelectorAll('.suggestion')].find(b => b.innerText.includes('화면 캡처'))?.click()`);

// 3. 턴 완료(.turn-meta 등장) 대기 — 60초 예산
const t0 = Date.now();
let done = false;
for (let i = 0; i < 90; i++) {
  if (await evaluate(`!!document.querySelector('.turn-meta')`)) { done = true; break; }
  await sleep(1000);
}
const elapsed = ((Date.now() - t0) / 1000).toFixed(1);

console.log("turn-done:", done, `(${elapsed}s)`);
console.log("user-msg:", await evaluate(`document.querySelector('.msg-user')?.innerText`));
console.log("tools-used:", await evaluate(`[...document.querySelectorAll('.tool-name')].map(e => e.innerText)`));
console.log("tool-states:", await evaluate(`[...document.querySelectorAll('.tool-dot')].map(e => e.className)`));
console.log("thinking-blocks:", await evaluate(`document.querySelectorAll('.think').length`));
console.log("answer:", (await evaluate(`[...document.querySelectorAll('.prose')].map(e => e.innerText).join(' | ')`))?.slice(0, 300));
console.log("turn-meta:", await evaluate(`document.querySelector('.turn-meta')?.innerText`));
ws.close();
process.exit(done ? 0 : 1);
