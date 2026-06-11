#!/usr/bin/env python3
"""
EUC-KR → UTF-8 BOM 인코딩 변환 스크립트

사용법:
    python convert_encoding.py <path> [--ext cpp h] [--dry-run] [--no-backup]
"""
import argparse
import io
import os
import shutil
import sys
from pathlib import Path
from typing import Literal, TypedDict

UTF8_BOM = b"\xef\xbb\xbf"
UTF16_LE_BOM = b"\xff\xfe"
UTF16_BE_BOM = b"\xfe\xff"

Status = Literal["skipped", "converted", "bom_added", "error", "dry_run"]
Action = Literal["convert", "bom_add", "none"]


class ConvertResult(TypedDict, total=False):
    status: Status
    reason: str
    action: Action
    korean_before: int
    korean_after: int


def detect_and_convert(
    file_path: Path, dry_run: bool, no_backup: bool
) -> ConvertResult:
    """
    파일 인코딩을 감지하고 필요 시 EUC-KR → UTF-8 BOM으로 변환합니다.

    Returns:
        ConvertResult with keys: status, reason, action, korean_before, korean_after
    """
    try:
        raw = file_path.read_bytes()
    except OSError as e:
        return {"status": "error", "reason": str(e), "action": "none"}

    # 1. 이미 UTF-8 BOM
    if raw[:3] == UTF8_BOM:
        return {"status": "skipped", "reason": "already UTF-8 BOM", "action": "none"}

    # 2. UTF-16 BOM (처리 대상 아님)
    if raw[:2] in (UTF16_LE_BOM, UTF16_BE_BOM):
        return {
            "status": "skipped",
            "reason": "UTF-16 file – skipped",
            "action": "none",
        }

    # 3. UTF-8 유효성 검사
    try:
        text = raw.decode("utf-8")
        encoding = "utf-8"
    except UnicodeDecodeError:
        # 4. EUC-KR 디코드 시도
        try:
            text = raw.decode("euc-kr")
            encoding = "euc-kr"
        except UnicodeDecodeError:
            return {
                "status": "skipped",
                "reason": "cannot decode as UTF-8 or EUC-KR",
                "action": "none",
            }

    korean_count = sum(1 for ch in text if "\uac00" <= ch <= "\ud7a3")
    action: Action = "convert" if encoding == "euc-kr" else "bom_add"

    if dry_run:
        label = "would convert" if action == "convert" else "would add BOM"
        return {
            "status": "dry_run",
            "reason": f"{encoding} → UTF-8 BOM ({label})",
            "action": action,
            "korean_before": korean_count,
            "korean_after": korean_count,
        }

    # 변환 실행 (원자적 쓰기: 임시 파일에 먼저 쓰고 성공 시에만 교체)
    new_bytes = UTF8_BOM + text.encode("utf-8")
    tmp_path = file_path.with_suffix(file_path.suffix + ".tmp")
    try:
        tmp_path.write_bytes(new_bytes)
        if not no_backup:
            shutil.copy2(file_path, file_path.with_suffix(file_path.suffix + ".bak"))
        os.replace(tmp_path, file_path)
    except Exception:
        tmp_path.unlink(missing_ok=True)
        raise

    status: Status = "converted" if action == "convert" else "bom_added"
    return {
        "status": status,
        "reason": f"{encoding} → UTF-8 BOM",
        "action": action,
        "korean_before": korean_count,
        "korean_after": korean_count,
    }


def collect_files(path: Path, extensions: list[str]) -> list[Path]:
    """디렉터리에서 지정 확장자 파일을 재귀적으로 수집합니다."""
    exts = {e.lstrip(".").lower() for e in extensions}
    if path.is_file():
        if path.suffix.lstrip(".").lower() in exts:
            return [path]
        return []
    return sorted(
        p
        for p in path.rglob("*")
        if p.suffix.lstrip(".").lower() in exts and ".bak" not in p.suffixes
    )


def main() -> None:
    # Windows 콘솔에서 UTF-8 출력 강제
    if sys.platform == "win32":
        sys.stdout = io.TextIOWrapper(
            sys.stdout.buffer, encoding="utf-8", errors="replace"
        )
        sys.stderr = io.TextIOWrapper(
            sys.stderr.buffer, encoding="utf-8", errors="replace"
        )

    parser = argparse.ArgumentParser(
        description="EUC-KR → UTF-8 BOM 인코딩 변환"
    )
    parser.add_argument("path", help="변환할 파일 또는 디렉터리 경로")
    parser.add_argument(
        "--ext",
        nargs="+",
        default=["cpp", "h"],
        metavar="EXT",
        help="처리할 확장자 목록 (기본: cpp h)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="변환 없이 감지 결과만 출력",
    )
    parser.add_argument(
        "--no-backup",
        action="store_true",
        help=".bak 백업 파일 생성 안 함",
    )
    args = parser.parse_args()

    root = Path(args.path)
    if not root.exists():
        print(f"오류: 경로를 찾을 수 없습니다: {root}", file=sys.stderr)
        sys.exit(1)

    files = collect_files(root, args.ext)
    if not files:
        print("처리할 파일이 없습니다.")
        return

    print(f"{'[DRY RUN] ' if args.dry_run else ''}총 {len(files)}개 파일 검사 중...\n")

    stats = {"converted": 0, "bom_added": 0, "skipped": 0, "error": 0}
    korean_total_before = 0
    korean_total_after = 0

    for fp in files:
        result = detect_and_convert(fp, dry_run=args.dry_run, no_backup=args.no_backup)
        status = result["status"]
        reason = result.get("reason", "")
        kb = result.get("korean_before", 0)
        ka = result.get("korean_after", 0)
        korean_total_before += kb
        korean_total_after += ka

        try:
            rel = fp.relative_to(root)
        except ValueError:
            rel = fp
        marker = {
            "converted": "✓",
            "bom_added": "+",
            "skipped": "·",
            "error": "✗",
            "dry_run": "?",
        }.get(status, " ")

        print(f"  {marker} {rel}  [{reason}]")
        if status in ("converted", "bom_added"):
            if kb != ka:
                print(f"    ⚠ 한글 문자 수 불일치: 변환 전 {kb} → 변환 후 {ka}")
            stats[status] += 1
        elif status == "dry_run":
            action = result.get("action", "none")
            if action == "convert":
                stats["converted"] += 1
            else:
                stats["bom_added"] += 1
        elif status == "skipped":
            stats["skipped"] += 1
        else:
            stats["error"] += 1

    print(f"""
{'='*50}
결과 요약
  {'변환 예정' if args.dry_run else '변환 완료'} (EUC-KR→UTF-8 BOM): {stats['converted']}개
  BOM 추가{'(예정)' if args.dry_run else ''}:               {stats['bom_added']}개
  건너뜀:                        {stats['skipped']}개
  오류:                          {stats['error']}개
  한글 문자 (전체): {korean_total_before}자 → {korean_total_after}자 {'✓' if korean_total_before == korean_total_after else '⚠ 불일치!'}
{'='*50}""")

    if not args.dry_run and not args.no_backup:
        print("  백업 파일(.bak)이 생성되었습니다. 검증 후 삭제하세요.")


if __name__ == "__main__":
    main()
