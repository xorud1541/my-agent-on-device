; local-agent.iss — 단일 인스톨러 (검토용 초안, 2026-06-15)
; 설계: docs/superpowers/specs/2026-06-15-packaging-single-installer-design.md
;
; ⚠️ 미검증 초안 (Windows ISCC 필요). §9 결정 후 확정할 항목은 [DECISION] 주석으로 표시.
; 전제: collect-payload.ps1 로 installer\payload\{llama,localsearch,models,onnxruntime.dll} 준비됨.
; 빌드: ISCC.exe local-agent.iss   (코드사인 시 /DUSE_SIGNTOOL="ESTSign")

#define MyAppName "Local Agent"
#define MyAppExe  "local-agent.exe"
#define MyVersion "0.1.0"
; [DECISION #6] Tauri 번들을 끄고(bundle.active=false) 이 exe 를 ISCC 가 패키징한다고 가정.
#define AppExeSrc "..\src-tauri\target\release\local-agent.exe"
#define Payload   "payload"

[Setup]
AppId={{E57S0FT-0CA1-A6E7-0001-000000000001}}
AppName={#MyAppName}
AppVersion={#MyVersion}
; [DECISION #3] 설치 위치: per-user(관리자 불필요) 기본. Program Files 로 바꾸려면
;   DefaultDirName={autopf}\{#MyAppName} + PrivilegesRequired=admin 로 교체.
DefaultDirName={localappdata}\local-agent
PrivilegesRequired=lowest
Compression=lzma2/max
SolidCompression=yes
MinVersion=10.0
OutputDir=Output
OutputBaseFilename=local-agent_{#MyVersion}
SetupIconFile=..\src-tauri\icons\icon.ico
WizardStyle=modern
DisableProgramGroupPage=yes
CloseApplications=force
; [DECISION #5] 산출물이 2GB 초과면 GitHub Release 업로드 불가 → 아래 분할 활성화하거나 사내 호스팅.
;DiskSpanning=yes
;DiskSliceSize=1992294400
#ifdef USE_SIGNTOOL
SignTool={#USE_SIGNTOOL}
#endif

[Components]
; [DECISION #2] 선택 기능 동봉을 '설치 시점'으로 미룬다 — payload 에 해당 파일이 있을 때만 설치됨.
Name: "core";     Description: "앱 + 추론 + 로컬 검색 (필수)"; Types: full compact custom; Flags: fixed
Name: "vision";   Description: "이미지 판독(비전, +~671MB)";    Types: full
Name: "removebg"; Description: "AI 배경제거(+~116MB)";          Types: full

[Types]
Name: "full";    Description: "전체 설치"
Name: "compact"; Description: "최소 설치 (검색·채팅만)"
Name: "custom";  Description: "사용자 지정"; Flags: iscustom

[Files]
; 앱 + onnxruntime (exe 옆 — config.rs resolve_installed / ensure_ort_dylib 가 여기서 찾음)
Source: "{#AppExeSrc}";                 DestDir: "{app}";             Flags: ignoreversion; Components: core
Source: "{#Payload}\onnxruntime.dll";   DestDir: "{app}";             Flags: ignoreversion; Components: core
; 추론 엔진
Source: "{#Payload}\llama\*";           DestDir: "{app}\llama";       Flags: recursesubdirs ignoreversion; Components: core
; 검색 사이드카 + pdfium
Source: "{#Payload}\localsearch\*";     DestDir: "{app}\localsearch"; Flags: recursesubdirs ignoreversion; Components: core
; 모델 — 메인 + Harrier(검색)는 core, mmproj/removeBG 는 옵션 컴포넌트
Source: "{#Payload}\models\Qwen3.5-2B-Q4_K_M.gguf"; DestDir: "{app}\models"; Flags: ignoreversion; Components: core
Source: "{#Payload}\models\harrier-v1-270m-onnx\*"; DestDir: "{app}\models\harrier-v1-270m-onnx"; Flags: recursesubdirs ignoreversion; Components: core
Source: "{#Payload}\models\mmproj-Qwen3.5-2B-BF16.gguf"; DestDir: "{app}\models"; Flags: ignoreversion skipifsourcedoesntexist; Components: vision
Source: "{#Payload}\models\removeBG.ort"; DestDir: "{app}\models"; Flags: ignoreversion skipifsourcedoesntexist; Components: removebg

[Icons]
Name: "{group}\{#MyAppName}";        Filename: "{app}\{#MyAppExe}"
Name: "{commondesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "바탕화면 바로가기 생성"; GroupDescription: "추가 작업:"

[Run]
; 앱이 사이드카(llama/localsearch)를 스스로 기동하므로 별도 런처 불필요 — exe 만 실행
Filename: "{app}\{#MyAppExe}"; Description: "{#MyAppName} 실행"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; 설치 폴더만 제거. 사용자 데이터(워크스페이스/색인 DB/config.json)는 의도적으로 보존.
Type: filesandordirs; Name: "{app}"
