---
name: altools-ci-run
description: >
  altools-ci GitHub Actions 워크플로우를 실행하고
  모니터링한다. 빌드 → 서명 → 패키징 → GitHub Release → SharePoint
  업로드까지 전체 CI 파이프라인을 트리거하고 결과를 보고한다.
  사용자가 "altools"를 명시적으로 언급한 경우에만 트리거한다:
  "altools CI", "altools 패키징", "altools 셋업", "altools 빌드",
  "altools 파이프라인", "altools 릴리스", "altools-ci 실행".
argument-hint: '[version]'
---

# altools-ci-run

altools-ci 워크플로우를 실행하고 모니터링하는 스킬.

## 사전 검사

### 워크플로우 파일 확인

워크플로우 파일이 현재 브랜치에 존재하는지 확인한다. 없으면 사용자에게 알리고 중단한다.

```bash
git show HEAD:.github/workflows/altools-ci.yml > /dev/null 2>&1
```

실패 시: "현재 브랜치에 `.github/workflows/altools-ci.yml`이 없습니다." 안내 후 중단.

### 리모트 push 확인

CI는 리모트 코드 기준으로 실행되므로, 로컬 최신 커밋이 push되었는지 확인한다.

```bash
git fetch origin
LOCAL=$(git rev-parse HEAD)
REMOTE=$(git rev-parse @{u} 2>/dev/null)
```

- **리모트 tracking 브랜치가 없거나 LOCAL ≠ REMOTE**: push되지 않은 커밋이 있음을 알리고, push 여부를 묻는다. 사용자가 push를 원하면 `git push origin <branch>` 실행 후 진행한다. push하지 않으면 리모트에 push된 커밋 기준으로 그대로 진행한다.
- **LOCAL = REMOTE**: 다음 단계로 진행한다.

## 워크플로우 옵션

| 옵션 | gh 플래그 | 기본값 | 설명 |
|------|-----------|--------|------|
| 브랜치 | `--ref` | 현재 브랜치 | 실행 대상 브랜치 |
| version | `-f version=` | (인자 또는 HEAD 커밋에서 추출) | SemVer 버전 |
| build | `-f build=` | `true` | true: 소스 빌드, false: GitHub Release 바이너리 사용 |
| alsetup_ref | `-f alsetup_ref=` | `master` | alsetup-windows 브랜치/태그 |
| download_link_name | `-f download_link_name=` | (없음) | Teams 알림 다운로드 링크 이름 |

## 실행 절차

### 1. 기본값 수집

```bash
# 현재 브랜치
git branch --show-current

# HEAD 커밋 메시지 (버전 추출용)
git log --pretty=format:'%s' -1 HEAD
```

인자로 version이 제공되면 그대로 사용한다. 없으면 HEAD 커밋 메시지에서 SemVer 패턴 추출을 시도한다.

**SemVer 검증**: 아래 정규식에 맞아야 check job이 통과한다. 맞지 않으면 사용자에게 입력을 요청한다.
```
^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([\w.-]+))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$
```

**4자리 버전 → SemVer 변환**: 커밋 메시지에서 4자리 버전(예: `3.25.2.34`)을 추출하면 아래 선택지를 제시한다:
```
버전 3.25.2.34 → SemVer 선택:
  1. 3.25.2+34  (release)
  2. 3.25.2-34  (pre-release)
  3. 3.25.2     (빌드 번호 제외)
```

### 2. 옵션 확인

사용자에게 옵션 표를 제시하고 확인받는다. 기본값이 아닌 옵션만 변경하면 된다.

```
다음 설정으로 altools-ci를 실행합니다:

| 옵션 | 값 |
|------|-----|
| 브랜치 | <현재 브랜치> |
| version | <추출된 버전> |
| build | true |
| alsetup_ref | master |
| download_link_name | (없음) |

변경할 옵션이 있으면 말씀해주세요.
```

### 3. build=false 사전 체크

`build=false`일 때는 해당 버전의 GitHub Release에서 exe를 다운로드한다. Release가 존재하는지 먼저 확인한다:

```bash
gh release view "v${version}" --json tagName 2>/dev/null
```

존재하지 않으면 경고하고 진행 여부를 확인한다.

### 4. 워크플로우 실행

```bash
gh workflow run altools-ci.yml --ref <branch> \
  -f version=<version> \
  -f build=<true|false> \
  -f alsetup_ref=<ref> \
  -f download_link_name=<name>
```

기본값인 옵션은 `-f` 플래그를 생략해도 된다. 단, `version`은 자동 추출이 불안정하므로 항상 명시한다.

실행 후 3초 대기 후 Run ID 확인:

```bash
sleep 3 && gh run list --workflow=altools-ci.yml --limit 1
```

### 5. 모니터링

`gh run watch`를 백그라운드로 실행한다:

```bash
gh run watch <run-id>
# timeout: 600000 (10분)
# run_in_background: true
```

사용자에게 알린다:
```
워크플로우 실행 중입니다. (Run ID: XXXXXXX)
완료되면 알려드리겠습니다.
```

### 6. 결과 보고

완료 알림을 받으면:

```bash
gh run view <run-id>
```

**성공 시** — job별 소요 시간 표로 보고한다.

**실패 시** — 실패한 job의 로그를 자동 확인한다:

```bash
# 방법 1: --log-failed
gh run view <run-id> --log-failed

# 방법 2: 출력이 없으면 API로 직접 확인
# 먼저 실패한 job ID를 찾는다
gh api repos/{owner}/{repo}/actions/runs/<run-id>/jobs --jq '.jobs[] | select(.conclusion=="failure") | .id'

# 해당 job 로그 확인
gh api repos/{owner}/{repo}/actions/jobs/<job-id>/logs 2>&1 | tail -40
```

실패 원인을 분석하여 보고하고, 해결 방안을 제시한다.
