# 테스트 & 벤치마크 가이드

## 단위 테스트 (모델 불필요, 수 초)

```bash
cd src-tauri
cargo test
```

26개: 도구 전체(tempdir 기반), 에이전트 루프(mock LLM — 툴 실행/실패 피드백/라운드 한도/
동일호출 차단/빈완성 재시도/컨텍스트 압축), 스트리밍 클라이언트(mock HTTP 서버 — 500 재시도/
즉시 실패/취소 절단), zip 왕복(한글 파일명, Zip Slip), 로깅.

주의: dev 앱이 떠 있으면 exe 잠금으로 빌드 실패. 앱 끄고 실행.

## E2E (실제 모델 구동, ~1분)

```bash
cd src-tauri
cargo test --test e2e_agent --release -- --ignored --nocapture --test-threads=1
```

시나리오 5종 + 레이턴시 단언(≤60초): 파일 검색 → 멀티턴 이미지 리사이즈 → 파일 읽고
내용 질문 → 화면 캡처 → 잡담(도구 미사용). 샌드박스는 `~/local-agent-e2e`에 자동 생성/정리.

## UI 자동 구동 (실행 중인 앱을 CDP로 조작)

앱을 디버그 포트와 함께 띄운다:

```bash
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" pnpm tauri dev
```

```bash
node bench/ui_send.mjs "발화 내용" [대기초=90] [keep]
#  keep 을 주면 새 대화로 초기화하지 않고 이어서 보냄 (멀티턴 테스트)
node bench/ui_cancel_test.mjs   # 생성 중 ■ 버튼이 즉시 듣는지 검증
node bench/ui_drive.mjs         # ready 대기 → 캡처 제안 클릭 → 턴 관찰 (스모크)
```

출력: turn-time, 사용된 도구, 도구 상태(ok/error), 답변 앞부분.
주의: ui_send 의 answer 는 화면의 모든 .prose 를 합친 것 — 정확한 턴 내용은 chat 로그로 확인.

## 모델 벤치마크

```bash
# 처리량 (prefill/decode t/s)
llama-bench.exe -m <model.gguf> --device Vulkan0 -ngl 99 -p 512 -n 128 -r 2

# 한국어 툴콜 정확도 5종 (검색/PDF/캡처/리사이즈/잡담)
node bench/toolcall_test.mjs <model.gguf> <라벨>
```

1차 측정 결과는 `docs/superpowers/specs/2026-06-11-local-agent-design.md` 의 벤치마크 표 참고.

## 로그 기반 분석

테스트(또는 사용자 테스트) 후:

```bash
# 최근 턴들 훑기 — elapsed/error/도구 호출 체인
node -e "const fs=require('fs');for(const l of fs.readFileSync('C:/Users/EST/AppData/Roaming/com.estsoft.local-agent/logs/chat_$(date +%Y%m%d).jsonl','utf8').trim().split('\n').slice(-5)){const j=JSON.parse(l);console.log('===',j.elapsed_ms+'ms',j.error??'');for(const m of j.messages)console.log(' ['+m.role+']',(m.content??'').slice(0,120),(m.tool_calls??[]).map(t=>t.function.name).join(','))}"
```

보는 지표: 턴당 elapsed(목표 ≤60s), error 필드, 도구 체인이 의도대로인지,
같은 도구 반복 여부, 도구 결과/답변 길이.
