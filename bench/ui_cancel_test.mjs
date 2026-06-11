#!/usr/bin/env node
// 생성 도중 ■(중단) 버튼이 즉시 듣는지 검증:
// 긴 출력을 유도하는 발화 전송 → 텍스트가 흐르기 시작하면 중단 클릭 → 턴이 곧바로 끝나는지 확인
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
console.log("ready:", await ev(`!!document.querySelector('.led.ready')`));

// 긴 출력 유도 발화 입력 후 전송 (React controlled textarea — native setter 로 주입)
await ev(`(() => {
  const ta = document.querySelector('.composer textarea');
  const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
  set.call(ta, '1부터 300까지 숫자를 하나도 빠짐없이 전부 써줘');
  ta.dispatchEvent(new Event('input', { bubbles: true }));
  return true;
})()`);
await sleep(200);
await ev(`document.querySelector('.send-btn').click()`);

// 본문 텍스트가 흐르기 시작할 때까지 대기
let streaming = false;
for (let i = 0; i < 60; i++) {
  const len = await ev(`[...document.querySelectorAll('.prose')].map(e=>e.innerText).join('').length`);
  if (len > 20) { streaming = true; break; }
  await sleep(500);
}
console.log("streaming-started:", streaming);

// 중단 클릭
const t0 = Date.now();
await ev(`document.querySelector('.send-btn.stop')?.click()`);
let ended = false;
for (let i = 0; i < 30; i++) {
  if (await ev(`!!document.querySelector('.turn-meta')`)) { ended = true; break; }
  await sleep(500);
}
const cancelLatency = ((Date.now() - t0) / 1000).toFixed(1);
const textLen = await ev(`[...document.querySelectorAll('.prose')].map(e=>e.innerText).join('').length`);
console.log(`cancel-worked: ${ended} (중단→종료 ${cancelLatency}s, 출력 ${textLen}자에서 멈춤)`);
ws.close();
process.exit(ended && Number(cancelLatency) < 6 ? 0 : 1);
