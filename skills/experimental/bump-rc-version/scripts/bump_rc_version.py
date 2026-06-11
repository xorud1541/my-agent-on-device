#!/usr/bin/env python3
"""
Windows RC 파일 버전 번호 업데이트 스크립트

Usage:
    python bump_rc_version.py <files...> [--product-version X.X.X.X] [--file-version X.X.X.X] [--dry-run]
"""
import argparse
import io
import os
import re
import sys
from datetime import date
from pathlib import Path

# --- Encoding detection & preservation ---

_UTF16_LE_BOM = b"\xff\xfe"
_UTF8_BOM = b"\xef\xbb\xbf"

# Internal encoding identifiers
_ENC_UTF16LE = "utf-16-le-bom"
_ENC_UTF8SIG = "utf-8-sig"
_ENC_UTF8 = "utf-8"
_ENC_CP949 = "cp949"

# Regex patterns for 4 version locations
_RE_FV_COMMA = re.compile(r"(FILEVERSION\s+)(\d+\s*,\s*\d+\s*,\s*\d+\s*,\s*\d+)")
_RE_PV_COMMA = re.compile(r"(PRODUCTVERSION\s+)(\d+\s*,\s*\d+\s*,\s*\d+\s*,\s*\d+)")
_RE_FV_DOT = re.compile(r'(VALUE\s+"FileVersion"\s*,\s*")(\d+\.\d+\.\d+\.\d+)')
_RE_PV_DOT = re.compile(r'(VALUE\s+"ProductVersion"\s*,\s*")(\d+\.\d+\.\d+\.\d+)')


def _detect_encoding(raw: bytes) -> str | None:
    """Detect file encoding from BOM bytes or content heuristics."""
    if raw[:2] == _UTF16_LE_BOM:
        return _ENC_UTF16LE
    if raw[:3] == _UTF8_BOM:
        return _ENC_UTF8SIG
    try:
        raw.decode("utf-8")
        return _ENC_UTF8
    except UnicodeDecodeError:
        pass
    try:
        raw.decode("cp949")
        return _ENC_CP949
    except UnicodeDecodeError:
        return None


def _read_file(path: Path) -> tuple[str | None, str | None]:
    """Read RC file with auto-detected encoding.

    Returns:
        (text_content, encoding_id) or (None, None) on failure.
    """
    raw = path.read_bytes()
    enc = _detect_encoding(raw)
    if enc is None:
        return None, None
    if enc == _ENC_UTF16LE:
        return raw[2:].decode("utf-16-le"), enc
    if enc == _ENC_UTF8SIG:
        return raw[3:].decode("utf-8"), enc
    if enc == _ENC_UTF8:
        return raw.decode("utf-8"), enc
    # cp949
    return raw.decode("cp949"), enc


def _write_file(path: Path, text: str, encoding: str) -> None:
    """Atomic write preserving original encoding (tmp → os.replace)."""
    if encoding == _ENC_UTF16LE:
        data = _UTF16_LE_BOM + text.encode("utf-16-le")
    elif encoding == _ENC_UTF8SIG:
        data = _UTF8_BOM + text.encode("utf-8")
    elif encoding == _ENC_UTF8:
        data = text.encode("utf-8")
    elif encoding == _ENC_CP949:
        data = text.encode("cp949")
    else:
        raise ValueError(f"Unknown encoding: {encoding}")
    tmp = path.with_suffix(path.suffix + ".tmp")
    try:
        tmp.write_bytes(data)
        os.replace(tmp, path)
    except Exception:
        tmp.unlink(missing_ok=True)
        raise


def _parse_versions(text: str) -> tuple[str, str] | None:
    """Extract current ProductVersion and FileVersion (dot-separated) from RC content.

    Returns:
        (product_version, file_version) or None if either is missing.
    """
    pv = _RE_PV_DOT.search(text)
    fv = _RE_FV_DOT.search(text)
    if not pv or not fv:
        return None
    return pv.group(2), fv.group(2)


def _verify_versions(text: str, new_pv: str, new_fv: str) -> bool:
    """Verify all 4 version locations in RC content match the expected versions.

    Checks both comma-format (FILEVERSION / PRODUCTVERSION) and
    dot-format (VALUE "FileVersion" / VALUE "ProductVersion") locations.
    Comma-format values may have spaces around commas, so we normalize before
    comparing to avoid false negatives from whitespace differences.
    """
    # Verify dot-format locations
    pv_dot = _RE_PV_DOT.search(text)
    fv_dot = _RE_FV_DOT.search(text)
    if not pv_dot or pv_dot.group(2) != new_pv:
        return False
    if not fv_dot or fv_dot.group(2) != new_fv:
        return False

    # Verify comma-format locations; normalize spaces around commas before comparing
    pv_comma = _RE_PV_COMMA.search(text)
    fv_comma = _RE_FV_COMMA.search(text)
    if not pv_comma or not fv_comma:
        return False
    # re.sub strips optional spaces so "1 , 2 , 3 , 4" → "1,2,3,4"
    norm_pv = re.sub(r"\s*,\s*", ",", pv_comma.group(2))
    norm_fv = re.sub(r"\s*,\s*", ",", fv_comma.group(2))
    if norm_pv != new_pv.replace(".", ","):
        return False
    if norm_fv != new_fv.replace(".", ","):
        return False

    return True


def _compute_new_versions(
    current_pv: str,
    current_fv: str,
    arg_pv: str | None,
    arg_fv: str | None,
) -> tuple[str, str]:
    """Compute new ProductVersion and FileVersion.

    Rules:
    - No args: build number +1, FileVersion date = today
    - --product-version only: use given, FileVersion = today + same build number
    - Both given: use as-is
    """
    if arg_pv and arg_fv:
        return arg_pv, arg_fv

    pv_parts = current_pv.split(".")
    old_build = int(pv_parts[3])

    if arg_pv:
        new_pv_parts = arg_pv.split(".")
        build = new_pv_parts[3]
        today = date.today()
        new_fv = f"{today.year % 100}.{today.month}.{today.day}.{build}"
        return arg_pv, new_fv

    # No args: bump build number
    new_build = old_build + 1
    new_pv = f"{pv_parts[0]}.{pv_parts[1]}.{pv_parts[2]}.{new_build}"
    today = date.today()
    new_fv = f"{today.year % 100}.{today.month}.{today.day}.{new_build}"
    return new_pv, new_fv


def _replace_versions(text: str, new_pv: str, new_fv: str) -> tuple[str, int]:
    """Replace all 4 version locations in RC content.

    Returns:
        (modified_text, total_replacements)
    """
    new_pv_comma = new_pv.replace(".", ",")
    new_fv_comma = new_fv.replace(".", ",")

    total = 0
    text, n = _RE_FV_COMMA.subn(rf"\g<1>{new_fv_comma}", text)
    total += n
    text, n = _RE_PV_COMMA.subn(rf"\g<1>{new_pv_comma}", text)
    total += n
    text, n = _RE_FV_DOT.subn(rf"\g<1>{new_fv}", text)
    total += n
    text, n = _RE_PV_DOT.subn(rf"\g<1>{new_pv}", text)
    total += n
    return text, total


def _error(msg: str) -> None:
    print(f"[error] {msg}", file=sys.stderr)


def _validate_version_format(version: str, label: str) -> None:
    """Validate that version string matches X.X.X.X format (4 integer parts)."""
    parts = version.split(".")
    if len(parts) != 4 or not all(p.isdigit() for p in parts):
        _error(f"잘못된 {label} 형식: {version} (expected: X.X.X.X)")
        sys.exit(1)


def main() -> None:
    if sys.platform == "win32":
        sys.stdout = io.TextIOWrapper(
            sys.stdout.buffer, encoding="utf-8", errors="replace"
        )
        sys.stderr = io.TextIOWrapper(
            sys.stderr.buffer, encoding="utf-8", errors="replace"
        )

    parser = argparse.ArgumentParser(description="Windows RC 파일 버전 번호 업데이트")
    parser.add_argument("files", nargs="+", help="RC 파일 경로 (하나 이상)")
    parser.add_argument("--product-version", dest="product_version", help="새 ProductVersion (A.B.C.XX)")
    parser.add_argument("--file-version", dest="file_version", help="새 FileVersion (YY.M.D.XX)")
    parser.add_argument("--dry-run", action="store_true", help="변경 없이 미리보기만 출력")
    args = parser.parse_args()

    if args.file_version and not args.product_version:
        _error("--file-version은 --product-version과 함께 사용해야 합니다.")
        sys.exit(1)

    if args.product_version:
        _validate_version_format(args.product_version, "ProductVersion")
    if args.file_version:
        _validate_version_format(args.file_version, "FileVersion")

    # Filter: keep only files that contain FILEVERSION
    rc_files: list[Path] = []
    for f in args.files:
        p = Path(f)
        if not p.is_file():
            _error(f"{p}: 파일을 찾을 수 없습니다.")
            continue
        text, enc = _read_file(p)
        if text is None:
            _error(f"{p}: 인코딩을 감지할 수 없습니다.")
            continue
        if "FILEVERSION" not in text:
            _error(f"{p}: FILEVERSION을 찾을 수 없습니다.")
            continue
        rc_files.append(p)

    if not rc_files:
        _error("처리할 RC 파일이 없습니다.")
        sys.exit(1)

    # Parse current versions from the first file, verify all files match
    first_text, _ = _read_file(rc_files[0])
    versions = _parse_versions(first_text)
    if versions is None:
        _error(f"{rc_files[0]}: 버전 정보를 파싱할 수 없습니다.")
        sys.exit(1)
    current_pv, current_fv = versions

    for p in rc_files[1:]:
        t, _ = _read_file(p)
        v = _parse_versions(t)
        if v is None:
            _error(f"{p}: 버전 정보를 파싱할 수 없습니다.")
            sys.exit(1)
        if v != (current_pv, current_fv):
            _error(
                f"버전 불일치: {rc_files[0].name}={current_pv}/{current_fv}, "
                f"{p.name}={v[0]}/{v[1]}"
            )
            sys.exit(1)

    # Compute new versions
    new_pv, new_fv = _compute_new_versions(
        current_pv, current_fv, args.product_version, args.file_version
    )

    # 빌드 번호(4번째 자리)가 ProductVersion과 FileVersion 간 일치해야 함
    new_pv_build = new_pv.split(".")[-1]
    new_fv_build = new_fv.split(".")[-1]
    if new_pv_build != new_fv_build:
        _error(
            f"빌드 번호 불일치: ProductVersion={new_pv} (build {new_pv_build}), "
            f"FileVersion={new_fv} (build {new_fv_build})"
        )
        sys.exit(1)

    if new_pv == current_pv and new_fv == current_fv:
        print("[info] 변경 사항이 없습니다.")
        sys.exit(0)

    # Output
    file_names = ", ".join(p.name for p in rc_files)
    print(f"[files] {file_names}")
    print(f"[before] ProductVersion: {current_pv}, FileVersion: {current_fv}")
    print(f"[after]  ProductVersion: {new_pv}, FileVersion: {new_fv}")

    if args.dry_run:
        sys.exit(0)

    # Apply changes
    for p in rc_files:
        text, enc = _read_file(p)
        modified, count = _replace_versions(text, new_pv, new_fv)
        if count < 4:
            _error(f"{p.name}: {count}/4곳만 치환됨 (expected >= 4)")
            sys.exit(1)
        _write_file(p, modified, enc)

    # Verify all 4 locations (2 comma-format + 2 dot-format)
    all_ok = True
    for p in rc_files:
        text, _ = _read_file(p)
        if not _verify_versions(text, new_pv, new_fv):
            _error(f"{p.name}: 검증 실패 — 기대 PV={new_pv} FV={new_fv}")
            all_ok = False

    if all_ok:
        print(f"[verified] 모든 파일의 4곳이 정상 업데이트됨 ✓")
    else:
        sys.exit(1)


if __name__ == "__main__":
    main()
