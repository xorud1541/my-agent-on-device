; =============================================================================
; Local Agent — Inno Setup 스크립트 (단일 풀-오프라인 동봉, 사내 테스트 배포용)
; =============================================================================
; 컴파일:  ISCC.exe local-agent.iss
; 산출물:  Output\local-agent_<MyVersion>.exe
;
; 사전 조건 (installer\BUILD.md 참고):
;   1) pnpm tauri build                   → target\release\local-agent.exe + region-capture.exe
;      (반드시 tauri build — 맨 cargo build 는 webview 가 devUrl 을 봐서 설치본이 깨진다)
;   2) installer\collect-payload.ps1      → installer\payload\{llama,localsearch,models}
;
; 우리 앱은 부팅 시 사이드카(llama-server, localsearch-cli)를 스스로 기동하므로
; alian 과 달리 VBS/PS1 런처가 필요 없다 — 바로가기는 local-agent.exe 를 직접 가리킨다.
; =============================================================================

#define MyAppName "Local Agent"
#define MyAppId   "local-agent"
#define MyAppExe  "local-agent.exe"
#define MyVersion "0.1.0"
#define MyPublisher "ESTsoft Corp."

[Setup]
AppId={{E57A0F70-10CA-4A6E-9C01-0000A6E07001}}
AppName={#MyAppName}
AppVersion={#MyVersion}
AppPublisher={#MyPublisher}
; per-user 설치 — 관리자 권한 불필요. {localappdata} 가 사용자 컨텍스트로 풀린다.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\{#MyAppId}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
DisableDirPage=yes
Compression=lzma2/max
SolidCompression=yes
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=Output
OutputBaseFilename=local-agent_{#MyVersion}
SetupIconFile=..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\{#MyAppExe}
; 재설치 시 실행 중인 앱/사이드카가 파일을 잠가도 강제 종료 후 덮어쓴다(프롬프트 제거).
CloseApplications=force
RestartApplications=no
WizardStyle=modern

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "바탕화면 바로가기 생성"; GroupDescription: "바로가기:"

[Files]
; --- 앱 본체 + 캡처 헬퍼 (빌드 산출물) ---
Source: "..\src-tauri\target\release\{#MyAppExe}"; DestDir: "{app}"; Flags: ignoreversion
; region-capture.exe: 영역 캡처 헬퍼. local-agent.exe 옆에서 찾으므로 {app} 루트에 둔다.
Source: "..\src-tauri\target\release\region-capture.exe"; DestDir: "{app}"; Flags: ignoreversion
; onnxruntime.dll: ort load-dynamic. exe 옆에서 찾고(image_ai), 사이드카엔 ORT_DYLIB_PATH 로 넘긴다.
Source: "..\src-tauri\vendor\onnxruntime\onnxruntime.dll"; DestDir: "{app}"; Flags: ignoreversion
; --- 추론 엔진 ---
Source: "payload\llama\*"; DestDir: "{app}\llama"; Flags: recursesubdirs ignoreversion
; --- 검색 사이드카 (+pdfium) ---
Source: "payload\localsearch\*"; DestDir: "{app}\localsearch"; Flags: recursesubdirs ignoreversion
; --- 모델 (대용량) ---
Source: "payload\models\*"; DestDir: "{app}\models"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExe}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; 설치 폴더만 정리. 사용자 데이터(워크스페이스/색인 DB/config)는 의도적으로 남긴다.
Type: filesandordirs; Name: "{app}"
