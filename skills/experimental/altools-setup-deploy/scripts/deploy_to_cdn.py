"""CDN 배포 — setup 파일 업로드 + appinfo 갱신 + purge."""

import argparse
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
    get_cdn_config,
    get_exe_version,
    get_purge_config,
    parse_major_version,
    purge_urls,
    to_cdn_url,
    to_sftp_path,
    update_appinfo,
    upload_files,
)


def parse_args():
    parser = argparse.ArgumentParser(description="Deploy to CDN")
    parser.add_argument("setup_local", help="Local path to setup file")
    parser.add_argument("--appinfo", required=True, help="CDN appinfo URL")
    parser.add_argument("--remote", required=True, help="CDN setup remote URL")
    parser.add_argument("--local-appinfo", default=None, help="Local appinfo JSON file (skip CDN download)")
    parser.add_argument("--pub", default="{}", help="JSON string of pub field overrides")
    return parser.parse_args()


def main():
    temp_files = []
    try:
        args = parse_args()

        appinfo_url = to_cdn_url(args.appinfo)
        appinfo_remote = to_sftp_path(args.appinfo)
        setup_url = to_cdn_url(args.remote)
        setup_remote = to_sftp_path(args.remote)
        pub_fields = json.loads(args.pub)

        host, port, username, password = get_cdn_config()
        base_prefix, purge_url = get_purge_config()

        # 1. appinfo 로드 (로컬 파일 또는 CDN)
        if args.local_appinfo:
            print(f"[local] {args.local_appinfo}")
            with open(args.local_appinfo, "r", encoding="utf-8") as f:
                appinfo = json.load(f)
        else:
            print(f"[download] {appinfo_url}")
            appinfo = download_appinfo(appinfo_url)

        # 2. setup 파일 분석
        print(f"[checksum] {args.setup_local}")
        checksum = compute_sha256(args.setup_local)

        version = get_exe_version(args.setup_local)
        if version is None:
            if "version" not in pub_fields:
                raise AppInfoError("version is required: pass --pub '{\"version\": \"x.x.x.x\"}'")
            version = pub_fields["version"]
        print(f"[version]  {version}")

        major = parse_major_version(version)

        # 3. appinfo 머지
        updated_appinfo = update_appinfo(
            appinfo, pub_fields, setup_url, checksum, version, major
        )

        # 4. 임시 파일로 저장
        tmp = tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, encoding="utf-8"
        )
        json.dump(updated_appinfo, tmp, ensure_ascii=False, indent=4)
        tmp.close()
        temp_files.append(tmp.name)

        # 5. SFTP 업로드 (setup → appinfo 순서)
        upload_queue = [
            {"local": args.setup_local, "remote": setup_remote},
            {"local": tmp.name, "remote": appinfo_remote},
        ]

        print("[sftp] connecting...")
        with create_sftp_connection(host, port, username, password) as sftp:
            uploaded_paths = upload_files(sftp, upload_queue)

        # 6. Purge
        print("[purge] requesting...")
        targets = build_purge_targets(uploaded_paths, base_prefix)
        purge_urls(username, password, purge_url, targets)

        print("[done]")
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
