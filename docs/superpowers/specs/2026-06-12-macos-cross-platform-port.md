# macOS 크로스플랫폼 포팅 — 설계 / 핸드오프

날짜: 2026-06-12
상태: 설계만 (코드 미적용). 실제 포팅은 macOS 장비에서 검증 가능할 때 착수.
대상 독자: 이 작업을 이어받아 맥 빌드를 실제로 띄울 다음 에이전트/개발자.

> ⚠️ 이 문서는 **분석과 제안**만 담는다. 코드는 한 줄도 바꾸지 않았다.
> Windows 빌드가 현재 정상 동작하므로, 검증 불가능한 환경(현재는 Windows 전용)에서
> 무턱대고 바꿔 Windows 경로를 깨뜨리지 않기 위함이다.

---

## 1. 결론 요약

- **아키텍처상 포팅 가능**하다. Tauri 2 + Rust + React 스택과 의존 크레이트 대부분이 macOS 1급 지원.
- **그대로는 실행 불가**. 막는 지점은 코드 로직이 아니라 ① 추론 백엔드(Vulkan→Metal),
  ② OS별 설정 기본값, ③ 번들 타깃(NSIS→dmg/app) 세 가지뿐.
- **모델**: 현재 Qwen3.5-2B 선정은 *Intel Arc iGPU + Vulkan 백엔드 제약의 산물*이다(§4 근거).
  맥(Metal·통합메모리)에서는 이 제약이 사라지므로 **같은 Qwen3.5 계열 안에서 체급을 올리는 것**이
  최저 리스크 경로다. RAM별 권장은 §4.3 표 참조.

---

## 2. 이미 크로스플랫폼인 부분 (변경 불필요)

| 영역 | 근거 |
|---|---|
| 프론트엔드 | React 19 + Vite. OS 무관. |
| 경로 처리 | `config.rs`가 `dirs::home_dir()`/`config_dir()` 사용 → OS별 자동 분기. config 저장 위치도 OK. |
| 의존 크레이트 | `image`, `xcap`(캡처), `trash`(휴지통), `rfd`(다이얼로그), `reqwest`, `lopdf`, `pdf-extract` 모두 macOS 지원. |
| ONNX(ort) | `Cargo.toml`에서 이미 OS 분기됨: Windows는 `load-dynamic`(MSVC LNK2038 회피), 그 외는 기본 링크. 맥은 ort가 onnxruntime 바이너리를 자체 링크하므로 별도 dll 불필요. |
| 콘솔창 숨김 | `server.rs:51` `CREATE_NO_WINDOW`가 `#[cfg(windows)]`로 이미 가드됨. |
| 사이드카 정리 | `server.rs` `kill_on_drop(true)` + `lib.rs:64` Exit 핸들러 → OS 무관 동작. |

---

## 3. 막는 지점과 제안 (코드 미적용)

### 3.1 추론 백엔드: Vulkan → Metal  ★최우선

현재 `config.rs`의 기본값은 Windows Vulkan 빌드를 가리킨다.

- `config.rs:77 default_server_exe()` → `~/Downloads/llama-b9334-bin-win-vulkan-x64/llama-server.exe`
  (Win 전용 Vulkan 빌드 + `.exe`)
- `config.rs:44 device: "Vulkan0"` → 맥엔 Vulkan iGPU 없음. Apple Silicon은 **Metal**.

**제안 A — OS별 기본값 분기** (`config.rs`):

```rust
#[cfg(target_os = "windows")]
fn default_server_exe() -> String {
    let home = dirs::home_dir().unwrap_or_default();
    home.join("Downloads").join("llama-b9334-bin-win-vulkan-x64")
        .join("llama-server.exe").to_string_lossy().into_owned()
}

#[cfg(target_os = "macos")]
fn default_server_exe() -> String {
    // Metal 백엔드 llama-server (확장자 없음). 사용자가 빌드/설치한 경로로 추후 조정.
    let home = dirs::home_dir().unwrap_or_default();
    home.join("llama.cpp").join("build").join("bin")
        .join("llama-server").to_string_lossy().into_owned()
}

/// 디바이스 기본값. Windows=Vulkan0, macOS=빈 값(Metal 자동 선택).
#[cfg(target_os = "windows")]
fn default_device() -> String { "Vulkan0".into() }
#[cfg(not(target_os = "windows"))]
fn default_device() -> String { String::new() }
```

그리고 `Default`의 `device: "Vulkan0".into()` → `device: default_device()`.

**제안 B — `--device` 조건부 부착** (`server.rs:28~38`):
현재 `--device {device}`를 항상 붙인다. device가 비면 `--device ""`로 llama-server가 죽는다.
Metal에선 디바이스를 지정하지 않는(=자동) 게 정석이므로:

```rust
// 배열에서 ["--device", &cfg.device] 제거 후:
if !cfg.device.trim().is_empty() {
    cmd.args(["--device", &cfg.device]);
}
```

> 참고: Metal은 `-ngl 99`만으로 GPU 오프로드가 자동 적용된다. `device` 빈 값 + 플래그 생략이면
> 같은 코드로 Windows(Vulkan0 명시)·맥(Metal 자동)을 모두 만족한다.

### 3.2 번들 타깃: NSIS → dmg/app

- `tauri.conf.json:30` `"targets": ["nsis"]` — NSIS는 Windows 전용.
- `tauri.conf.json:32` resources에 `onnxruntime.dll` — 맥에선 불필요(ort 자체 링크).

**제안 — 플랫폼별 설정 파일 분리** (Tauri 2 머지 규칙 활용):
`src-tauri/tauri.macos.conf.json` 신규 생성:

```json
{
  "bundle": {
    "targets": ["app", "dmg"]
  }
}
```

base `tauri.conf.json`은 Windows 그대로 둔다(머지로 맥만 override).
- ⚠️ **검증 필요**: Tauri 2의 객체 머지 규칙상 base의 `bundle.resources`(onnxruntime.dll)가
  맥 번들에도 남을 수 있다. 남더라도 ort가 사용 안 하므로 *무해한 잉여 파일*이지만,
  깔끔히 빼려면 `resources`를 base에서 `tauri.windows.conf.json`으로 옮기는 방법이 있다.
  단 이는 현재 동작하는 Windows 패키징을 건드리므로 맥 빌드 가능 시점에 함께 검증할 것.

### 3.3 변경 요약 체크리스트

- [ ] `config.rs`: `default_server_exe()` OS 분기, `default_device()` 신설, `Default` 수정
- [ ] `server.rs`: `--device` 조건부 부착
- [ ] `tauri.macos.conf.json` 신설 (targets: app/dmg)
- [ ] (선택) resources를 OS별로 분리해 맥 번들에서 dll 제외
- [ ] macOS Metal 빌드 llama-server 준비 + 경로 설정
- [ ] CLAUDE.md "Build Target: Windows" 전제 갱신
- [ ] 맥에서 모델 재벤치(§4.4) 후 기본 모델 확정

---

## 4. 모델 선정 (맥)

### 4.1 현재 2B 선정은 "백엔드 제약"의 결과다

`docs/.../2026-06-11-local-agent-design.md` §벤치 결과(82~108줄)의 핵심 발견:

1. **Vulkan(Intel iGPU)은 Q4_K 계열만 빠르다** — 같은 Qwen2B가 Q8_0 tg 16.4 / Q6_K 13.5 vs
   Q4_K_M 72.8. 고정밀 양자화로 품질↑ 경로가 이 백엔드에선 불성립.
2. **dense 4B는 대역폭 병목** — Qwen3.5-4B Q4 tg 8.9 (속도 탈락).
3. **gemma-4 계열은 에이전트 부적합** — 빠르지만 툴콜 대신 "어디에 저장할까요?" 되묻기. 교정 불가.
4. 결과: 속도·정확도·메모리(iGPU 공유 ~18GB)를 동시에 만족하는 유일점이 **Qwen3.5-2B-Q4_K_M**.

→ 즉 2B는 "iGPU에서 가능한 최선"이지, "이 작업에 필요한 최선"이 아니다.

### 4.2 맥(Metal)에서 바뀌는 것

- **통합메모리 대역폭이 iGPU보다 월등** (Apple Silicon 칩별 ~100~800GB/s) → §4.1의 (2) dense 4B
  병목이 대폭 완화. Vulkan에서 속도 탈락했던 Qwen3.5-4B가 맥에선 실용 후보로 부활.
- **Metal은 Q5/Q6/Q8을 효율적으로 처리** → §4.1의 (1) "Q4_K만 빠름" 절벽이 사라짐. 품질↑ 여지 생김.
- 메모리 예산은 칩 RAM(8/16/24/32GB+)에 종속.

### 4.3 RAM별 권장 (출발점, 벤치로 확정 전 가설)

| Mac RAM | 1순위 권장 | 비고 |
|---|---|---|
| 8GB | Qwen3.5-2B Q4_K_M | Windows와 동일. 안전빵. ctx 절약 필요. |
| 16GB | **Qwen3.5-4B Q4_K_M** | Vulkan에선 못 쓴 체급. 품질↑, Metal 대역폭으로 속도 확보. |
| 24GB+ | Qwen3.5-7B/8B Q4_K_M~Q5_K_M | 헤드룸 충분. 컨텍스트도 확대 가능. |

### 4.4 모델 선택 원칙 (체급보다 중요)

- **계열은 Qwen3.5로 고정** 권장. 이유:
  - 이미 한국어 툴콜 5/5 + 멀티턴 추종 검증됨(2B 기준). 같은 계열은 챗 템플릿·툴콜 파서 거동이 동일.
  - **CLAUDE.md 제약**: "system 메시지는 messages[0] 1개만 — Qwen 챗 템플릿이 중간 system을 400 거부".
    이 가정 자체가 Qwen 챗 템플릿에 묶여 있다. 타 계열로 바꾸면 `agent.rs` DIGEST 패턴·시스템
    프롬프트 규칙(특히 규칙11 능력 환각 완화)을 재검증해야 한다 → 리스크.
  - gemma-4 계열은 §4.1(3)대로 에이전트 부적합. 맥에서도 채택 금지 권장.
- **체급을 올린다 = 같은 계열에서 파라미터만 키운다**가 가장 안전한 업그레이드 경로.

### 4.5 확정 방법 (반드시 맥에서 재벤치)

위 표는 가설이다. 절대치는 칩/RAM/llama.cpp 버전에 종속되므로 **기존 하니스를 맥에서 그대로 재실행**해
확정할 것 (이 프로젝트의 검증된 진실원천):

- `bench/toolcall_test.mjs` — 한국어 툴콜 5종 정확도
- `gt_eval.mjs` (`GT_BASE` 환경변수로 후보 서버 평가) — GT 60건 에이전트 적합성
- `llama-bench` — pp/tg t/s (Metal)

판정 기준은 설계문서 그대로: ① 턴당(2~3 호출) 30~60초 예산 충족, ② 툴콜 정확도, ③ 멀티턴 추종,
④ RAM 풋프린트. 속도만 보고 gemma류로 가지 말 것(과거 실패 재현).

---

## 5. 리스크 / 열린 질문

- **검증 환경 부재**: 현재 개발 장비가 Windows라 맥 빌드/런타임을 여기서 확인 불가. 위 코드 제안은
  로직상 타당하나 실측 미검증. 맥 장비 확보 후 §3.3 체크리스트 순서로 진행.
- **Tauri resources 머지 거동**(§3.2)은 실제 빌드로 확인 필요.
- **xcap 권한**: macOS는 화면 캡처에 *화면 기록 권한*(시스템 설정) 동의가 필요. 첫 `screen_capture`
  호출 시 OS 권한 팝업 → 안내 UX 필요할 수 있음.
- **trash 휴지통**: macOS 휴지통 동작은 OK이나 동작 차이(권한/이름충돌) 스모크 테스트 권장.
- **코드사이닝/공증(notarization)**: 배포까지 가려면 Apple Developer 인증서 + notarize 필요(앱 실행 차단 회피). 별도 과제.

---

## 6. 다음 단계

1. 맥 장비에서 llama.cpp Metal 빌드 → `llama-server` 확보.
2. §3.3 체크리스트대로 config/server/tauri 분기 적용 (Windows 경로 회귀 없는지 `cargo test` 확인).
3. §4.5 하니스로 16GB 기준 Qwen3.5-4B vs 2B 재벤치 → 기본 모델 확정.
4. dmg 번들 → 실행 → 5종 시나리오 E2E(`e2e_agent.rs` 맥에서) 통과 확인.
