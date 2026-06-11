---
name: altools-module-deploy
description: ALTools 공용 모듈(DLL 등)을 CDN 테스트 서버에 배포. modules.json 엔트리 업데이트 또는 신규 추가, 모듈 파일 업로드, applist.json 체크섬 갱신을 수행한다. 모듈 배포, DLL 업로드, modules.json 갱신에 사용.
---

# deploy-altools-modules

ALTools 공용 모듈(DLL 등)을 CDN 테스트 서버(`test-aldn`)에 배포한다.

- **기존 모듈 업데이트**: modules.json에 이미 존재하는 엔트리의 파일/체크섬/버전을 갱신
- **신규 모듈 추가**: modules.json에 새로운 엔트리를 등록

## 사전 조건

- `CDN_USERNAME`, `CDN_PASSWORD` 환경변수 또는 `.env` 파일
- Python 의존성: pysftp, requests, python-dotenv
- (선택) pywin32 — DLL 버전 자동 추출용

### CDN 인증 정보 설정 방법

`CDN_USERNAME/CDN_PASSWORD is required` 오류가 발생하면 아래 방법 중 하나로 설정한다.

**방법 1 — 도구별 `.env` 파일 (권장)**

사용하는 AI 도구에 맞는 경로에 추가한다 (둘 다 있으면 `.claude/.env`가 우선):

| 도구 | 파일 경로 | 감지 환경변수 |
|------|----------|-------------|
| Claude Code | `<프로젝트루트>/.claude/.env` | `CLAUDE_CODE` |
| Codex CLI (OpenAI) | `<프로젝트루트>/.codex/.env` | `CODEX` |
| Gemini CLI (Google) | `<프로젝트루트>/.gemini/.env` | `GEMINI_CLI` |
| Cursor | `<프로젝트루트>/.cursor/.env` | `CURSOR` |

내용:
```
CDN_USERNAME=실제아이디
CDN_PASSWORD=실제비밀번호
```

> `.gitignore`에 해당 경로를 반드시 추가해 인증 정보가 커밋되지 않도록 한다.

**방법 2 — Windows 사용자 환경변수 (영구 설정)**

```powershell
[System.Environment]::SetEnvironmentVariable("CDN_USERNAME", "실제아이디", "User")
[System.Environment]::SetEnvironmentVariable("CDN_PASSWORD", "실제비밀번호", "User")
```

> 설정 후 **터미널(또는 Claude Code)을 재시작**해야 반영된다.

## 흐름

### 0단계: 정보 수집

사용자로부터 아래 정보를 수집한다.

#### 기존 모듈 업데이트 시

| 항목 | 필수 | 수집 방식 |
|------|------|----------|
| 모듈 파일 경로 | O | 사용자 제공 |
| 모듈 ID | - | `filename`으로 modules.json에서 자동 매칭. 동일 filename이 여러 엔트리에 존재하면 사용자에게 질문 |
| 버전 | △ | win32api로 DLL에서 자동 추출 시도. 실패 시 사용자에게 질문 |

#### 신규 모듈 추가 시

| 항목 | 필수 | 수집 방식 |
|------|------|----------|
| 모듈 파일 경로 | O | 사용자 제공 |
| 모듈 ID | O | 사용자 제공 |
| 원격 URL | O | 사용자 제공 (CDN 전체 URL 또는 상대 경로) |
| architecture | △ | x64일 때만 지정. 사용자 제공 |
| filename | △ | 미지정 시 로컬 파일명 사용 |
| version | △ | win32api 자동 추출, 실패 시 사용자에게 질문 |
| updaterate | △ | 기본값 100 |
| dir | △ | 설치 경로. 미지정 시 생략 |

### 1단계: 미리보기

정보를 수집한 후, 사용자 확인 없이 바로 미리보기를 실행한다.

```bash
SCRIPT_DIR="$(ls -d ~/.claude/skills/altools-module-deploy/scripts .claude/skills/altools-module-deploy/scripts 2>/dev/null | head -1)"

# 기존 모듈 업데이트
python "$SCRIPT_DIR/preview_modules.py" update <local_file> [--id <module_id>] [--version X.X.X.X]

# 신규 모듈 추가
python "$SCRIPT_DIR/preview_modules.py" add <local_file> --id <module_id> --url <remote_url> [--architecture x64] [--version X.X.X.X] [--filename name] [--updaterate N] [--dir <dir>]
```

스크립트는 세 가지를 출력한다:
1. **modules.json unified diff** — 변경된 부분만 하이라이트
2. **applist.json checksum 변경** — before/after
3. **전체 modules.json** — CDN에 업로드될 최종 파일

#### 표시 순서

**1) 경로 정보** — 추론된 경로를 테이블로 표시한다.

````
| 항목 | 값 |
|------|---|
| 모듈 로컬 경로 | `C:/path/to/module.dll` |
| 모듈 원격 URL | `https://test-aldn.altools.co.kr/altools/altoolsmanager/update/module.dll` |
| modules.json URL | `https://test-aldn.altools.co.kr/altools/altoolsmanager/appinfo/modules.json` |
| applist.json URL | `https://test-aldn.altools.co.kr/altools/altoolsmanager/appinfo/applist.json` |
````

**2) 변경 사항 요약** — diff 출력에서 변경된 필드만 추출하여 테이블로 표시한다. 긴 값(URL, checksum 등)은 앞뒤를 `...`으로 축약한다.

````
**변경 사항 요약**

| 필드 | 이전 | 이후 |
|------|------|------|
| checksum | `01bc6c3a...` | `281c70d4...` |
| version | `25.10.2.28` | `1.8.1.29` |
````

**3) 전체 modules.json diff** — 업로드될 파일의 변경된 줄을 `- old` / `+ new` 형식으로 인라인 표시한다.

````
```diff
 {
     "module": {
         "files": [
             ...
-            "checksum": "01bc6c3a...",
+            "checksum": "281c70d4...",
             ...
         ]
     }
 }
```
````

**4) applist.json checksum 변경** — 스크립트 출력의 applist.json checksum before/after를 표시한다.

````
**applist.json modules checksum**

| | 값 |
|------|---|
| 이전 | `1c5c23c6...` |
| 이후 | `72cbd42a...` |
````

스크립트 출력의 `[version]` 라인을 확인:
- 버전이 표시되면: "DLL에서 버전 X.X.X.X를 추출했습니다"라고 안내
- `[version]` 라인이 없거나 `(keeping existing: ...)` 형태면: DLL에서 버전을 추출할 수 없었음을 안내하고, 기존 값이 유지됨을 알림. 사용자가 버전 변경을 원하면 `--version`으로 재실행

사용자가 수정을 요청하면 반영하여 다시 실행한다. 승인하면 다음 단계로 진행한다.

### 2단계: 배포

```bash
SCRIPT_DIR="$(ls -d ~/.claude/skills/altools-module-deploy/scripts .claude/skills/altools-module-deploy/scripts 2>/dev/null | head -1)"

# 기존 모듈 업데이트
python "$SCRIPT_DIR/deploy_modules.py" update <local_file> [--id <module_id>] [--version X.X.X.X]

# 신규 모듈 추가
python "$SCRIPT_DIR/deploy_modules.py" add <local_file> --id <module_id> --url <remote_url> [--architecture x64] [--version X.X.X.X] [--filename name] [--updaterate N] [--dir <dir>]
```

1단계 미리보기와 동일한 인자를 사용한다.

배포 스크립트는 다음을 수행한다:
1. modules.json 다운로드 및 수정
2. 모듈 파일을 CDN에 업로드
3. 수정된 modules.json을 CDN에 업로드
4. applist.json의 modules checksum을 갱신하여 CDN에 업로드
5. 변경된 모든 URL의 CDN 캐시 퍼지

실행 결과를 사용자에게 표시한다.

`CDNAuthError` 발생 시, "사전 조건 > CDN 인증 정보 설정 방법" 섹션의 내용을 사용자에게 그대로 안내한다.

## 여러 모듈을 한 번에 처리할 때

x86/x64 쌍처럼 여러 모듈을 동시에 처리해야 하는 경우:
1. 첫 번째 모듈로 미리보기 실행
2. 두 번째 모듈도 미리보기 실행
3. 두 결과를 합쳐서 사용자에게 보여줌
4. 승인 후 각각 deploy 실행

단, 두 번째 deploy 실행 시 modules.json이 첫 번째 deploy로 이미 변경되어 있으므로 정상적으로 누적 적용된다.
applist.json의 modules checksum은 마지막 deploy가 최종값을 반영한다.
