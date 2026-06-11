"""modules.json 변경사항 미리보기 — unified diff를 stdout에 출력."""

import argparse
import difflib
import hashlib
import json
import sys
import traceback

from cdn_utils import (
    AppInfoError,
    CA_BUNDLE,
    compute_sha256,
    download_appinfo,
    find_entries_by_filename,
    get_exe_version,
    to_cdn_url,
)

CDN_BASE = "https://test-aldn.altools.co.kr/altools/altoolsmanager"
MODULES_URL = f"{CDN_BASE}/appinfo/modules.json"
APPLIST_URL = f"{CDN_BASE}/appinfo/applist.json"


def colorize_diff(diff_text):
    RED = "\033[31m"
    GREEN = "\033[32m"
    CYAN = "\033[36m"
    RESET = "\033[0m"
    lines = []
    for line in diff_text.splitlines(keepends=True):
        if line.startswith("---"):
            lines.append(f"{RED}{line}{RESET}")
        elif line.startswith("+++"):
            lines.append(f"{GREEN}{line}{RESET}")
        elif line.startswith("@@"):
            lines.append(f"{CYAN}{line}{RESET}")
        elif line.startswith("-"):
            lines.append(f"{RED}{line}{RESET}")
        elif line.startswith("+"):
            lines.append(f"{GREEN}{line}{RESET}")
        else:
            lines.append(line)
    return "".join(lines)


def format_unified_diff(before_str, after_str, fromfile="before", tofile="after"):
    before_lines = before_str.splitlines(keepends=True)
    after_lines = after_str.splitlines(keepends=True)
    diff = difflib.unified_diff(before_lines, after_lines, fromfile=fromfile, tofile=tofile)
    return colorize_diff("".join(diff))


def parse_args():
    parser = argparse.ArgumentParser(description="Preview modules.json changes")
    sub = parser.add_subparsers(dest="action", required=True)

    # update: 기존 모듈 업데이트
    p_update = sub.add_parser("update", help="Update existing module entry")
    p_update.add_argument("local_file", help="Local module file path")
    p_update.add_argument("--id", default=None, help="Module ID (auto-detected from filename if omitted)")
    p_update.add_argument("--version", default=None, help="Version override")

    # add: 신규 모듈 추가
    p_add = sub.add_parser("add", help="Add new module entry")
    p_add.add_argument("local_file", help="Local module file path")
    p_add.add_argument("--id", required=True, help="New module ID")
    p_add.add_argument("--url", required=True, help="Remote CDN URL for the module file")
    p_add.add_argument("--architecture", default=None, help="Architecture (e.g., x64)")
    p_add.add_argument("--version", default=None, help="Module version")
    p_add.add_argument("--filename", default=None, help="Filename override (default: from local file)")
    p_add.add_argument("--updaterate", type=int, default=100, help="Update rate (default: 100)")
    p_add.add_argument("--dir", default=None, help="Install directory")

    return parser.parse_args()


def do_update(modules, args):
    """기존 모듈 엔트리를 업데이트한다."""
    import os
    files = modules["module"]["files"]
    local_filename = os.path.basename(args.local_file)

    if args.id:
        # ID로 직접 찾기
        matches = [(i, f) for i, f in enumerate(files) if f.get("id") == args.id]
        if not matches:
            raise AppInfoError(f"module ID not found: {args.id}")
    else:
        # filename으로 자동 매칭
        matches = find_entries_by_filename(files, local_filename)
        if not matches:
            raise AppInfoError(
                f"no module entry with filename '{local_filename}' found in modules.json. "
                f"Use --id to specify the module ID explicitly."
            )
        if len(matches) > 1:
            ids = [f["id"] for _, f in matches]
            raise AppInfoError(
                f"multiple entries found for filename '{local_filename}': {ids}. "
                f"Use --id to specify which one to update."
            )

    idx, entry = matches[0]
    checksum = compute_sha256(args.local_file)

    # 버전 추출
    version = args.version
    if version is None:
        version = get_exe_version(args.local_file)
    if version is not None:
        print(f"[version]  {version}")
        entry["version"] = version
    elif "version" in entry:
        print(f"[version]  (keeping existing: {entry['version']})")

    entry["checksum"] = checksum
    print(f"[checksum] {checksum}")
    print(f"[id]       {entry['id']}")

    files[idx] = entry
    return modules


def do_add(modules, args):
    """신규 모듈 엔트리를 추가한다."""
    import os
    files = modules["module"]["files"]

    # 중복 ID 체크
    existing_ids = {f["id"] for f in files}
    if args.id in existing_ids:
        raise AppInfoError(f"module ID already exists: {args.id}. Use 'update' action instead.")

    checksum = compute_sha256(args.local_file)
    filename = args.filename or os.path.basename(args.local_file)
    url = to_cdn_url(args.url)

    # 버전 추출
    version = args.version
    if version is None:
        version = get_exe_version(args.local_file)
    if version is not None:
        print(f"[version]  {version}")

    new_entry = {
        "checksum": checksum,
        "filename": filename,
        "id": args.id,
        "updaterate": args.updaterate,
        "url": url,
    }

    if args.architecture:
        new_entry["architecture"] = args.architecture
    if args.dir:
        new_entry["dir"] = args.dir
    if version:
        new_entry["version"] = version

    # 알파벳 순으로 키 정렬
    new_entry = dict(sorted(new_entry.items()))

    files.append(new_entry)
    print(f"[checksum] {checksum}")
    print(f"[id]       {args.id}")

    return modules


def main():
    try:
        args = parse_args()

        print(f"\n{'=' * 60}")
        print(f"Action: {args.action}")
        print(f"{'=' * 60}")

        # 1. modules.json 다운로드
        print(f"[download] {MODULES_URL}")
        modules = download_appinfo(MODULES_URL)
        modules_old_str = json.dumps(modules, indent=4, ensure_ascii=False)

        # 2. 변경 적용
        if args.action == "update":
            modules = do_update(modules, args)
        elif args.action == "add":
            modules = do_add(modules, args)

        modules_new_str = json.dumps(modules, indent=4, ensure_ascii=False)

        # 3. modules.json diff 출력
        diff_output = format_unified_diff(modules_old_str, modules_new_str,
                                          "modules.json (current)", "modules.json (modified)")
        if diff_output:
            print(f"\n--- modules.json diff ---\n")
            print(diff_output)
        else:
            print("\n[no changes to modules.json]")

        # 4. applist.json checksum 변경 미리보기
        modules_new_checksum = hashlib.sha256(modules_new_str.encode("utf-8")).hexdigest()
        print(f"\n{'=' * 60}")
        print(f"[download] {APPLIST_URL}")
        applist = download_appinfo(APPLIST_URL)
        old_checksum = applist.get("modules", {}).get("checksum", "")

        print(f"\n--- applist.json modules checksum ---\n")
        print(f"  before: {old_checksum}")
        print(f"  after:  {modules_new_checksum}")

        # 5. 전체 수정된 modules.json 출력
        print(f"\n{'=' * 60}")
        print("Full modules.json (after):")
        print(f"{'=' * 60}")
        print(modules_new_str)

        print(f"\n{'=' * 60}")
        print("Preview complete.")
        return 0

    except Exception:
        sys.stderr.write(traceback.format_exc())
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
