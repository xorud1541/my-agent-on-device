---
name: altools-setup-deploy
description: ALTools(알툴즈) 제품군의 설치 파일(셋업)을 CDN 서버(운영/테스트)에 배포. setup .exe 파일이 주어지면 파일명에서 앱과 버전을 자동 추론한다. 알툴즈 배포, CDN 업로드, appinfo 갱신, 셋업 배포에 사용.
---

# publish-altools-appinfo

ALTools 앱 업데이트를 CDN 테스트 서버(`test-aldn`)에 배포한다.
운영 서버 배포는 현재 미지원 (추후 확장 예정).

## 사전 조건

- `CDN_USERNAME`, `CDN_PASSWORD` 환경변수 또는 `.env` 파일
- Python 의존성: pysftp, requests, python-dotenv
- (선택) pywin32 — 자동 버전 추출용

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

> **참고**: `CLAUDE_CODE`는 공식 확인됨. 나머지 환경변수는 관례적 추정이므로 감지가 안 될 수 있다. 감지 실패 시 스크립트가 stderr에 경고 메시지와 함께 지원 도구 목록을 출력하므로, 방법 2를 사용한다.

**방법 2 — Windows 사용자 환경변수 (영구 설정)**

PowerShell에서 실행 (재시작 없이 영구 적용):

```powershell
[System.Environment]::SetEnvironmentVariable("CDN_USERNAME", "실제아이디", "User")
[System.Environment]::SetEnvironmentVariable("CDN_PASSWORD", "실제비밀번호", "User")
```

> 설정 후 **터미널(또는 Claude Code)을 재시작**해야 반영된다.

설정 완료 후 다시 배포를 시도하면 된다.

## 앱 이름 → CDN 경로 매핑

| 앱 이름 | appinfo 경로 | setup 경로 |
|---------|-------------|-----------|
| alcapture | `altools/altoolsmanager/appinfo/alcapture.json` | `altools/altoolsmanager/setup/<파일명>` |
| alzip | `altools/altoolsmanager/appinfo/alzip.json` | `altools/altoolsmanager/setup/<파일명>` |
| alpen | `altools/altoolsmanager/appinfo/alpen.json` | `altools/altoolsmanager/setup/<파일명>` |
| alpdf | `altools/altoolsmanager/appinfo/alpdf.json` | `altools/altoolsmanager/setup/<파일명>` |
| alsee | `altools/altoolsmanager/appinfo/alsee.json` | `altools/altoolsmanager/setup/<파일명>` |
| aldrive | `altools/altoolsmanager/appinfo/aldrive.json` | `altools/altoolsmanager/setup/<파일명>` |

CDN URL 베이스: `https://test-aldn.altools.co.kr/`

## 흐름

### 1단계: 미리보기

인자로 setup 파일 경로를 받는다. 파일명에서 앱 이름을 추론하고, 매핑 테이블로 CDN 경로를 자동 결정한다.

예: `ALPen_1.1.0.25.exe` → 앱=alpen

#### description/updatehistory 자동 제안

테스트 서버(`test-aldn`)의 경우, 다음 형식으로 `description`과 `updatehistory`를 자동 생성하여 `--pub`에 포함한다:

```
<앱이름> <버전> <YY.MM.DD> 테스트
```

예: `ALPen 1.1.0.25 26.03.10 테스트`

description과 updatehistory에 동일한 값을 사용한다.
운영 서버 배포는 현재 미지원 (추후 확장 시, 사용자에게 내용을 질문하는 방식으로 처리 예정).

#### 미리보기 실행

경로를 결정한 후, 사용자 확인 없이 바로 미리보기를 실행한다.

```bash
SCRIPT_DIR="$(ls -d ~/.claude/skills/altools-setup-deploy/scripts .claude/skills/altools-setup-deploy/scripts 2>/dev/null | head -1)"
python "$SCRIPT_DIR/preview_appinfo.py" <setup_local> --appinfo <appinfo_url> --remote <setup_remote_url> [--pub '<json>']
```

- `--pub`에는 자동 생성된 description/updatehistory와 사용자가 추가로 변경을 요청한 필드를 JSON으로 전달한다.
- 변경할 pub 필드가 없으면 `--pub` 옵션을 생략한다.

출력은 git diff 형식(unified diff)으로 표시된다. 변경된 줄은 `-`/`+`로, 변경되지 않은 줄은 컨텍스트로 함께 표시된다.

스크립트는 두 가지를 출력한다:
1. **unified diff** — 변경된 부분만 하이라이트 (ANSI 색상 적용: 삭제=빨간, 추가=초록)
2. **전체 appinfo JSON** — CDN에 실제 업로드될 최종 파일 전체

사용자에게는 **경로 정보 + 변경 사항 요약 + 전체 appinfo diff**를 순서대로 보여주고, 한 번만 확인을 요청한다.
콘솔 인코딩 문제로 한글이 깨질 수 있으므로, 깨진 한글 부분만 원본 값으로 대체하여 보여준다.

#### 표시 순서

**1) 경로 정보** — 추론된 경로를 테이블로 표시한다.

````
| 항목 | 값 |
|------|---|
| setup 로컬 경로 | `C:/Users/EST/Downloads/ALPen_1.1.0.25.exe` |
| setup 원격 URL | `https://test-aldn.altools.co.kr/altools/altoolsmanager/setup/ALPen_1.1.0.25.exe` |
| appinfo URL | `https://test-aldn.altools.co.kr/altools/altoolsmanager/appinfo/alpen.json` |
````

**2) 변경 사항 요약** — diff 출력에서 변경된 필드만 추출하여 테이블로 표시한다. 긴 값(URL, checksum 등)은 앞뒤를 `...`으로 축약하여 가독성을 높인다.

````
**변경 사항 요약**

| 필드 | 이전 | 이후 |
|------|------|------|
| version | `1.0.0.20` | `1.1.0.25` |
| setupurl | `...alpen_teest11.exe` | `...ALPen_1.1.0.25.exe` |
| checksum | `5689623e...` | `a0cf0562...` |
| description | `ALPen 1.0.0.20 26.03.10 테스트` | `ALPen 1.1.0.25 26.03.10 테스트` |
| updatehistory | `ALPen 1.0.0.20 26.03.10 테스트` | `ALPen 1.1.0.25 26.03.10 테스트` |
````

**3) 전체 appinfo diff** — 업로드될 파일 전체를 빠짐없이 보여주되, 변경된 줄은 `- old` / `+ new` 형식으로 인라인 표시한다.
`diff` 코드 블록을 사용하고, 변경되지 않은 줄은 접두사 없이(공백 한 칸) 그대로 표시한다.

````
```diff
 {
     "detailurl": "https://altools.co.kr/product/ALTOOLS",
     "pub": [
         {
             "major": 1,
-            "version": "1.0.0.20",
+            "version": "1.1.0.25",
             "minversion": "1.0.0.0",
             ...
         }
     ]
 }
```
````

스크립트 출력의 `[version]` 라인을 확인하여 버전 추출 결과를 사용자에게 안내한다:
- `[version]` 라인에 버전이 표시되면: "exe 파일에서 버전 X.X.X.X를 추출했습니다"라고 안내한다.
- `[version]` 라인이 없거나 비어 있으면 (win32api 없음): 사용자에게 버전을 물어보고 `--pub`의 JSON에 `"version": "x.x.x.x"`를 추가하여 다시 실행한다.

사용자가 수정을 요청하면 (경로 또는 appinfo 내용) 반영하여 다시 실행한다. 승인하면 다음 단계로 진행한다.

### 2단계: 배포

```bash
SCRIPT_DIR="$(ls -d ~/.claude/skills/altools-setup-deploy/scripts .claude/skills/altools-setup-deploy/scripts 2>/dev/null | head -1)"
python "$SCRIPT_DIR/deploy_to_cdn.py" <setup_local> --appinfo <appinfo_url> --remote <setup_remote_url> [--pub '<json>']
```

1단계 미리보기와 동일한 `--pub` 인자를 사용한다.
실행 결과를 사용자에게 표시한다.

`CDNAuthError` 발생 시, "사전 조건 > CDN 인증 정보 설정 방법" 섹션의 내용을 사용자에게 그대로 안내한다.

## pub 필드 원칙

- **자동 추출 (항상 업데이트)**: `version`, `major`, `checksum`, `setupurl` — setup 파일에서 자동 결정.
- **자동 제안**: `description`, `updatehistory` — 테스트 서버는 `<앱이름> <버전> <YY.MM.DD> 테스트` 형식으로 자동 제안, 운영 서버는 사용자에게 질문.
- **CDN 기존 값 유지 (기본)**: `updaterate`, `autoupdate`, `os` 등 — 사용자가 변경을 요청하지 않는 한 기존 값 유지.
- **사용자 요청 시 `--pub`로 전달**: 사용자가 특정 필드를 변경하고 싶을 때만 해당 필드를 JSON으로 전달.
