---
name: euckr-to-utf8bom
description: >
  Converts C/C++ source files from EUC-KR (CP949) to UTF-8 with BOM, or adds
  BOM to BOM-less UTF-8 files. Use this skill whenever the user needs file
  encoding conversion for MSVC/Visual Studio projects — including EUC-KR → UTF-8,
  CP949 → UTF-8 BOM, adding BOM to existing UTF-8 files, or scanning for
  encoding issues. Trigger on: garbled Korean text in source files (한글 깨짐,
  ???, □, mojibake), MSVC warning C4819, CI builds breaking on Korean string
  literals, #pragma execution_character_set issues, "인코딩 변환", "UTF-8 BOM으로
  바꿔줘", "BOM 추가", encoding normalization across a C++ project, or legacy
  project encoding cleanup. Also trigger for dry-run encoding scans. Do NOT
  trigger for: runtime encoding conversion in Python/Java code, Base64/URL
  encoding, HTML charset meta tags, git terminal display issues, or database
  encoding problems.
---

# EUC-KR → UTF-8 BOM 인코딩 변환

MSVC(Visual Studio)는 BOM 없는 파일을 시스템 기본 코드페이지(한국 Windows = CP949/EUC-KR)로 해석합니다.
UTF-8 BOM(`EF BB BF`)을 붙이면 MSVC가 파일을 UTF-8로 강제 인식하므로,
한국어 주석·문자열이 있는 C++ 프로젝트에서는 UTF-8 BOM이 권장 형식입니다.

## 스크립트 위치

변환 스크립트는 이 스킬 디렉터리의 `scripts/convert_encoding.py`에 있습니다.
실행할 때는 항상 **스킬의 base directory 기준 절대 경로**를 사용하세요.

```bash
# 예시 (실제 경로는 스킬 로드 시 제공되는 base directory 사용)
python "<스킬-base-directory>/scripts/convert_encoding.py" <path> [options]
```

## 변환 전략 선택

사용자의 요청에 따라 두 가지 모드를 구분합니다.

| 상황 | 모드 |
|---|---|
| 특정 파일 목록을 지정한 경우 | **파일 지정 모드** |
| 디렉터리/프로젝트 전체를 변환하는 경우 | **프로젝트 스캔 모드** |

모드가 명확하지 않으면 사용자에게 확인하세요.

---

## 변환 절차

### 1단계 – 변환 대상 파악 (dry-run)

두 모드 모두 `--dry-run`으로 먼저 실행해 변환 대상을 확인합니다.

**프로젝트 스캔 모드:**

```bash
python "<스킬-base-directory>/scripts/convert_encoding.py" <디렉터리> --dry-run --ext cpp h
```

**파일 지정 모드:** 단일 파일도 확장자 필터가 적용됩니다.
`--ext`에 포함된 확장자만 처리되므로, 비표준 확장자 파일은 `--ext`에 추가하세요.

```bash
python "<스킬-base-directory>/scripts/convert_encoding.py" <파일경로> --dry-run --ext cpp h hpp cc
```

출력된 목록을 사용자에게 보여주고, 의도치 않은 파일이 포함되지 않았는지 확인합니다.
확인이 끝나면 2단계로 진행합니다.

### 2단계 – 변환 실행

`--dry-run` 없이 실행합니다.
스크립트는 원자적 쓰기(임시 파일에 먼저 쓰고 `os.replace`로 교체)를 사용하므로,
쓰기 도중 실패해도 원본 파일이 손상되지 않습니다.
기본적으로 `.bak` 백업을 자동 생성합니다.

```bash
python "<스킬-base-directory>/scripts/convert_encoding.py" <디렉터리> --ext cpp h
```

### 3단계 – 검증

스크립트가 실행 후 요약 통계를 출력합니다. 다음을 확인하세요:

1. **한글 보존 확인** – 결과 요약의 "한글 문자" 행에서 변환 전·후 수가 동일(✓)인지 확인합니다.
2. **빌드 확인** – 가능하면 MSBuild로 빌드해 컴파일 에러가 없는지 확인합니다.

### 4단계 – 백업 정리

검증이 완료되면 `.bak` 파일을 삭제합니다. 문제가 발생하면 `.bak`으로 원상 복구합니다.

```bash
# 백업 삭제 (Git Bash / Unix)
find <디렉터리> -name "*.bak" -delete

# 백업 삭제 (PowerShell)
Get-ChildItem -Path <디렉터리> -Recurse -Filter "*.bak" | Remove-Item

# 백업으로 복구 (Git Bash / Unix)
find <디렉터리> -name "*.bak" | while read f; do mv "$f" "${f%.bak}"; done

# 백업으로 복구 (PowerShell)
Get-ChildItem -Path <디렉터리> -Recurse -Filter "*.bak" | ForEach-Object {
  Move-Item $_.FullName ($_.FullName -replace '\.bak$','')
}
```

---

## 스크립트 옵션

```
python convert_encoding.py <path> [--ext EXT [EXT ...]] [--dry-run] [--no-backup]
```

| 옵션 | 기본값 | 설명 |
|---|---|---|
| `path` | (필수) | 파일 또는 디렉터리 경로 |
| `--ext` | `cpp h` | 처리할 확장자 목록 (단일 파일에도 적용) |
| `--dry-run` | off | 변환 없이 감지 결과만 출력 |
| `--no-backup` | off | `.bak` 백업 파일 생성 안 함 |

---

## 감지 로직

스크립트는 다음 순서로 인코딩을 판별합니다.

1. **UTF-8 BOM 확인** (`EF BB BF`) → 이미 변환됨, 건너뜀
2. **UTF-16 BOM 확인** (`FF FE` / `FE FF`) → 경고 출력 후 건너뜀
3. **UTF-8 유효성 검사** → 유효한 UTF-8이면 BOM만 추가
4. **EUC-KR 디코드 시도** → 성공하면 EUC-KR로 판정하여 변환
5. **판정 불가** → 경고 출력 후 건너뜀 (바이너리 파일 등)

### 감지 한계

- **순수 ASCII 파일**: ASCII는 UTF-8의 부분집합이므로 3단계에서 UTF-8로 판정되어 BOM만 추가됩니다. 이 동작은 정상이며, ASCII 파일에 BOM을 추가해도 내용에 영향이 없습니다.
- **EUC-KR 오탐 가능성**: 매우 드물지만, EUC-KR+한글 파일의 바이트가 우연히 유효한 UTF-8 시퀀스를 형성하면 UTF-8로 오판정될 수 있습니다. 한글이 많이 포함된 대용량 파일에서는 실질적으로 발생하지 않습니다.
- **`.bak` 파일**: 스크립트가 자동으로 `.bak` 확장자 파일을 수집 대상에서 제외하므로, 재실행 시 백업 파일이 처리되지 않습니다.

---

## 주의 사항

- `.vcxproj`, `.sln`, XML 계열 파일은 BOM이 있으면 파서가 오동작할 수 있으므로 기본 대상에서 제외됩니다. `--ext`에 추가하지 마세요.
- 변환 후 `git diff`가 크게 나타나는 것은 정상입니다 (인코딩 바이트 변경).
- MSVC 이외 컴파일러(GCC, Clang)에서는 `-finput-charset=utf-8` 플래그가 필요할 수 있습니다.
