---
name: bump-rc-version
description: >
  Windows RC 파일의 FILEVERSION·PRODUCTVERSION을 업데이트하고 커밋한다.
  "버전 올려줘", "버전 bump", "버전 업", "빌드 번호 올려줘",
  "version bump", "버전 변경" 등의 요청 시 트리거한다.
argument-hint: '[ProductVersion] [FileVersion]'
---

# bump-rc-version

Windows RC 파일의 버전 번호를 올리고 커밋한다.

## 스크립트 위치

스크립트는 이 스킬 디렉터리의 `scripts/bump_rc_version.py`에 있다.
실행할 때는 항상 **스킬의 base directory 기준 절대 경로**를 사용한다.

```bash
python "<스킬-base-directory>/scripts/bump_rc_version.py" <files...> [options]
```

## 사전 검사

프로젝트에서 FILEVERSION이 포함된 `.rc` 파일을 찾는다:

```bash
grep -rl "FILEVERSION" --include="*.rc" .
```

**결과가 없으면 이 스킬을 중단한다.** 사용자에게 알리지 않고 조용히 취소하여
다른 버전 관리 방식(CMakeLists.txt, package.json 등)이 처리할 수 있도록 한다.

## 버전 체계

RC 파일에는 두 종류의 버전이 있다.

| RC 필드 | 형식 | 예시 | 설명 |
|---|---|---|---|
| `FILEVERSION` / `"FileVersion"` | `YY,M,D,XX` / `"YY.M.D.XX"` | `26,4,1,33` | 날짜+빌드 기반 |
| `PRODUCTVERSION` / `"ProductVersion"` | `A,B,C,XX` / `"A.B.C.XX"` | `3,25,2,33` | 제품 버전 |

두 버전의 마지막 자리(빌드 번호)는 항상 동일하게 유지한다.

## 인자 처리

| 사용자 인자 | 스크립트 옵션 | 동작 |
|-------------|-------------|------|
| 없음 | (옵션 없이 실행) | 빌드 번호 +1, FileVersion 날짜를 오늘로 |
| `3.25.2.34` | `--product-version 3.25.2.34` | ProductVersion 지정, FileVersion = 오늘 날짜 + 동일 빌드 번호 |
| `3.25.2.34 26.4.2.34` | `--product-version 3.25.2.34 --file-version 26.4.2.34` | 둘 다 지정 |

## 실행 절차

### 1. dry-run으로 미리보기

```bash
SCRIPT_DIR="$(ls -d ~/.claude/skills/bump-rc-version/scripts .claude/skills/bump-rc-version/scripts 2>/dev/null | head -1)"
python "$SCRIPT_DIR/bump_rc_version.py" <RC파일들> --dry-run [--product-version X.X.X.X] [--file-version X.X.X.X]
```

스크립트 출력을 표로 정리하여 사용자에게 확인받는다:

```
| 항목 | 이전 | 이후 |
|------|------|------|
| ProductVersion | 3.25.2.33 | 3.25.2.34 |
| FileVersion    | 26.4.1.33 | 26.4.3.34 |
| 대상 파일 | ALCapture.rc, ALCaptureEditor.rc |
```

### 2. 실행

사용자가 승인하면 `--dry-run`을 빼고 동일한 명령을 실행한다.

스크립트가 자동으로 인코딩을 감지하고, 수정 후 동일 인코딩으로 저장하며, 4곳 모두 업데이트되었는지 검증한다.

### 3. 커밋

이전 버전 변경 커밋을 찾아 prefix 여부를 판단한다:

```bash
git log --oneline -20 | grep -E '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+'
```

- 이전 버전 커밋에 prefix가 있으면 동일한 prefix 사용 (예: `build: 1.1.0.21 (26.2.27.21)`)
- prefix가 없으면 prefix 없이 커밋 (예: `3.25.2.34 (26.4.2.34)`)

변경된 RC 파일만 staging하여 커밋한다:

```bash
git add <RC파일1> <RC파일2> ...
git commit -m "$(cat <<'EOF'
[prefix] <ProductVersion> (<FileVersion>)
EOF
)"
```

## 주의사항

- exe 바이너리는 커밋하지 않는다.
- 빌드 번호는 PRODUCTVERSION과 FILEVERSION 양쪽에서 항상 동일하게 유지한다.
- FILEVERSION의 날짜 부분은 항상 **오늘 날짜** 기준으로 설정한다.
