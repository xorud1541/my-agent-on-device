# Local Agent — 단일 인스톨러 빌드 (사내 테스트 배포용)

`llama-server + LocalSearch 사이드카 + 모델`을 전부 동봉한 완전 오프라인 InnoSetup 셋업을 만든다.
앱이 사이드카를 자체 기동하므로 런처/EULA/서명 없이 단순하게 구성한다.

## 사전 요구
- Inno Setup 6 (`ISCC.exe`) — 이 PC: `%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe`
- 모델/바이너리 소스 (개발 PC 기본 경로, 다르면 `collect-payload.ps1` 파라미터로 지정):
  - llama Vulkan 빌드: `~\Downloads\llama-b9334-bin-win-vulkan-x64`
  - Qwen GGUF(+mmproj): `~\.lmstudio\models\lmstudio-community\Qwen3.5-2B-GGUF`
  - removeBG / harrier: `~\.alice\models`
  - localsearch-cli: `%LOCALAPPDATA%\alian\localsearch-cli.exe`
  - pdfium: `%LOCALAPPDATA%\pdf2md\pdfium-7690\pdfium.dll`

## 단계
```powershell
# 1) 프론트 빌드 (exe 에 embed)
pnpm build

# 2) 앱(프론트 임베드) + 캡처 헬퍼 빌드.
#    반드시 `tauri build` 를 쓴다 — 맨 `cargo build` 로 만든 exe 는 webview 가 프론트를
#    임베드하지 않고 devUrl(http://localhost:1420)을 바라봐, 설치본 실행 시
#    "이 페이지에 연결할 수 없습니다 / ERR_CONNECTION_REFUSED" 가 뜬다.
#    bundle.active=false 라 tauri build 는 exe 만 산출(인스톨러 번들은 skip).
pnpm tauri build
#    region-capture.exe 가 함께 안 나오면 별도로:  (현재는 tauri build 가 같이 산출)
#    cd src-tauri; cargo build --release --bin region-capture; cd ..

# 3) payload 수집 (llama / localsearch / models)
pwsh -File installer\collect-payload.ps1            # 비전 포함(기본)
# pwsh -File installer\collect-payload.ps1 -NoVision  # mmproj 제외(~670MB 절감)

# 4) 인스톨러 컴파일
& "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" installer\local-agent.iss
# → installer\Output\local-agent_0.1.0.exe  (약 2.5GB)
```

## 설치본 레이아웃 (config.rs 의 exe-상대 기본값이 가리키는 위치)
```
%LOCALAPPDATA%\local-agent\
├─ local-agent.exe
├─ region-capture.exe
├─ onnxruntime.dll
├─ llama\llama-server.exe + *.dll
├─ localsearch\localsearch-cli.exe + pdfium.dll
└─ models\Qwen3.5-2B-Q4_K_M.gguf, mmproj-*.gguf, removeBG.ort, harrier-v1-270m-onnx\
```
사용자 데이터(워크스페이스 / `config.json` / 색인 DB)는 설치 폴더 밖(사용자 폴더)에 남으며
제거 시 보존된다.

## 검증
깨끗한 Windows(또는 모델/엔진 소스가 없는 계정)에서 설치 → 실행 → llama-server·localsearch
자동 기동, 채팅·이미지·로컬검색 동작 확인. (오프라인)
