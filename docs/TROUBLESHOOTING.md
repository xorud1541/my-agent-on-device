# 트러블슈팅 사례집

1차 작업(2026-06-11) 중 실사용 로그로 잡은 버그들. 비슷한 증상이 재발하면 여기부터 본다.

## 로그 위치 (디버깅의 시작점)

```
%APPDATA%\com.estsoft.local-agent\logs\
├── chat_YYYYMMDD.jsonl   대화 로그: 턴별 ts/session_id/elapsed_ms/error/messages
└── llama-server.log      추론 서버 stdout/stderr (타이밍, reasoning-budget, 오류)
```

- 턴 중간에 emit 된 Error 이벤트도 `error` 필드에 기록된다 (`|`로 연결).
- llama-server.log에서 봐야 할 키워드: `print_timing`(속도), `reasoning-budget`,
  `Failed to parse`(툴콜 JSON), `exceed`(컨텍스트), `truncated`.

## 사례

### 1. llama-server 500 "Failed to parse tool call arguments as JSON"
- **증상**: 턴이 빨간 에러로 죽음. `last read: '"C:\Users\'`
- **원인**: 모델이 사용자 발화의 단일 백슬래시 Windows 경로를 JSON에 그대로 복사 → `\U` 는 invalid escape
- **수정**: ① 프롬프트 규칙 7 "경로는 슬래시(/)" ② 해당 500은 스트림 시작 전 반환되므로 최대 2회 재생성 재시도 (`client.rs::is_retryable_generation_error`)
- **잔존 리스크**: 3연속 실패 시 에러 표출 — 그 경우 다른 원인을 의심할 것

### 2. 폭주 생성 (한 턴이 2~3분)
- **증상**: `print_timing` n_decoded 가 1000+ 까지 증가, 턴 35초~사실상 무한
- **원인**: max_tokens 4096 + 도구 결과(파일 목록)를 모델이 표로 전부 재출력
- **수정**: `max_output_tokens`(기본 1024) + 프롬프트 규칙 8(목록 20개 초과 요약, 도구 결과 재출력 금지)

### 3. 생성 중 ■(중단) 버튼 무반응
- **원인**: 취소 플래그를 LLM 호출 사이에서만 검사
- **수정**: DeltaSink 가 bool 반환 — false 면 클라이언트가 SSE 연결 절단(서버 생성도 중단). 실측 0.5초

### 4. 컨텍스트 초과로 턴 사망 (400 exceed_context_size)
- **증상**: 멀티턴 + 큰 도구 결과 후 "request (13253 tokens) exceeds ..."
- **원인 체인**: 긴 한글 파일명 200개 list_dir × 동일 호출 2회 반복 × ctx 8192
- **수정**: ① 동일 (도구,인자) 반복을 코드에서 차단 ② 초과 시 오래된 도구 결과 압축(최근 2개 보존) 후 재시도 ③ list_dir 100개 상한 ④ 히스토리 클립 4000자 ⑤ ctx 기본 16384

### 5. 빈 응답 (본문도 툴콜도 없이 턴 종료)
- **증상**: 빈 말풍선 또는 "응답을 완성하지 못했습니다" 에러. 서버 로그에 `reasoning-budget: ... forcing end sequence` 직후 종료, 또는 활성화 후 토큰 <100에서 종료
- **원인**: thinking 모델이 사고로 출력 예산 소진 / 강제 사고 종료 직후 EOS 샘플링 (유도 메시지로도 불충분)
- **수정(최종)**: 기본 `--reasoning off` (reasoning_budget=0). 사고 없이 툴콜 5/5, 호출당 1~5초. 빈 완성은 1회 재생성 후 에러 표출
- **사고를 다시 켤 때**: 설정 reasoning_budget=N>0 → `--reasoning-budget N` + 유도 메시지 주입. 빈 응답 재발 가능성 있음을 인지할 것

### 6. 답변에 내부 규칙 중얼거림 ("이 질문은 도구 없이...")
- **원인**: 사고를 끈 모델이 판단 과정을 본문에 서술
- **수정**: 프롬프트 규칙 9 (규칙/판단 과정 언급 금지, 본론부터)

### 7. UI가 "로딩 중"에 고착 (서버는 정상)
- **원인**: 백엔드 ready 이벤트가 React 리스너 등록 전에 발행되는 레이스
- **수정**: `useAgent.ts` — ready 아닌 동안 2초마다 `server_status` 폴링 보정

## 개발 환경 함정 (이 PC 특유)

- 회사망 SSL: cargo는 `~/.cargo/config.toml`의 `check-revoke=false`, curl은 `--ssl-no-revoke`
- `python3`은 MS스토어 스텁 — 스크립트는 node 사용
- dev 앱 떠 있는 동안 `cargo build/test` 불가 (exe 잠금) — 앱 종료 후 빌드
- `pnpm tauri dev` 재시작 시 고아 vite가 1420 점유 가능 → `Get-NetTCPConnection -LocalPort 1420`으로 정리
