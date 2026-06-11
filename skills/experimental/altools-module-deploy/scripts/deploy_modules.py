"""modules.json 모듈 업데이트/추가 배포 — 파일 업로드 + modules.json 갱신 + applist.json checksum 갱신 + purge."""

import argparse
import hashlib
import json
import os
import sys
import tempfile
import traceback

from cdn_utils import (
    AppInfoError,
    build_purge_targets,
    compute_sha256,
    create_sftp_connection,
    download_appinfo,
    find_entries_by_filename,
    get_cdn_config,
    get_exe_version,
    get_purge_config,
    purge_urls,
    to_cdn_url,
    to_sftp_path,
    upload_files,
)

CDN_BASE = "https://test-aldn.altools.co.kr/altools/altoolsmanager"
MODULES_URL = f"{CDN_BASE}/appinfo/modules.json"
APPLIST_URL = f"{CDN_BASE}/appinfo/applist.json"


def parse_args():
    parser = argparse.ArgumentParser(description="Deploy module updates to CDN")
    sub = parser.add_subparsers(dest="action", required=True)

    p_update = sub.add_parser("update", help="Update existing module entry")
    p_update.add_argument("local_file", help="Local module file path")
    p_update.add_argument("--id", default=None, help="Module ID (auto-detected from filename if omitted)")
    p_update.add_argument("--version", default=None, help="Version override")

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
    """기존 모듈 엔트리를 업데이트하고, 업로드에 필요한 정보를 반환."""
    files = modules["module"]["files"]
    local_filename = os.path.basename(args.local_file)

    if args.id:
        matches = [(i, f) for i, f in enumerate(files) if f.get("id") == args.id]
        if not matches:
            raise AppInfoError(f"module ID not found: {args.id}")
    else:
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

    # 업로드 대상: 기존 URL 경로에 파일 업로드
    remote_url = entry["url"]
    files[idx] = entry
    return modules, remote_url


def do_add(modules, args):
    """신규 모듈 엔트리를 추가하고, 업로드에 필요한 정보를 반환."""
    files = modules["module"]["files"]

    existing_ids = {f["id"] for f in files}
    if args.id in existing_ids:
        raise AppInfoError(f"module ID already exists: {args.id}. Use 'update' action instead.")

    checksum = compute_sha256(args.local_file)
    filename = args.filename or os.path.basename(args.local_file)
    url = to_cdn_url(args.url)

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

    new_entry = dict(sorted(new_entry.items()))

    files.append(new_entry)
    print(f"[checksum] {checksum}")
    print(f"[id]       {args.id}")

    return modules, url


def main():
    temp_files = []
    try:
        args = parse_args()
        host, port, username, password = get_cdn_config()
        base_prefix, purge_url = get_purge_config()

        # 1. modules.json 다운로드
        print(f"[1/5] modules.json 다운로드...")
        modules = download_appinfo(MODULES_URL)

        # 2. 변경 적용
        print(f"[2/5] 모듈 엔트리 {args.action}...")
        if args.action == "update":
            modules, remote_url = do_update(modules, args)
        elif args.action == "add":
            modules, remote_url = do_add(modules, args)

        # 3. modules.json 임시 파일 생성
        modules_str = json.dumps(modules, indent=4, ensure_ascii=False)
        modules_checksum = hashlib.sha256(modules_str.encode("utf-8")).hexdigest()
        print(f"[modules checksum] {modules_checksum}")

        tmp_modules = tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, encoding="utf-8"
        )
        tmp_modules.write(modules_str)
        tmp_modules.close()
        temp_files.append(tmp_modules.name)

        # 4. applist.json 체크섬 갱신
        print(f"[3/5] applist.json 체크섬 갱신...")
        applist = download_appinfo(APPLIST_URL)
        old_checksum = applist.get("modules", {}).get("checksum", "")
        applist["modules"]["checksum"] = modules_checksum
        print(f"  {old_checksum} -> {modules_checksum}")

        applist_str = json.dumps(applist, indent=4, ensure_ascii=False)
        tmp_applist = tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, encoding="utf-8"
        )
        tmp_applist.write(applist_str)
        tmp_applist.close()
        temp_files.append(tmp_applist.name)

        # 5. SFTP 업로드
        print(f"[4/5] SFTP 업로드...")
        upload_queue = [
            {"local": args.local_file, "remote": to_sftp_path(remote_url)},
            {"local": tmp_modules.name, "remote": to_sftp_path(MODULES_URL)},
            {"local": tmp_applist.name, "remote": to_sftp_path(APPLIST_URL)},
        ]

        with create_sftp_connection(host, port, username, password) as sftp:
            uploaded_paths = upload_files(sftp, upload_queue)

        for p in uploaded_paths:
            print(f"  uploaded: {p}")

        # 6. CDN 캐시 퍼지
        print(f"[5/5] CDN 캐시 퍼지...")
        targets = build_purge_targets(uploaded_paths, base_prefix)
        purge_urls(username, password, purge_url, targets)
        for t in targets:
            print(f"  purged: {t}")

        print("\n[done]")
        return 0

    except Exception:
        sys.stderr.write(traceback.format_exc())
        return 1

    finally:
        for f in temp_files:
            try:
                os.unlink(f)
            except OSError:
                pass


if __name__ == "__main__":
    raise SystemExit(main())
