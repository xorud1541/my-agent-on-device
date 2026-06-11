"""CDN appinfo 변경사항 미리보기 — unified diff를 stdout에 출력."""

import argparse
import difflib
import json
import sys
import traceback

from cdn_utils import (
    AppInfoError,
    compute_sha256,
    download_appinfo,
    get_exe_version,
    parse_major_version,
    to_cdn_url,
    update_appinfo,
)


def load_local_appinfo(path):
    """로컬 appinfo JSON 파일을 읽는다."""
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def colorize_diff(diff_text):
    """diff 출력에 ANSI 색상을 적용한다."""
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


def format_unified_diff(before_json, after_json):
    """두 JSON 객체를 unified diff 형식으로 비교."""
    before_lines = json.dumps(before_json, ensure_ascii=False, indent=4).splitlines(keepends=True)
    after_lines = json.dumps(after_json, ensure_ascii=False, indent=4).splitlines(keepends=True)
    diff = difflib.unified_diff(before_lines, after_lines, fromfile="before", tofile="after")
    return colorize_diff("".join(diff))


def parse_args():
    parser = argparse.ArgumentParser(description="Preview appinfo changes")
    parser.add_argument("setup_local", help="Local path to setup file")
    parser.add_argument("--appinfo", required=True, help="CDN appinfo URL")
    parser.add_argument("--remote", required=True, help="CDN setup remote URL")
    parser.add_argument("--local-appinfo", default=None, help="Local appinfo JSON file (skip CDN download)")
    parser.add_argument("--pub", default="{}", help="JSON string of pub field overrides")
    return parser.parse_args()


def main():
    try:
        args = parse_args()

        appinfo_url = to_cdn_url(args.appinfo)
        setup_url = to_cdn_url(args.remote)
        pub_fields = json.loads(args.pub)

        print(f"\n{'='*60}")
        print(f"App: {appinfo_url}")
        print(f"{'='*60}")

        # 1. appinfo 로드 (로컬 파일 또는 CDN)
        if args.local_appinfo:
            print(f"[local] {args.local_appinfo}")
            appinfo = load_local_appinfo(args.local_appinfo)
        else:
            print(f"[download] {appinfo_url}")
            appinfo = download_appinfo(appinfo_url)

        # 2. setup 파일 분석
        checksum = compute_sha256(args.setup_local)
        version = get_exe_version(args.setup_local)
        if version is None:
            print(f"[version] win32api unavailable — version not extracted")
            if "version" not in pub_fields:
                raise AppInfoError("version is required: pass --pub '{\"version\": \"x.x.x.x\"}'")
            version = pub_fields["version"]

        major = parse_major_version(version)
        print(f"[version]  {version}")
        print(f"[checksum] {checksum}")

        # 3. 머지 결과 생성
        updated_appinfo = update_appinfo(
            appinfo, pub_fields, setup_url, checksum, version, major
        )

        # 4. unified diff 출력
        diff_output = format_unified_diff(appinfo, updated_appinfo)
        if diff_output:
            print()
            print(diff_output)
        else:
            print("\n[no changes]")

        # 5. 업로드될 전체 appinfo JSON 출력
        print(f"\n{'='*60}")
        print("Full appinfo (after):")
        print(f"{'='*60}")
        print(json.dumps(updated_appinfo, ensure_ascii=False, indent=4))

        print(f"\n{'='*60}")
        print("Preview complete.")
        return 0

    except Exception:
        sys.stderr.write(traceback.format_exc())
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
