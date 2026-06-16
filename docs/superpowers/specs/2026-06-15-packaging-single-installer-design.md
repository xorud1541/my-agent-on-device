# 단일 인스톨러 패키징 전략 — 설계 / 핸드오프

날짜: 2026-06-15
상태: 설계만 (코드 미적용). 검토 후 착수.
대상: 곧 feat 브랜치들이 main 에 통합된 뒤, **모델 + 데스크탑앱 + llama-server + LocalSearch 사이드카**를
하나의 Windows 셋업(InnoSetup)으로 동봉 배포하기 위한 전략.

> ⚠️ 분석/제안만 담는다. 코드는 바꾸지 않았다.
> 참고: `~/Projects/alian` 의 InnoSetup 패키징(3분할 + fetch-deps).

---

## 0. 결론 요약 (먼저 읽기)

- **단일 오프라인 셋업은 가능하다.** 우리 앱은 시작 시 사이드카(llama-server, localsearch-cli)를
  **스스로 기동**하므로(`lib.rs` setup 훅), alian 처럼 외부 PowerShell 런처가 필요 없다. 셋업은
  "파일을 올바른 위치에 깔기 + 바로가기" 만 하면 된다.
- **가장 큰 비용은 용량**이다. 전부 동봉하면 압축 후 **약 2.2~2.8 GB** 인스톨러가 된다(GGUF·ONNX 는
  이미 엔트로피 인코딩되어 거의 안 줄어듦). 이건 하드웨어가 아니라 배포·업데이트 비용 문제다.
- **단일 동봉이 우리 타깃과 맞다.** 정책서의 B2B **폐쇄망** 시나리오는 인터넷 fetch 가 불가하므로,
  alian 의 Google Drive fetch-deps 모델보다 **완전 오프라인 단일 셋업**이 오히려 적합하다.
- **핵심 선결 작업은 코드 한 곳**이다: `config.rs` 의 기본 경로(현재 `~/Downloads`, `~/.lmstudio`,
  `~/.alice` 등 개발 PC 경로)를 **설치 위치(exe 기준 상대경로)** 로 바꾸는 것. 이게 되면 셋업은 단순해진다.

---

## 1. 동봉 대상 산출물과 크기

| 구성요소 | 파일 | 대략 크기 | 압축성 | 필수? |
|---|---|---|---|---|
| 데스크탑 앱 | `local-agent.exe` (web/dist embed) | ~15~30 MB | 보통 | ✅ |
| ONNX 런타임 | `onnxruntime.dll` | ~13 MB (현 vendor) | 보통 | ✅ (배경제거·Harrier) |
| 추론 엔진 | `llama-server.exe` + `ggml-*.dll` (Vulkan) | ~100 MB | 보통 | ✅ |
| 메인 모델 | `Qwen3.5-2B-Q4_K_M.gguf` | ~1.3~1.5 GB | **거의 안 줄어듦** | ✅ |
| 비전 어댑터 | `mmproj-Qwen3.5-2B-BF16.gguf` | ~671 MB | 안 줄어듦 | ⚠️ 선택(이미지 판독) |
| 검색 사이드카 | `localsearch-cli.exe` (release) | ~20 MB | 보통 | ✅(로컬검색) |
| 임베딩 모델 | `harrier-v1-270m-onnx/` (onnx+data+tokenizer) | ~360 MB | 안 줄어듦 | ✅(로컬검색) |
| PDF 렌더 | `pdfium.dll` | ~10 MB | 보통 | ✅(검색의 PDF 색인) |
| 배경제거 모델 | `removeBG.ort` | ~116 MB | 안 줄어듦 | ⚠️ 선택(배경제거) |

**합계(압축 전)**: 비전 포함 ~2.6~3.0 GB / 비전 제외 ~2.0~2.3 GB.
**인스톨러(LZMA 후)**: 대략 **2.2~2.8 GB** (모델이 대부분이라 압축 이득 작음).

> 참고: 속도(인덱싱·추론)는 §본문 외 — 별도 측정 문서대로 하드웨어(CPU/iGPU) 의존. 패키징과 무관.

---

## 2. alian(3분할) vs 우리(단일 동봉) 비교

| 항목 | alian: 3분할 + fetch-deps | 우리 목표: 단일 동봉 |
|---|---|---|
| 셋업 크기 | ~32 MB (슬림) | ~2.5 GB |
| 모델/엔진 | Google Drive zip, 최초 실행 시 fetch | 셋업에 동봉 |
| 인터넷 필요 | 최초 1회 필요 | **불필요(완전 오프라인)** |
| 폐쇄망(B2B) | ✗ fetch 불가 | ✅ 적합 |
| 앱 코드 업데이트 | 셋업만 재배포(작음) | 매번 2.5 GB 재배포 |
| 모델 독립 교체 | 가능(zip만 교체) | 셋업 재빌드 |
| 배포 호스팅 | Drive(대용량 OK) | GitHub Release **2 GB/파일 한계** 주의 |
| 런처 | VBS→PS1 (llama 기동·GPU 정책·readiness) | **불필요**(앱이 자체 기동) |

**판단**: 폐쇄망/시연/일괄배포엔 단일 동봉이 유리. 잦은 앱 업데이트엔 불리. → 둘 다 원하면
"풀 오프라인 셋업"을 기본으로 하고, 추후 "슬림+fetch" 변형을 옵션으로 둘 수 있다(§9).

---

## 3. 우리만의 결정적 단순화: 런처가 필요 없다

alian 은 데스크탑 앱이 `ALICE_SERVING_URL`(이미 떠 있는 서버)을 기대해서, **외부 PS1 런처**가
llama-server 를 먼저 띄우고 readiness 를 기다린 뒤 앱을 실행한다.

우리는 다르다 — `lib.rs` 의 setup 훅이 앱 부팅 시:
- `start_server_inner` → `LlamaServer::start`(llama-server 스폰 + /health 대기)
- `start_localsearch_inner` → 워크스페이스 인덱싱 + `LocalSearchServer::start`(serve 스폰)

즉 **앱 실행 = 사이드카 자동 기동**. 따라서 셋업은 VBS/PS1 런처, GPU 정책 스크립트, env 주입이
**전부 불필요**하고, 바탕화면 바로가기는 그냥 `local-agent.exe` 를 직접 가리키면 된다.
(GPU 디바이스 선택은 현재 `config.device`=Windows `Vulkan0`. dGPU/iGPU 정책이 필요하면 설정 패널이나
config 로 처리 — 런처 없이도 가능.)

---

## 4. 권장 설치 레이아웃 (단일 폴더, exe 상대)

핵심은 "앱이 찾는 모든 것을 설치 폴더 안 예측 가능한 위치에" 두는 것. 사용자 데이터(워크스페이스,
config, 색인 DB)는 기존대로 사용자 폴더.

```
%LOCALAPPDATA%\local-agent\            ← 설치 루트 (per-user, 관리자 불필요)
├─ local-agent.exe                     앱
├─ onnxruntime.dll                     (exe 옆 — ensure_ort_dylib 가 이미 여기서 찾음)
├─ llama\
│  ├─ llama-server.exe
│  └─ ggml-*.dll (vulkan/cpu)
├─ localsearch\
│  ├─ localsearch-cli.exe
│  └─ pdfium.dll                       (localsearch-cli 옆 — pdfium 로더가 찾음)
└─ models\
   ├─ Qwen3.5-2B-Q4_K_M.gguf
   ├─ mmproj-Qwen3.5-2B-BF16.gguf      (선택)
   ├─ removeBG.ort
   └─ harrier-v1-270m-onnx\            (localsearch_models_dir 가 이 부모를 가리킴)
      ├─ model_quantized.onnx
      ├─ model_quantized.onnx_data
      └─ tokenizer.json

%USERPROFILE%\ (또는 config_dir)        ← 사용자 데이터 (셋업이 건드리지 않음)
├─ <workspace>\                         사용자 지정 작업 폴더(인덱싱 대상)
├─ AppData\Roaming\com.estsoft.local-agent\config.json
└─ AppData\Local\local-agent\localsearch\   색인 DB (default_index_db_dir)
```

> onnxruntime 은 `ensure_ort_dylib`(image_ai.rs)가 이미 exe 옆 → vendor 순으로 찾으므로 exe 옆 배치면 끝.
> localsearch 사이드카에는 우리가 `ORT_DYLIB_PATH` 를 넘긴다 → config `ort_dylib` 를 exe 옆 dll 로 지정.

---

## 5. 선결 코드 변경 (패키징의 80%)

현재 `config.rs` 기본값은 **개발 PC 경로**를 가리켜, 그대로 배포하면 신규 PC에서 아무것도 못 찾는다.
**설치 레이아웃(§4)에 맞춘 exe-상대 기본 경로 resolver** 가 필요하다.

### 5.1 `config.rs` — "설치 위치 우선" 해석

각 `default_*` 를 다음 우선순위로:
1. **exe 기준 상대경로**(설치본): `current_exe()/../llama/llama-server.exe` 등 — 존재하면 채택
2. 기존 개발 기본값(`~/Downloads`, `~/.lmstudio`, `~/.alice`) — 폴백
3. 사용자가 config.json 으로 덮어쓰기 (최우선)

대상 필드: `server_exe`, `model_path`, `mmproj_path`, `removebg_model`,
`localsearch_bin`, `localsearch_models_dir`, `ort_dylib`.

의사코드:
```rust
fn installed(rel: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let p = dir.join(rel);
    p.exists().then_some(p)
}
fn default_server_exe() -> String {
    installed("llama/llama-server.exe")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(dev_fallback_server_exe)
}
// model_path → installed("models/Qwen3.5-2B-Q4_K_M.gguf"), ...
// ort_dylib(Windows) → installed("onnxruntime.dll")  // 사이드카에 넘길 명시 경로
```
> 이미 `localsearch::LocalSearchConfig::from_app` 은 config 우선·env 폴백 구조라, config 기본값만
> 설치경로로 바뀌면 자연히 동작한다.

### 5.2 Tauri 번들러 끄고 InnoSetup 사용 (alian 방식)

`tauri.conf.json`:
- 현재 `bundle.targets=["nsis"]`, `resources` 에 onnxruntime.dll 1개.
- 단일 InnoSetup 으로 갈 거면 alian 처럼 **`bundle.active=false`** 로 두고(또는 NSIS 유지하되 미사용),
  Tauri 는 `local-agent.exe` 만 만들고 InnoSetup 이 전부 패키징.
- macOS 는 `tauri.macos.conf.json` 의 dmg/app 그대로(별개 트랙).

### 5.3 vendor 디렉토리에 산출물 수집

빌드 머신에서 `src-tauri/vendor/`(또는 `installer/payload/`) 아래에 llama/ localsearch/ models/ 를
모아둔다(또는 빌드 스크립트가 수집). InnoSetup `[Files]` 가 이 트리를 설치 루트로 복사.

---

## 6. InnoSetup 스크립트 골격 (`installer/local-agent.iss`)

alian `alian.iss` 를 기반으로, 런처 제거·단일 동봉으로 단순화한 형태:

```inno
#define MyAppName "Local Agent"
#define MyAppExe  "local-agent.exe"
#define MyVersion "0.1.0"

[Setup]
AppId={{E5TS0FT-LOCAL-AGENT-0001}}
AppName={#MyAppName}
AppVersion={#MyVersion}
DefaultDirName={localappdata}\local-agent     ; per-user, 관리자 불필요
PrivilegesRequired=lowest
Compression=lzma2/max
SolidCompression=yes
MinVersion=10.0
OutputBaseFilename=local-agent_{#MyVersion}
SetupIconFile=..\src-tauri\icons\icon.ico
; DiskSpanning=yes / DiskSliceSize=...   ; ← GitHub 2GB 한계 회피가 필요하면

[Files]
; 앱 + onnxruntime (Tauri 산출물)
Source: "..\src-tauri\target\release\{#MyAppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\src-tauri\vendor\onnxruntime\onnxruntime.dll"; DestDir: "{app}"; Flags: ignoreversion
; 엔진
Source: "payload\llama\*"; DestDir: "{app}\llama"; Flags: recursesubdirs ignoreversion
; 검색 사이드카 + pdfium
Source: "payload\localsearch\*"; DestDir: "{app}\localsearch"; Flags: recursesubdirs ignoreversion
; 모델 (대용량)
Source: "payload\models\*"; DestDir: "{app}\models"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"
Name: "{commondesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "바탕화면 바로가기 생성"

[Run]
Filename: "{app}\{#MyAppExe}"; Description: "지금 실행"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}"
; 주의: 사용자 데이터(워크스페이스/색인 DB/config)는 의도적으로 삭제하지 않음
```

런처(VBS/PS1)·GPU 정책·fetch-deps·deps-manifest **모두 불필요**(§3). alian 대비 셋업이 크게 단순해진다.

---

## 7. 용량 문제와 완화책

1. **비전 어댑터(mmproj 671MB) 옵션화** — 이미지 판독을 안 쓰면 빼서 ~670MB 절감.
   InnoSetup `[Components]` 로 "이미지 기능(비전)" 선택 설치 가능.
2. **배경제거(removeBG 116MB)·로컬검색(Harrier 360MB)** 도 컴포넌트로 분리 가능(기능 옵트인).
3. **GitHub Release 2 GB/파일 한계** — 2.5GB 단일 exe 는 업로드 불가. 대안:
   - InnoSetup **DiskSpanning**(분할 .bin) 또는 7z 분할
   - 사내 팀드라이브/파일서버 호스팅(폐쇄망이면 어차피 내부 배포)
4. **압축**: LZMA2/max + Solid. 단 GGUF/ONNX 는 거의 안 줄어듦 → 기대치 낮게.
5. **모델 경량화**: 필요시 더 낮은 양자화(Q4_K_S 등). 단 품질 트레이드오프(별도 검토).

---

## 8. 빌드 파이프라인 (제안)

```
1) 프론트:  pnpm build                      → web/dist
2) 앱:      pnpm tauri build (bundle.active=false) 또는 cargo build --release
            → target/release/local-agent.exe
3) payload 수집(스크립트):
   - llama-server(Vulkan win) + ggml dll  → installer/payload/llama/
   - localsearch-cli.exe(release) + pdfium.dll → installer/payload/localsearch/
   - Qwen3.5-2B-Q4_K_M.gguf (+mmproj) , removeBG.ort, harrier-v1-270m-onnx/
                                             → installer/payload/models/
   - onnxruntime.dll                        → src-tauri/vendor/onnxruntime/ (기존)
4) ISCC.exe installer/local-agent.iss [/DUSE_SIGNTOOL=...]
   → installer/Output/local-agent_<ver>.exe
```
- CI(.github/workflows) 없음 → 후속으로 windows-latest 러너에서 위 단계 자동화 권장.
- 코드사이닝: alian 처럼 `USE_SIGNTOOL` 조건부. unsigned 면 SmartScreen 경고(알파 정상).

---

## 9. 선택지 / 결정 필요

1. **단일 풀-오프라인** (권장 기본, 폐쇄망 적합) vs **슬림+fetch**(alian식) vs **둘 다 제공**.
2. **비전(mmproj) 동봉 여부** — 671MB. 기본 포함? 컴포넌트 옵션?
3. **설치 위치** — `%LOCALAPPDATA%\local-agent`(per-user, 관리자 불필요, 권장) vs `Program Files`(관리자 필요).
4. **모델 양자화 고정** — Q4_K_M 유지? (용량/품질)
5. **배포 호스팅** — GitHub(2GB 한계 → 분할) vs 사내 드라이브.
6. **Tauri 번들** — `active=false`+InnoSetup(alian식, 권장) vs NSIS 유지+resources 확장.

---

## 10. 착수 순서 (제안 체크리스트)

- [x] `config.rs` 기본 경로를 exe-상대 설치 레이아웃 우선으로 (§5.1) — **완료** (커밋 9f160fc)
      `resolve_installed`/`installed` 헬퍼 + 각 default_* 에 설치본 우선·개발 폴백. TDD 통과.
      (남은 검증: 실제 설치본처럼 폴더 구성 후 Windows 에서 앱 실행 → 사이드카·모델 탐색 확인)
- [ ] `tauri.conf.json` `bundle.active=false`(InnoSetup 채택 시)
- [ ] `installer/` 디렉토리 + `local-agent.iss` 작성 (§6)
- [ ] payload 수집 스크립트(`installer/collect-payload.ps1`) — 바이너리/모델 모으기
- [ ] (선택) `[Components]` 로 비전/배경제거/로컬검색 옵션화 (§7)
- [ ] 빌드 1회 수행 → 깨끗한 Windows VM 에서 설치·실행 검증 (오프라인)
- [ ] (후속) CI 자동화, 코드사이닝, 배포 호스팅(2GB 분할)

> 참고 레퍼런스: alian `installer/alian.iss`(InnoSetup 디렉티브), `installer/package-deps.ps1`
> (대용량 zip·SHA256), `installer/fetch-deps.ps1`(우리는 미사용이지만 슬림 변형 시 참고).
> 우리 측 근거: `lib.rs` setup 훅(사이드카 자동 기동), `config.rs` 기본 경로, `image_ai.rs` `ensure_ort_dylib`.
