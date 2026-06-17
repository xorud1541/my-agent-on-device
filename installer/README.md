# Local Agent — 단일 인스톨러 (검토용 초안)

날짜: 2026-06-15 · 상태: **초안, 미검증** (Windows ISCC 빌드 필요)
설계: `docs/superpowers/specs/2026-06-15-packaging-single-installer-design.md`

모델 + 데스크탑앱 + llama-server + LocalSearch 사이드카를 **하나의 오프라인 셋업**으로 묶는다.
앱이 사이드카를 스스로 기동하므로 alian 식 런처/fetch-deps 가 필요 없다.

## 파일
- `collect-payload.ps1` — 흩어진 바이너리/모델을 `payload/{llama,localsearch,models}` 로 수집
- `local-agent.iss` — InnoSetup 스크립트 (`[Components]` 로 비전/배경제거 옵션)
- `payload/` — (생성됨, git 미추적 권장) 동봉 파일

## 빌드 순서 (Windows)
```powershell
# 0) 앱 빌드 (tauri bundle.active=false 가정) → src-tauri/target/release/local-agent.exe
pnpm tauri build   # 또는 cargo build --release

# 1) payload 수집 (소스 경로는 빌드 머신에 맞게)
pwsh -File collect-payload.ps1 `
  -LlamaDir "C:\llama-bin-win-vulkan-x64" `
  -LocalSearchCli "...\localsearch-cli.exe" -PdfiumDll "...\pdfium.dll" `
  -ModelsSrc "C:\models" -IncludeVision -IncludeRemoveBg

# 2) 인스톨러 빌드
ISCC.exe local-agent.iss        # 코드사인: ISCC.exe local-agent.iss /DUSE_SIGNTOOL="ESTSign"
# 산출물: Output/local-agent_<ver>.exe
```

## 이미 반영된 것
- `config.rs` 가 설치본 레이아웃(`<install>/llama|localsearch|models`, exe 옆 onnxruntime.dll)을
  **먼저 탐색**하고 없으면 개발 기본값으로 폴백 (커밋 9f160fc). 즉 위 레이아웃대로 깔리면 앱이 자동으로 찾음.

## 착수 전 결정 필요 (설계 §9)
1. 단일 풀-오프라인(기본) vs 슬림+fetch vs 둘 다
2. 비전(mmproj 671MB) 기본 동봉? → 현재 `[Components]` 의 vision 옵션으로 처리(설치 시 선택)
3. 설치 위치: `%LOCALAPPDATA%`(현재, 관리자 불필요) vs `Program Files`
4. 모델 양자화 Q4_K_M 유지?
5. 배포 호스팅: GitHub(2GB 한계→DiskSpanning) vs 사내 드라이브
6. Tauri 번들 `active=false`+InnoSetup(가정) vs NSIS 유지

## 검증 안 된 부분 (Windows 머신에서 확인)
- `.iss`/`.ps1` 문법·동작 (macOS 에서 작성, ISCC/pwsh 미실행)
- 깨끗한 Windows VM 에서 설치 → 오프라인 실행 → 사이드카·모델 탐색 → RAG 응답
