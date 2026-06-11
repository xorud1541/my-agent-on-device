"""CDN 공통 유틸리티 — SFTP 업로드, Purge, 인증, 파일 분석."""

import hashlib
import json
import logging
import os
import subprocess
from typing import Any, Dict, List, Optional, Tuple

import pysftp
import requests
from dotenv import load_dotenv

logger = logging.getLogger(__name__)


# ── 커스텀 예외 ────────────────────────────────────


class CDNError(Exception):
    """CDN 관련 기본 예외."""


class CDNAuthError(CDNError):
    """CDN 인증 실패."""


class CDNUploadError(CDNError):
    """CDN 업로드 실패."""


class AppInfoError(CDNError):
    """appinfo 처리 오류."""


# 지원하는 AI 도구 → (감지 환경변수, .env 디렉터리) 매핑
_TOOL_MARKERS: List[Tuple[str, str]] = [
    ("CLAUDE_CODE", ".claude"),   # Claude Code (Anthropic)
    ("CODEX", ".codex"),          # Codex CLI (OpenAI)
    ("GEMINI_CLI", ".gemini"),    # Gemini CLI (Google)
    ("CURSOR", ".cursor"),        # Cursor
]


def _detect_tool_env_dir() -> Optional[str]:
    """현재 실행 중인 AI 도구를 환경변수로 감지해 해당 .env 디렉터리명을 반환한다.

    감지 불가 시 None 반환.
    """
    for env_var, tool_dir in _TOOL_MARKERS:
        if os.getenv(env_var):
            return tool_dir
    return None


def _find_dotenv() -> None:
    """프로젝트 루트의 도구별 .env 파일을 찾아 로드한다.

    현재 실행 중인 도구를 감지해 해당 경로만 탐색한다.
    감지 불가 시 경고를 출력하고 현재 디렉터리의 .env만 시도한다.
    """
    import sys
    try:
        root = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"],
            stderr=subprocess.DEVNULL, text=True
        ).strip()
        detected = _detect_tool_env_dir()
        if detected:
            env_path = os.path.join(root, detected, ".env")
            if os.path.isfile(env_path):
                load_dotenv(env_path)
                return
        else:
            supported = ", ".join(
                f"{tool_dir}/.env (${env_var})"
                for env_var, tool_dir in _TOOL_MARKERS
            )
            print(
                f"[경고] 실행 중인 AI 도구를 감지하지 못했습니다. "
                f"지원 도구: {supported}. "
                f"프로젝트 루트의 도구별 .env를 건너뜁니다.",
                file=sys.stderr,
            )
    except (subprocess.CalledProcessError, FileNotFoundError):
        pass
    load_dotenv()


_find_dotenv()


# ── SSL/TLS 검증 설정 ──────────────────────────────
# 내부 CDN의 자체 서명 인증서 환경에서는 CDN_CA_BUNDLE 환경변수를
# 인증서 경로 또는 "false"로 설정하여 검증을 제어할 수 있다.
_ca_env = os.getenv("CDN_CA_BUNDLE", "")
if _ca_env.lower() == "false":
    CA_BUNDLE: Any = False
elif _ca_env:
    CA_BUNDLE = _ca_env
else:
    # 기본값: 자체 서명 인증서 환경을 위해 비활성화 (운영 전환 시 인증서 경로 설정 권장)
    CA_BUNDLE = False


# ── 설정 ────────────────────────────────────────


def get_cdn_config() -> Tuple[str, int, str, str]:
    host = "estsoft-ftp.lgucdn.com"
    port = 2222
    username = os.getenv("CDN_USERNAME")
    password = os.getenv("CDN_PASSWORD")
    if not username or not password:
        raise CDNAuthError("CDN_USERNAME/CDN_PASSWORD is required")
    return host, port, username, password


def get_purge_config() -> Tuple[str, str]:
    base_prefix = os.getenv("CDN_BASE_PREFIX", "/aldntest/")
    purge_url = os.getenv(
        "CDN_PURGE_URL",
        "https://api.lgucdn.com/v3/management/service/altools"
        "/domain/aldntest-altools.lgucdn.com/purge",
    )
    return base_prefix, purge_url


# ── 경로 변환 ────────────────────────────────────

CDN_URL_BASE = os.getenv("CDN_URL_BASE", "https://test-aldn.altools.co.kr/")
SFTP_PREFIX = os.getenv("CDN_BASE_PREFIX", "/aldntest/")


def to_relative_path(path: str) -> str:
    """CDN URL 또는 SFTP 경로에서 상대 경로를 추출한다."""
    if path.startswith(CDN_URL_BASE):
        return path[len(CDN_URL_BASE):]
    if path.startswith(SFTP_PREFIX):
        return path[len(SFTP_PREFIX):]
    return path.strip("/")


def to_sftp_path(path: str) -> str:
    return f"{SFTP_PREFIX}{to_relative_path(path)}"


def to_cdn_url(path: str) -> str:
    return f"{CDN_URL_BASE}{to_relative_path(path)}"


# ── 모듈 검색 ────────────────────────────────────


def find_entries_by_filename(files: list, filename: str) -> list:
    """filename이 일치하는 모든 모듈 엔트리를 반환."""
    return [(i, f) for i, f in enumerate(files) if f.get("filename") == filename]


# ── 파일 분석 ────────────────────────────────────


def compute_sha256(file_path: str) -> str:
    if not os.path.isfile(file_path):
        raise AppInfoError(f"local file not found: {file_path}")
    sha256 = hashlib.sha256()
    with open(file_path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            sha256.update(chunk)
    return sha256.hexdigest()


def get_exe_version(file_path: str) -> Optional[str]:
    """Windows PE FileVersion 추출. win32api 없으면 None 반환."""
    try:
        import win32api
        lang, codepage = win32api.GetFileVersionInfo(
            file_path, "\\VarFileInfo\\Translation"
        )[0]
        version = win32api.GetFileVersionInfo(
            file_path,
            f"\\StringFileInfo\\{lang:04X}{codepage:04X}\\FileVersion",
        )
        return version.strip()
    except ImportError:
        return None
    except Exception as e:
        logger.warning("버전 추출 실패: %s: %s", file_path, e)
        return None


def parse_major_version(version: str) -> int:
    """버전 문자열에서 major 버전 번호를 추출한다."""
    parts = version.split(".")
    if not parts or not parts[0].isdigit():
        raise AppInfoError(f"잘못된 버전 형식: {version!r}")
    return int(parts[0])


# ── appinfo ──────────────────────────────────────


def download_appinfo(url: str) -> Dict[str, Any]:
    resp = requests.get(url, verify=CA_BUNDLE, timeout=30)
    if resp.status_code != 200:
        raise AppInfoError(f"failed to download appinfo: {resp.status_code} {url}")
    return resp.json()


# pub 필드 중 허용되는 키 목록 (version, checksum, setupurl, major는 코드에서 직접 설정)
ALLOWED_PUB_FIELDS = {
    "description", "updatehistory", "updaterate", "autoupdate",
    "os", "architecture", "channel",
}


def update_appinfo(
    appinfo: Dict[str, Any],
    pub_fields: Dict[str, Any],
    setup_url: str,
    checksum: str,
    version: str,
    major: int,
) -> Dict[str, Any]:
    """appinfo의 pub 배열에서 매칭되는 항목을 머지 방식으로 갱신."""
    # 허용된 키만 통과시켜 임의 필드 주입을 방지
    safe_pub_fields = {k: v for k, v in pub_fields.items() if k in ALLOWED_PUB_FIELDS}

    updated = dict(appinfo)
    pub_list = list(updated.get("pub", []))

    target_arch = safe_pub_fields.get("architecture")
    matched = False

    for i, pub in enumerate(pub_list):
        if pub.get("major") != major:
            continue
        if target_arch and pub.get("architecture") != target_arch:
            continue
        merged = dict(pub)
        merged.update(safe_pub_fields)
        merged["version"] = version
        merged["major"] = major
        merged["setupurl"] = setup_url
        merged["checksum"] = checksum
        pub_list[i] = merged
        matched = True

    if not matched:
        raise AppInfoError(
            f"no matching pub entry for major={major}"
            + (f", architecture={target_arch}" if target_arch else "")
        )

    updated["pub"] = pub_list
    return updated


# ── SFTP ─────────────────────────────────────────


def create_sftp_connection(
    host: str, port: int, username: str, password: str
) -> pysftp.Connection:
    known_hosts = os.getenv("CDN_KNOWN_HOSTS", "")
    if known_hosts:
        cnopts = pysftp.CnOpts(knownhosts=known_hosts)
    else:
        # 내부 CDN 환경에서 known_hosts가 설정되지 않은 경우 호스트키 검증을 비활성화.
        # 운영 환경에서는 CDN_KNOWN_HOSTS 환경변수로 known_hosts 파일 경로를 지정할 것.
        cnopts = pysftp.CnOpts()
        cnopts.hostkeys = None
    return pysftp.Connection(
        host, port=port, username=username, password=password, cnopts=cnopts
    )


def ensure_remote_dir(sftp: pysftp.Connection, remote_path: str) -> None:
    remote_dir = os.path.dirname(remote_path).replace("\\", "/")
    if remote_dir and remote_dir != "/":
        sftp.makedirs(remote_dir)


def upload_files(sftp: pysftp.Connection, files: List[Dict[str, str]]) -> List[str]:
    """files: list of {"local": ..., "remote": ...}"""
    uploaded_paths = []
    for item in files:
        local_path = item["local"]
        remote_path = item["remote"]
        ensure_remote_dir(sftp, remote_path)
        sftp.put(local_path, remote_path)
        uploaded_paths.append(remote_path)
    return uploaded_paths


# ── Purge ────────────────────────────────────────


def build_purge_targets(remote_paths: List[str], base_prefix: str) -> List[str]:
    normalized_prefix = f"/{base_prefix.strip('/')}/"
    targets = []
    for remote_path in remote_paths:
        normalized_remote = remote_path.replace("\\", "/")
        if normalized_remote.startswith(normalized_prefix):
            normalized_remote = f"/{normalized_remote[len(normalized_prefix):]}"
        targets.append(f"/{normalized_remote.lstrip('/')}")
    return targets


def generate_token(
    username: str, password: str, token_url: str, expires_in: str
) -> str:
    payload = {"username": username, "password": password, "expiresIn": expires_in}
    res = requests.post(token_url, json=payload, verify=CA_BUNDLE)
    if res.status_code == 200:
        return res.json().get("token")
    raise CDNAuthError(f"failed to generate token: {res.status_code} {res.text}")


def purge_urls(
    username: str,
    password: str,
    purge_url: str,
    targets: List[str],
    expires_in: str = "1d",
) -> None:
    if not targets:
        return
    token = generate_token(
        username, password,
        token_url="https://api.lgucdn.com/v3/auth/tokens",
        expires_in=expires_in,
    )
    headers = {"Authorization": f"Bearer {token}"}
    res = requests.post(
        purge_url, headers=headers, json={"filelist": targets}, verify=CA_BUNDLE
    )
    if not (200 <= res.status_code < 300):
        raise CDNError(f"failed to purge: {res.status_code} {res.text}")
