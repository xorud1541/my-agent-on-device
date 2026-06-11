---
name: altools-ci-generator
description: >
  altools-ci.yml GitHub Actions 워크플로우를 생성한다.
  team-util/altools-ci 재사용 워크플로우를 활용하여 check → build →
  packaging → upload 파이프라인을 구성한다.
  사용자가 "altools-ci"를 명시적으로 언급한 경우에만 트리거한다:
  "altools-ci 연동", "altools-ci 만들어줘", "altools-ci 생성",
  "altools-ci 설정", "altools-ci.yml 생성".
---

# ALTools Release Pipeline 생성

`team-util/altools-ci` 재사용 워크플로우와 액션을 활용하여 ESTsoft Windows
데스크톱 앱의 `.github/workflows/altools-ci.yml`을 생성합니다.

## 1단계: 프로젝트 구조 파악 (코드베이스 탐색)

파일을 작성하기 전에 반드시 다음을 확인하세요.

### 빌드 시스템 감지

프로젝트 루트에서 빌드 시스템을 먼저 확인합니다:

```
CMakeLists.txt 존재  → CMake 빌드 경로
*.sln 파일만 존재    → Visual Studio 빌드 경로
```

빌드 시스템에 따라 이후 단계의 빌드 명령, 사용자 질문, 빌드 출력 경로가 달라집니다.

### 프로젝트 자동 파악

빌드 시스템과 무관하게 확인해야 하는 항목:

```
<패키징 디렉토리>/*.iss → Inno Setup 패키징 스크립트 위치 (dist/, setup/ 등)
<패키징 디렉토리>/bin/  → git-tracked 의존성 파일 구조
.github/workflows/      → 기존 워크플로우 유무 (참고용)
```

**CMake 프로젝트** 추가 확인:

- `CMakeLists.txt`에서 `CMAKE_RUNTIME_OUTPUT_DIRECTORY` → 빌드 결과물 경로 (미설정 시 CMake 기본 규칙, 프로젝트별 상이)
- 디버그 심볼 출력 경로 — 빌드 출력 경로와 다를 수 있으므로 프로젝트를 탐색하여 실제 경로를
  파악할 것. 탐색으로 확인되지 않으면 사용자에게 질문
- Qt 사용 여부 — `find_package(Qt...)` 존재 시 Qt 버전 확인 → `CMAKE_PREFIX_PATH` 결정
- 기타 외부 의존성 — `find_package(...)`, `FetchContent`, vcpkg 등의 의존성 관리 방식

**.sln 프로젝트** 추가 확인:

- `.sln` 파일명, 포함된 프로젝트 구조
- 빌드 출력 경로 — 다음 순서로 확인: (1) `.vcxproj`에서 `<OutDir>` 태그 검색,
  (2) 기존 CI 스크립트나 빌드 스크립트에서 출력 경로 추적, (3) `.gitignore`에서
  빌드 출력 디렉토리 패턴 확인, (4) 프로젝트 문서(CLAUDE.md 등)가 있으면 참고
- 디버그 심볼 출력 경로 — `.exe`와 같은 경로가 아닌 별도 서브디렉토리(예: `DebugInfo\`)일 수
  있으므로 프로젝트를 탐색하여 실제 경로를 파악할 것. 탐색으로 확인되지 않으면 사용자에게 질문
- 사용하는 MSVC 툴셋 버전과 플랫폼 (예: v140, Win32)

**공통 확인:**
`git ls-files <패키징 디렉토리>/` 로 어떤 파일이 git에 추적되는지 확인합니다.
Qt DLL, altools DLL, redist 파일들이 이미 git에 있다면 CI에서 windeployqt를 실행하지 않아도 됩니다.

### ISS 파일 분석

ISS 파일의 `Source:` 경로를 전부 분석하여 패키징에 필요한 파일 범위를 파악합니다.
이 분석 결과는 이후 sparse-checkout 판단과 artifact 배치 경로 결정에 사용됩니다.

확인 항목:

- `#define OutputDir` → 패키징 결과물 출력 경로 (예: `.\output`, `.\setup`)
- `[Files]` 섹션의 `Source:` 경로 → 패키징에 참조되는 모든 파일 위치
- `#include` 경로 → Inno Setup include 의존성 (보통 CompilerPath 기준)
- `signonce` 플래그가 있는 파일 → 코드서명 대상

### 사용자에게 확인할 사항 (AskUserQuestion 사용)

빌드 시스템에 따라 적절한 질문을 **한 번에** 물어보세요.

**선택지 중립성 규칙**: AskUserQuestion의 선택지 label이나 description에 "(권장)",
"(Recommended)", "추천" 등의 편향 표현을 임의로 추가하지 마세요. 이 스킬 텍스트에서
특정 옵션에 "(권장)"이 명시적으로 표기된 경우에만 그대로 반영하세요. 프로젝트 상태
(예: exe가 이미 git-tracked)를 근거로 임의 판단하여 권장 표시를 붙이면 안 됩니다.

**공통 질문:**

1. **GitHub 리포지토리 경로** — `org/repo` 형식. AskUserQuestion 선택지는
   `est-altools/<repo-name>`과 `team-util/<repo-name>` 2개를 제공 (Other로 직접 입력 가능)
2. **SharePoint 루트 경로** (`shared_point_root`) — upload.yml에 전달되는 최상위 폴더명.
   AskUserQuestion 선택지는 프로젝트에서 감지한 제품명 1개만 제공 (예: `ALCapture`, `ALPen`).
   Other로 직접 입력 가능
3. **Jira 연동 여부** — Teams 알림에 Jira 이슈 정보를 포함할지
4. **`build=false` 바이너리 소스** — 빌드 없이 패키징할 때 바이너리를 어디서 가져올지.
   패키징 디렉토리에 exe가 git 추적되어 있더라도 반드시 사용자에게 물어볼 것 (전략을 바꾸고 싶을 수 있음).
   AskUserQuestion 선택지:
   - **레파지토리 커밋 방식** — `<패키징디렉토리>/bin/`에 커밋된 exe를 그대로 사용
   - **GitHub Release 방식** — 해당 버전의 GitHub Release에서 서명된 exe를 다운로드 (release job 추가 필요)

**CMake 프로젝트 추가 질문:** 4. **빌드 환경** — 빌드 runner 이름 (예: `sign-worker`). Qt를 사용하는 프로젝트인 경우
Qt 설치 경로도 확인 (예: `C:\Qt\Qt6.9\Qt6.9\6.9.2\msvc2022_64`). Qt installer는
지정한 base 경로 아래에 버전명 폴더를 한 단계 더 생성하므로 실제 경로를 확인할 것.
기타 외부 의존성(vcpkg, Conan 등)이 있다면 runner에서의 설치 경로도 함께 확인

**.sln 프로젝트 추가 질문:** 4. **devenv.com 경로** — sign-worker에서 Visual Studio의 devenv.com 경로. 두 가지 선택지 제공:

- vswhere로 자동 탐색 — runner에 여러 VS가 있거나 경로가 불확실할 때
- 하드코딩 경로 — 경로를 정확히 아는 경우 (예: `${env:ProgramFiles(x86)}\Microsoft Visual Studio\2019\Professional\Common7\IDE\devenv.com`)

## 2단계: 파이프라인 구조

`build=false` 전략에 따라 파이프라인 구조가 달라집니다.

**레파지토리 커밋 방식:**

```
check ──→ build ──→ packaging ──→ upload
                └──→ upload-symbols
```

**GitHub Release 방식** — `release` job 추가:

```
check ──→ build ──→ packaging ──┬─→ release
                               └─→ upload
                └──→ upload-symbols
```

| Job              | Runner         | 역할                            | 주요 재사용 컴포넌트                         |
| ---------------- | -------------- | ------------------------------- | -------------------------------------------- |
| `check`          | windows-latest | 시맨틱 버전 검증                | `altools-ci/.github/workflows/check.yml@v1`  |
| `build`          | sign-worker    | 빌드 (빌드 시스템에 따라 다름)  | —                                            |
| `packaging`      | sign-worker    | 코드 서명 + Inno Setup 패키징   | `sign-files@v1`, `run-altools-packaging@v1`  |
| `release`        | sign-worker    | GitHub Release 생성 + 에셋 첨부 | — (GitHub Release 방식만)                    |
| `upload-symbols` | upload-worker  | 디버그 심볼 SharePoint 업로드   | `shared-point-upload@v1`                     |
| `upload`         | upload-worker  | SharePoint 업로드 + Teams 알림  | `altools-ci/.github/workflows/upload.yml@v1` |

## 3단계: 워크플로우 파일 작성

### 파일 위치

`.github/workflows/altools-ci.yml`

### 워크플로우 이름

`name:` 필드의 기본값은 `altools-ci`입니다. 파일 작성 전에 반드시 기존 워크플로우
이름과 충돌 여부를 확인하세요:

```bash
grep -h "^name:" .github/workflows/*.yml
```

같은 이름(`altools-ci`)이 이미 존재하면 사용자에게 다른 이름을 물어보세요.
GitHub Actions UI에서 워크플로우는 `name:` 값으로 식별되므로, 중복 이름은
혼동을 유발할 수 있습니다.

### workflow_dispatch inputs

아래 블록을 그대로 복사해서 사용하세요. `build`의 `description`만 전략에 따라 선택합니다.
나머지 `description` 문자열은 바꾸거나 요약하지 않습니다:

```yaml
on:
  workflow_dispatch:
    inputs:
      version:
        description: '시맨틱 버전 MAJOR.MINOR.PATCH[-prerelease][+build] — 생략 시 마지막 커밋에서 추출 (예: 1.0.0, 1.0.0-beta.1, 1.0.0+build.1)'
        required: false
        type: string
      build:
        description: '...'  # ← 아래에서 전략에 맞는 description 선택
        type: boolean
        default: true
      alsetup_ref:
        description: 'alsetup-windows 브랜치/태그'
        required: false
        type: string
        default: master
      download_link_name:
        description: 'Teams 알림 다운로드 링크 이름 (선택)'
        required: false
        type: string
```

`build`의 `description`은 전략에 따라 선택:

- 레파지토리 커밋 방식: `'빌드 — 해제 시 레파지토리에 커밋된 파일로 패키징'`
- GitHub Release 방식: `'빌드 — 해제 시 GitHub Release의 바이너리로 패키징'`

`build`는 기본값이 `true`이며, 체크 해제 시 빌드 없이 기존 바이너리로 패키징만 수행합니다.
구현 방법은 아래 [build 옵션](#build-옵션-빌드-제어) 섹션을 참고하세요.

### build job — CMake 빌드

`shell: pwsh`를 사용합니다. `cmd`의 `^` 줄 연속은 YAML → 배치 파일 변환 과정에서
trailing whitespace로 인해 깨질 수 있어 PowerShell 백틱(`` ` ``)을 사용합니다.
vcvars 환경변수는 `cmd /c "vcvars && set"` 패턴으로 현재 PowerShell 세션에 전파합니다.

```yaml
- name: Build with CMake
  shell: pwsh
  run: |
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualCpp.Tools.Core.x86 -property installationPath
    $vcvars = Join-Path $vsPath "VC\Auxiliary\Build\vcvars64.bat"

    # vcvars 환경변수를 현재 PowerShell 세션에 전파
    cmd /c "`"$vcvars`" && set" | Where-Object { $_ -match "^[^=]+=.+" } | ForEach-Object {
      $name, $value = $_ -split "=", 2
      [System.Environment]::SetEnvironmentVariable($name, $value)
    }

    # Qt를 사용하는 프로젝트라면 -DCMAKE_PREFIX_PATH="<Qt 경로>" 추가
    cmake -S . -B build -G Ninja `
      -DCMAKE_BUILD_TYPE=Release
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    cmake --build build --config Release
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

### build job — Visual Studio 빌드 (.sln)

`devenv.com`을 직접 호출하여 솔루션을 빌드합니다. 프로젝트에 따라 MSVC 툴셋
버전(v140, v142 등)과 플랫폼(Win32, x64)이 다를 수 있으므로 사전에 확인합니다.

사용자가 vswhere 자동 탐색을 선택한 경우:

```yaml
- name: Build with Visual Studio
  shell: pwsh
  run: |
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $vsPath = & $vswhere -latest -products * -property installationPath
    $devenv = Join-Path $vsPath "Common7\IDE\devenv.com"
    & $devenv "<프로젝트>.sln" /build "Release|<플랫폼>"
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

사용자가 하드코딩 경로를 제공한 경우:

```yaml
- name: Build with Visual Studio
  shell: pwsh
  run: |
    $devenv = "<사용자가 제공한 devenv.com 경로>"
    & $devenv "<프로젝트>.sln" /build "Release|<플랫폼>"
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

PowerShell에서 외부 프로세스 호출 후 `$LASTEXITCODE`를 반드시 확인해야 합니다.
이를 생략하면 빌드 실패가 무시되어 CI가 성공으로 끝날 수 있습니다.

### Artifact 평탄화 패턴

`actions/upload-artifact@v4`는 `path`에 지정한 경로의 **디렉토리 구조를 보존**합니다.
예를 들어 `path: setup/ALCapture/bin/ALCapture.exe`로 업로드하면, 다운로드 시
`path: setup/ALCapture/bin`을 지정해도 `setup/ALCapture/bin/setup/ALCapture/bin/ALCapture.exe`처럼
경로가 중첩될 수 있습니다.

이를 방지하려면 **빌드 후 임시 디렉토리에 평탄화**하여 업로드합니다:

```yaml
# 빌드 결과물을 평탄한 임시 디렉토리에 수집
- name: Collect build artifacts
  shell: pwsh
  run: |
    New-Item -ItemType Directory -Force _artifacts/exe | Out-Null
    New-Item -ItemType Directory -Force _artifacts/symbols | Out-Null
    Copy-Item "<빌드출력경로>\<앱이름>.exe" "_artifacts\exe\" -Force
    # 심볼 경로는 .vcxproj의 <ProgramDatabaseFile>/<MapFileName>으로 확인
    # .exe와 같은 경로가 아닌 서브디렉토리(예: DebugInfo\)일 수 있음
    $symbols = Get-ChildItem "<심볼출력경로>\*.pdb", "<심볼출력경로>\*.map" -ErrorAction SilentlyContinue
    if (-not $symbols) { Write-Error "디버그 심볼(.pdb/.map)이 없습니다. 빌드 설정을 확인하세요."; exit 1 }
    $symbols | Copy-Item -Destination "_artifacts\symbols\" -Force

- name: Upload build artifacts
  uses: actions/upload-artifact@v4
  with:
    name: build-artifacts
    path: _artifacts/exe/

- name: Upload debug symbols
  uses: actions/upload-artifact@v4
  with:
    name: debug-symbols
    path: _artifacts/symbols/
    retention-days: 7 # upload-symbols job에서 SharePoint로 영구 보관하므로 단기 유지
```

이렇게 하면 packaging job에서 다운로드할 때 ISS의 `.\bin\` 참조 경로에
맞춰 바로 배치할 수 있습니다:

```yaml
- name: Download build artifacts
  uses: actions/download-artifact@v4
  with:
    name: build-artifacts
    path: <패키징디렉토리>/bin # .exe가 여기에 직접 풀림
```

빌드 출력 경로는 프로젝트마다 다릅니다. 1단계에서 반드시 확인하세요:

- **CMake**: `CMAKE_RUNTIME_OUTPUT_DIRECTORY` 설정값에 따라 `bin/`, `bin/Release/`,
  `build/Release/` 등 다양. 미설정 시 CMake 기본 규칙을 따름
- **.sln**: `.vcxproj`의 `<OutDir>` 설정에 따라 `bin/`, `Release/`, `bin/Release/` 등
  프로젝트별 상이

확인 방법 우선순위: (1) 빌드 설정 파일(`.vcxproj`의 `<OutDir>`, `CMakeLists.txt`의
`CMAKE_RUNTIME_OUTPUT_DIRECTORY`), (2) 기존 CI/빌드 스크립트의 출력 경로,
(3) `.gitignore` 패턴, (4) 프로젝트 문서(CLAUDE.md 등)

### packaging job 핵심 패턴

#### sparse-checkout 판단

packaging job은 빌드 결과물이 아닌 **패키징 리소스**(DLL, 이미지, 설정 파일 등)만
필요합니다. ISS 파일의 `Source:` 경로를 모두 분석하여 sparse-checkout 가능 여부를
판단합니다:

**sparse-checkout 가능** — 모든 `Source:` 참조가 하나의 디렉토리 트리 안에 있을 때:

```yaml
- name: Checkout repository
  uses: actions/checkout@v4
  with:
    sparse-checkout: <디렉토리> # 예: setup, dist
```

예시: ISS가 `setup/ALCapture/` 아래 있고, `.\bin\*` → `setup/ALCapture/bin/`,
`..\Common\*` → `setup/Common/` 처럼 모든 참조가 `setup/` 하위에서 해결될 때.

**전체 checkout 필요** — `Source:` 경로가 프로젝트 루트의 여러 경로를 참조할 때:

```yaml
- name: Checkout repository
  uses: actions/checkout@v4
```

#### packaging 단계

```yaml
- name: Download build artifacts
  uses: actions/download-artifact@v4
  with:
    name: build-artifacts
    path: <ISS의 .\bin\ 참조가 가리키는 실제 경로>

- name: ALTools Code Signing
  uses: team-util/altools-ci/actions/sign-files@v1
  with:
    iss-file: '<ISS 파일 경로>'

# 서명 완료된 개별 .exe를 별도 artifact로 보존 (package-artifacts에는 설치 파일만 포함)
- name: Upload signed binaries
  uses: actions/upload-artifact@v4
  with:
    name: signed-binaries
    path: |
      <ISS의 .\bin\ 경로>/<앱이름>.exe

- name: ALTools Public Packaging
  uses: team-util/altools-ci/actions/run-altools-packaging@v1
  with:
    iss_file: '<ISS 파일 경로>'
    token: ${{ secrets.ALTOOLS_UPLOAD_PRIVATE_REPOSITORY_ACCESS_PAT }}
    alsetup_ref: ${{ inputs.alsetup_ref || 'master' }}

- name: Upload package artifacts
  uses: actions/upload-artifact@v4
  with:
    name: package-artifacts
    path: <ISS의 OutputDir이 가리키는 실제 경로>
```

### build 옵션 (빌드 제어)

`build` 입력의 기본값은 `true`이며, 체크 해제 시 빌드 없이 기존 바이너리로
패키징만 실행합니다. `build=false` 시 바이너리 소스에 따라 두 가지 전략이 있습니다.

**핵심 원리**: `build` 잡이 `if: false`로 스킵되면, GitHub Actions는 그 잡의
결과를 `skipped`로 처리합니다. 이때 `needs`에 `build`가 있는 하위 잡도 기본적으로
스킵되므로, `packaging` 잡은 `if: always() && ...` 조건으로 스킵된 build를
허용해야 합니다.

#### build 잡 — 조건 추가

```yaml
build:
  needs: check
  if: ${{ inputs.build }}   # ← 추가
  runs-on: sign-worker
  # ... 나머지 동일 ...
```

#### packaging 잡 — 전략에 따라 다운로드 방식 분기

packaging 잡의 `if` 조건과 checkout은 두 전략 모두 동일합니다:

```yaml
packaging:
  needs: [check, build]
  # build가 스킵된 경우에도 실행되도록 always() 사용
  if: |
    always() &&
    needs.check.result == 'success' &&
    (needs.build.result == 'success' || needs.build.result == 'skipped')
  runs-on: sign-worker
  steps:
    - name: Checkout repository
      uses: actions/checkout@v4
      with:
        sparse-checkout: <디렉토리>
```

**전략 A: 레파지토리 커밋 방식** — `<패키징디렉토리>/bin/`에 유효한 바이너리가
git에 커밋되어 있어야 합니다. `build=false`이면 체크아웃된 파일을 그대로 사용합니다:

```yaml
    # build=false이면 레파지토리 파일을 그대로 사용하므로 스텝 건너뜀
    - name: Download build artifacts
      if: ${{ inputs.build }}
      uses: actions/download-artifact@v4
      with:
        name: build-artifacts
        path: <패키징디렉토리>/bin

    # 이하 코드서명, 패키징 스텝은 동일
```

**전략 B: GitHub Release 방식** — GitHub Release에 서명된 exe가 업로드되어
있어야 합니다 (release job이 담당). `build=false`이면 해당 버전의 Release에서
다운로드합니다:

```yaml
    # build=true: 빌드 artifact에서 다운로드
    - name: Download build artifacts
      if: ${{ inputs.build }}
      uses: actions/download-artifact@v4
      with:
        name: build-artifacts
        path: <패키징디렉토리>/bin

    # build=false: 해당 버전의 GitHub Release에서 서명된 exe 다운로드
    - name: Download exe from GitHub Release
      if: ${{ !inputs.build }}
      shell: pwsh
      env:
        GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
      run: |
        $tag = "v${{ needs.check.outputs.version }}"
        gh release download $tag --pattern "<앱이름>.exe" --dir "<패키징디렉토리>/bin" --clobber

    # 이하 코드서명, 패키징 스텝은 동일
```

> 태그를 명시적으로 지정해야 prerelease Release에서도 정확히 다운로드할 수 있습니다.
> 태그 미지정 시 `gh release download`는 latest(stable) release에서만 다운로드합니다.

#### upload-symbols 잡 — 조건 추가

심볼이 없으므로 통째로 스킵합니다.

```yaml
upload-symbols:
  needs: [check, build]
  if: ${{ inputs.build }}   # ← 추가
  runs-on: upload-worker
  # ... 나머지 동일 ...
```

> **주의**: `upload` 잡도 `always()` 패턴이 필요합니다. `packaging`이 `always()`로
> 실행되는 잡이기 때문에, GitHub Actions는 간접 선행 잡인 `build`의 `skipped` 상태를
> 하위 잡까지 전파합니다. `upload`에 명시적 `if` 조건이 없으면 기본 `success()`가
> false로 평가되어 `upload`가 실행되지 않습니다. 아래 upload job 섹션을 참고하세요.

### upload-symbols job

`build` 완료 직후 `packaging`과 병렬로 실행됩니다. `upload-worker`에서 rclone을 사용해 SharePoint에 영구 보관합니다.

디버그 심볼은 설치 파일과 **동일한 버전 경로 아래 `DebugInfo/` 폴더**에 업로드합니다. `upload.yml`의 경로 계산 로직(major.minor/patch 계층, prerelease/build 처리)을 동일하게 재현해야 합니다.

```yaml
upload-symbols:
  needs: [check, build]
  runs-on: upload-worker
  steps:
    - name: Download debug symbols
      uses: actions/download-artifact@v4
      with:
        name: debug-symbols
        path: symbols/

    - name: Compute upload path
      id: path
      shell: pwsh
      run: |
        $version = '${{ needs.check.outputs.version }}'
        if ($version -match '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([\w.-]+))?(?:\+([\w.-]+))?$') {
          $major = $matches[1]
          $minor = $matches[2]
          $patch = $matches[3]
          $prerelease = $matches[4]
          $build = $matches[5]
        } else {
          throw "Invalid version: $version"
        }

        function Sanitize([string]$name) {
          $invalid = [IO.Path]::GetInvalidFileNameChars() -join ''
          $pattern = "[{0}]" -f [Regex]::Escape($invalid)
          return ($name -replace $pattern, '_')
        }

        $base = Sanitize "$major.$minor"
        $patchFolder = if ($build) { Sanitize "$patch ($build)" } else { "$patch" }
        $basePath = Join-Path $base $patchFolder
        $versionPath = if ($prerelease) { Join-Path (Sanitize $prerelease) $basePath } else { $basePath }
        $uploadPath = Join-Path '<shared_point_root>' $versionPath

        "upload_path=$uploadPath" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append

    - name: Upload debug symbols to SharePoint
      uses: team-util/altools-ci/actions/shared-point-upload@v1
      with:
        path: symbols/
        upload_path: 'sp:${{ steps.path.outputs.upload_path }}/DebugInfo'
```

SharePoint 경로 예시 (버전 `1.1.1`): `ALPen/1.1/1/DebugInfo/`

### release job (GitHub Release 방식만)

GitHub Release 방식을 선택한 경우에만 추가합니다. packaging 완료 후 서명된 exe와
설치 파일을 GitHub Release에 첨부합니다. `GH_REPO` 환경변수로 리포를 지정하므로
checkout이 불필요합니다.

```yaml
release:
  needs: [check, packaging]
  if: |
    always() &&
    needs.check.result == 'success' &&
    needs.packaging.result == 'success'
  runs-on: sign-worker
  steps:
    - name: Download signed binaries
      uses: actions/download-artifact@v4
      with:
        name: signed-binaries
        path: _release/binaries/

    - name: Download package artifacts
      uses: actions/download-artifact@v4
      with:
        name: package-artifacts
        path: _release/package/

    - name: Create GitHub Release
      env:
        GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        GH_REPO: ${{ github.repository }}
      shell: pwsh
      run: |
        $version = "${{ needs.check.outputs.version }}"
        $tag = "v$version"

        # Release에 첨부할 파일 수집
        $files = @()
        $files += @(Get-ChildItem "_release\binaries\*.exe" -File | ForEach-Object { $_.FullName })
        $files += @(Get-ChildItem "_release\package\*.exe" -File | ForEach-Object { $_.FullName })

        if ($files.Count -eq 0) {
          Write-Error "첨부할 파일이 없습니다."
          exit 1
        }

        # 기존 Release가 있으면 에셋만 덮어쓰기
        gh release view $tag 2>$null
        if ($LASTEXITCODE -eq 0) {
          Write-Host "기존 Release($tag) — 에셋 덮어쓰기"
          gh release upload $tag $files --clobber
        } else {
          if ($version -match '-') {
            gh release create $tag $files --title $tag --notes "<제품명> $tag" --target "${{ github.sha }}" --prerelease
          } else {
            gh release create $tag $files --title $tag --notes "<제품명> $tag" --target "${{ github.sha }}"
          }
        }
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

**설계 결정:**

- **중복 Release 처리**: `gh release view`로 기존 Release 확인 후 `gh release upload --clobber`로 에셋 덮어쓰기
- **Prerelease 판별**: 시맨틱 버전에 `-`가 포함되면 `--prerelease` 플래그 추가 (SemVer 표준)
- **checkout 불필요**: `GH_REPO` 환경변수로 리포를 지정하므로 `actions/checkout` 생략 가능

### upload job

```yaml
upload:
  needs: [check, packaging]
  # packaging이 always()를 사용하므로, build가 skipped일 때 GitHub Actions가 skipped
  # 상태를 전파합니다. 명시적 always() 조건 없이는 upload가 실행되지 않습니다.
  if: |
    always() &&
    needs.check.result == 'success' &&
    needs.packaging.result == 'success'
  uses: team-util/altools-ci/.github/workflows/upload.yml@v1
  secrets: inherit
  with:
    product_name: <한국어 제품명> # Teams 알림 제목에 사용
    shared_point_root: <루트 경로> # 사용자가 알려준 SharePoint 루트
    version: ${{ needs.check.outputs.version }}
    package_artifacts_name: package-artifacts
    download_link_name: ${{ inputs.download_link_name }}
```

## 4단계: 필요한 GitHub Secrets 안내

워크플로우 생성 후 사용자에게 아래 시크릿 설정을 안내하세요:

| Secret                                         | 설명                            | 필수 여부         |
| ---------------------------------------------- | ------------------------------- | ----------------- |
| `ALTOOLS_UPLOAD_PRIVATE_REPOSITORY_ACCESS_PAT` | alsetup-windows 체크아웃용 PAT  | 필수              |
| `ALTOOLS_UPLOAD_RCLONE_CONFIG`                 | SharePoint 업로드용 rclone 설정 | 필수              |
| `ALTOOLS_UPLOAD_WEBHOOK_URL`                   | Teams 알림 웹훅 URL             | 필수              |
| `JIRA_EMAIL`                                   | Jira 이메일 (연동 시)           | Jira 연동 시 필수 |
| `JIRA_API_TOKEN`                               | Jira API 토큰 (연동 시)         | Jira 연동 시 필수 |

## 자주 확인해야 할 사항

- **ISS 파일 내 `OutputDir`** — packaging artifact path와 일치해야 함 (예: `.\output` → `<패키징디렉토리>/output`)
- **ISS 파일 내 `.\bin\` 참조** — download artifact의 `path`가 ISS 기준 상대경로와 맞아야 함
- **패키징 디렉토리의 git 추적 여부** — `git ls-files <패키징 디렉토리>/`로 확인; `.exe`, `.dll`이 gitignore여도 force-add된 경우가 있음
- **빌드 출력 경로** — 프로젝트마다 다름. `.vcxproj`의 `<OutDir>`, `CMakeLists.txt`의 `CMAKE_RUNTIME_OUTPUT_DIRECTORY`, 기존 CI 스크립트, `.gitignore` 패턴 등에서 추적
- **ISS 파일 수정 필요 시 사용자 확인** — ISS 파일은 패키징의 핵심 스크립트이므로, 수정이 필요한 경우 반드시 사용자에게 변경 내용을 설명하고 동의를 구한 뒤 수정할 것
- **코드 서명이 안 될 때** — ISS `[Files]` 섹션에서 서명할 파일의 `Flags:`에 `signonce` 추가 필요. 또한 `Source:` 경로에 Inno Setup 전처리기 매크로(`{#...}`)가 있으면 `sign-files.ps1`이 파일을 resolve하지 못하므로 리터럴 경로로 써야 함

  ```ini
  ; 잘못된 예 (서명 안 됨)
  Source: ".\bin\{#MyAppExeName}"; Flags: ignoreversion

  ; 올바른 예
  Source: ".\bin\ALPen.exe"; Flags: ignoreversion signonce
  ```

## altools-ci 참고 경로

액션 및 재사용 워크플로우 레퍼런스:

- `team-util/altools-ci/actions/sign-files@v1`
- `team-util/altools-ci/actions/run-altools-packaging@v1`
- `team-util/altools-ci/actions/shared-point-upload@v1`
- `team-util/altools-ci/actions/notify-to-teams@v1`
- `team-util/altools-ci/.github/workflows/check.yml@v1`
- `team-util/altools-ci/.github/workflows/upload.yml@v1`
