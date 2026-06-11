#!/usr/bin/env node
// 임의 발화를 UI로 보내고 턴 결과를 출력하는 범용 드라이버.
// 사용법: node ui_send.mjs "발화 내용" [대기초=90] [keep]
//   keep: 새 대화로 초기화하지 않고 기존 세션에 이어 보냄 (멀티턴 테스트)
const [text, waitSec = "90", keep] = process.argv.slice(2);
const targets = await (await fetch("http://127.0.0.1:9222/json")).json();
const page = targets.find((t) => t.type === "page" && t.url.includes("localhost:1420"));
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const j = JSON.parse(m.data);
  pending.get(j.id)?.(j);
};
await new Promise((r) => (ws.onopen = r));
const ev = (expr) =>
  new Promise((res) => {
    const i = ++id;
    pending.set(i, (j) => res(j.result?.result?.value));
    ws.send(JSON.stringify({ id: i, method: "Runtime.evaluate", params: { expression: expr, returnByValue: true } }));
  });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

for (let i = 0; i < 120; i++) {
  if (await ev(`!!document.querySelector('.led.ready')`)) break;
  await sleep(1000);
}
const turnsBefore = keep === "keep" ? await ev(`document.querySelectorAll('.turn-meta').length`) : 0;
if (keep !== "keep") {
  await ev(`[...document.querySelectorAll('.icon-btn')].find(b => b.innerText.includes('새 대화'))?.click()`);
  await sleep(300);
}
await ev(`(() => {
  const ta = document.querySelector('.composer textarea');
  const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
  set.call(ta, ${JSON.stringify(text)});
  ta.dispatchEvent(new Event('input', { bubbles: true }));
  return true; })()`);
await sleep(200);
await ev(`document.querySelector('.send-btn').click()`);

let meta = null;
for (let i = 0; i < Number(waitSec); i++) {
  const metas = await ev(`[...document.querySelectorAll('.turn-meta')].map(e => e.innerText)`);
  if ((metas?.length ?? 0) > turnsBefore) {
    meta = metas[metas.length - 1];
    break;
  }
  await sleep(1000);
}
console.log("turn-time:", meta ?? "TIMEOUT");
console.log("tools:", await ev(`[...document.querySelectorAll('.tool-name')].map(e => e.innerText)`));
console.log("tool-states:", await ev(`[...document.querySelectorAll('.tool-dot')].map(e => e.className.replace('tool-dot ',''))`));
console.log(
  "answer:",
  ((await ev(`[...document.querySelectorAll('.prose')].map(e => e.innerText).join(' ')`)) ?? "").slice(0, 250).replace(/\n/g, " "),
);
ws.close();
process.exit(meta ? 0 : 1);
