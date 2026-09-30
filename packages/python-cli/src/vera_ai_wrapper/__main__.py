from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import shlex
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
from urllib.error import HTTPError
from urllib.parse import urlparse
import zipfile
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path


DEFAULT_REPO = "VeraTools/Vera"
MAX_REDIRECTS = 5
REQUEST_TIMEOUT = 30
DOWNLOAD_TIMEOUT = 180
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
MAX_BINARY_BYTES = 512 * 1024 * 1024


def package_version() -> str:
    try:
        return version("vera-ai")
    except PackageNotFoundError:
        pyproject = Path(__file__).resolve().parents[2] / "pyproject.toml"
        for line in pyproject.read_text(encoding="utf-8").splitlines():
            if line.startswith("version = "):
                return line.split("=", 1)[1].strip().strip('"')
        raise


def _detect_musl() -> bool:
    if platform.system().lower() != "linux":
        return False
    try:
        result = subprocess.run(
            ["ldd", "--version"], capture_output=True, text=True, check=False, timeout=5,
        )
        combined = (result.stdout or "") + (result.stderr or "")
        if "musl" in combined.lower():
            return True
    except (OSError, subprocess.TimeoutExpired):
        pass
    try:
        return any(e.startswith("ld-musl-") for e in os.listdir("/lib"))
    except OSError:
        return False


def resolve_target() -> str:
    override = os.environ.get("VERA_TARGET")
    if override:
        return override

    system = platform.system().lower()
    machine = platform.machine().lower()

    linux_x86 = "x86_64-unknown-linux-musl" if _detect_musl() else "x86_64-unknown-linux-gnu"
    targets = {
        ("linux", "x86_64"): linux_x86,
        ("linux", "amd64"): linux_x86,
        ("linux", "aarch64"): "aarch64-unknown-linux-gnu",
        ("linux", "arm64"): "aarch64-unknown-linux-gnu",
        ("darwin", "x86_64"): "x86_64-apple-darwin",
        ("darwin", "arm64"): "aarch64-apple-darwin",
        ("windows", "amd64"): "x86_64-pc-windows-msvc",
        ("windows", "x86_64"): "x86_64-pc-windows-msvc",
    }

    try:
        return targets[(system, machine)]
    except KeyError as exc:
        raise RuntimeError(f"unsupported platform: {platform.system()}/{platform.machine()}") from exc


def default_release_base_url() -> str:
    return os.environ.get("VERA_RELEASE_BASE_URL", f"https://github.com/{DEFAULT_REPO}")


def manifest_url() -> str:
    return os.environ.get(
        "VERA_MANIFEST_URL",
        f"{default_release_base_url()}/releases/download/v{package_version()}/release-manifest.json",
    )


def vera_home() -> Path:
    return Path(os.environ.get("VERA_HOME", Path.home() / ".vera")).expanduser()


def install_metadata_path() -> Path:
    return vera_home() / "install.json"


def detect_wrapper_install_method() -> str | None:
    explicit = os.environ.get("VERA_INSTALL_METHOD")
    if explicit in {"pip", "uv"}:
        return explicit

    if any(
        os.environ.get(name)
        for name in ("UV", "UV_CACHE_DIR", "UV_TOOL_DIR", "UV_TOOL_BIN_DIR")
    ):
        return "uv"

    return "pip"


def read_install_metadata() -> dict[str, object]:
    path = install_metadata_path()
    if not path.exists():
        return {}

    try:
        value = json.loads(path.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def write_install_metadata(
    *,
    install_method: str | None,
    version_value: str | None,
    binary_path: Path | None,
    target: str | None = None,
) -> None:
    path = install_metadata_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    current = read_install_metadata()
    payload = {
        "install_method": install_method or current.get("install_method"),
        "version": version_value or current.get("version"),
        "binary_path": str(binary_path) if binary_path is not None else current.get("binary_path"),
        "target": target or current.get("target"),
        "manifest_url": os.environ.get("VERA_MANIFEST_URL"),
        "requested_version": package_version(),
    }
    tmp_path = path.with_suffix(f".tmp.{os.getpid()}")
    tmp_path.write_text(f"{json.dumps(payload, indent=2)}\n", encoding="utf-8")
    tmp_path.replace(path)


def preferred_bin_dirs() -> list[Path]:
    override = os.environ.get("VERA_USER_BIN_DIR")
    if override:
        return [Path(override).expanduser()]

    home = Path.home()
    if os.name == "nt":
        return [
            home / "AppData" / "Roaming" / "npm",
            home / "AppData" / "Local" / "Programs" / "Vera" / "bin",
        ]

    return [home / ".local" / "bin", home / ".cargo" / "bin", home / "bin"]


def path_entries() -> set[Path]:
    entries = os.environ.get("PATH", "").split(os.pathsep)
    return {Path(entry).expanduser().resolve() for entry in entries if entry}


def pick_user_bin_dir() -> Path:
    entries = path_entries()
    for candidate in preferred_bin_dirs():
        resolved = candidate.expanduser().resolve()
        if resolved in entries:
            return resolved
    return preferred_bin_dirs()[0].expanduser().resolve()


def binary_name() -> str:
    return "vera.exe" if os.name == "nt" else "vera"


def shim_name() -> str:
    return "vera.cmd" if os.name == "nt" else "vera"


class LimitedRedirects(urllib.request.HTTPRedirectHandler):
    max_redirections = MAX_REDIRECTS

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        try:
            validate_url(newurl)
        except Exception:
            fp.close()
            raise
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def validate_url(url: str) -> None:
    if urlparse(url).scheme not in {"https", "http"}:
        raise RuntimeError("release downloads require an HTTP or HTTPS URL")


def response_chunks(response, limit: int, deadline: float):
    size = 0
    while True:
        if time.monotonic() > deadline:
            raise TimeoutError("release download timed out")
        chunk = response.read1(64 * 1024)
        if not chunk:
            break
        size += len(chunk)
        if size > limit:
            raise RuntimeError("release response exceeds its size limit")
        yield chunk


def open_response(url: str):
    validate_url(url)
    opener = urllib.request.build_opener(LimitedRedirects())
    try:
        return opener.open(url, timeout=REQUEST_TIMEOUT)
    except HTTPError as error:
        error.close()
        raise RuntimeError(f"release request failed: HTTP {error.code}") from error


def read_json(url: str) -> dict[str, object]:
    deadline = time.monotonic() + REQUEST_TIMEOUT
    with open_response(url) as response:
        payload = b"".join(response_chunks(response, MAX_MANIFEST_BYTES, deadline))
    return json.loads(payload.decode("utf-8"))


def load_manifest() -> dict[str, object]:
    manifest = read_json(manifest_url())
    if not isinstance(manifest, dict):
        raise RuntimeError("invalid release manifest")
    version_value = manifest.get("version")
    if not isinstance(version_value, str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", version_value):
        raise RuntimeError("invalid release version")
    if not os.environ.get("VERA_MANIFEST_URL") and version_value != package_version():
        raise RuntimeError(f"requested Vera {package_version()}, manifest contains {version_value}")
    return manifest


def download_file(url: str, destination: Path, expected_size: int) -> None:
    deadline = time.monotonic() + DOWNLOAD_TIMEOUT
    try:
        with open_response(url) as response, destination.open("xb") as handle:
            for chunk in response_chunks(response, expected_size, deadline):
                handle.write(chunk)
        if destination.stat().st_size != expected_size:
            raise RuntimeError("release archive size mismatch")
    except BaseException:
        destination.unlink(missing_ok=True)
        raise


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_member(name: str) -> bool:
    return bool(name) and not name.startswith("/") and "\\" not in name and all(
        part not in {"..", ".", ""} for part in name.rstrip("/").split("/")
    ) and ":" not in name and "\x00" not in name


def extract_archive(archive_path: Path, destination: Path, target: str) -> None:
    expected = f"vera-{target}/{binary_name()}"
    if archive_path.suffix == ".zip":
        with zipfile.ZipFile(archive_path) as archive:
            entries = archive.infolist()
            if len(entries) > 1024 or sum(entry.file_size for entry in entries) > MAX_BINARY_BYTES:
                raise RuntimeError("release archive exceeds its size limit")
            names = set()
            for entry in entries:
                mode = entry.external_attr >> 16
                if entry.filename in names or not safe_member(entry.filename) or stat.S_ISLNK(mode) or (
                    stat.S_IFMT(mode) not in {0, stat.S_IFREG, stat.S_IFDIR}
                ):
                    raise RuntimeError("unsafe release archive member")
                names.add(entry.filename)
            matches = [entry for entry in entries if entry.filename == expected and not entry.is_dir()
                       and stat.S_IFMT(entry.external_attr >> 16) != stat.S_IFDIR]
            if len(matches) != 1:
                raise RuntimeError("release archive must contain exactly one Vera binary")
            entry = matches[0]
            with archive.open(entry) as source:
                copy_binary(source, destination, entry.file_size)
        return

    with tarfile.open(archive_path, "r|gz") as archive:
        matches = 0
        expanded = 0
        names = set()
        for entry in archive:
            if entry.name in names or len(names) >= 1024 or not safe_member(entry.name) or not (entry.isfile() or entry.isdir()):
                raise RuntimeError("unsafe release archive member")
            names.add(entry.name)
            expanded += entry.size
            if expanded > MAX_BINARY_BYTES or entry.size < 0 or (entry.isdir() and entry.size):
                raise RuntimeError("release archive exceeds its size limit")
            if entry.name == expected and entry.isfile():
                matches += 1
                with archive.extractfile(entry) as source:
                    copy_binary(source, destination, entry.size)
        if matches != 1:
            raise RuntimeError("release archive must contain exactly one Vera binary")


def copy_binary(source, destination: Path, size: int) -> None:
    if not 0 < size <= MAX_BINARY_BYTES:
        raise RuntimeError("invalid release binary size")
    with destination.open("xb") as output:
        remaining = size
        while remaining:
            chunk = source.read(min(remaining, 64 * 1024))
            if not chunk:
                raise RuntimeError("incomplete release binary")
            output.write(chunk)
            remaining -= len(chunk)


def create_shim(binary_path: Path) -> Path:
    bin_dir = pick_user_bin_dir()
    bin_dir.mkdir(parents=True, exist_ok=True)
    shim_path = bin_dir / shim_name()
    if shim_path.absolute() == binary_path.absolute():
        return shim_path
    if os.name == "nt":
        escaped_path = str(binary_path).replace("%", "%%")
        contents = f'@echo off\r\nsetlocal DisableDelayedExpansion\r\n"{escaped_path}" %*\r\n'
    else:
        contents = f'#!/bin/sh\nexec {shlex.quote(str(binary_path))} "$@"\n'
    with tempfile.NamedTemporaryFile(prefix=".vera-shim-", dir=bin_dir, delete=False) as handle:
        staged_shim = Path(handle.name)
        handle.write(contents.encode("utf-8"))
    try:
        if os.name != "nt":
            staged_shim.chmod(0o755)
        staged_shim.replace(shim_path)
    finally:
        staged_shim.unlink(missing_ok=True)
    return shim_path


def cached_binary(target: str, requested_version: str) -> tuple[Path, str] | None:
    version_value = requested_version
    if os.environ.get("VERA_MANIFEST_URL"):
        metadata = read_install_metadata()
        if metadata.get("manifest_url") != manifest_url() or metadata.get("requested_version") != requested_version:
            return None
        version_value = metadata.get("version", "")
        if not isinstance(version_value, str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", version_value):
            return None
    binary_path = vera_home() / "bin" / version_value / target / binary_name()
    try:
        receipt = json.loads(Path(f"{binary_path}.receipt.json").read_text(encoding="utf-8"))
        info = binary_path.lstat()
        if stat.S_ISREG(info.st_mode) and info.st_size > 0 and receipt == {
            "version": version_value, "target": target, "size": info.st_size, "sha256": sha256(binary_path),
        }:
            return binary_path, version_value
    except (OSError, ValueError):
        pass
    return None


def finish_install(binary_path: Path, version_value: str, target: str, announce: bool = False) -> tuple[Path, str]:
    shim_path = create_shim(binary_path)
    if announce and shim_path.parent.resolve() not in path_entries():
        print(f"Added Vera to {shim_path.parent}. Add that directory to PATH to run `vera` directly.", file=sys.stderr)
    write_install_metadata(
        install_method=detect_wrapper_install_method(), version_value=version_value,
        binary_path=binary_path, target=target,
    )
    return binary_path, version_value


def ensure_binary_installed() -> tuple[Path, str]:
    target = resolve_target()
    if not re.fullmatch(r"[A-Za-z0-9_-]+", target):
        raise RuntimeError("invalid release target")
    cached = cached_binary(target, package_version())
    if cached:
        return finish_install(cached[0], cached[1], target)
    manifest = load_manifest()
    assets = manifest.get("assets", {})
    asset = assets.get(target) if isinstance(assets, dict) else None
    if not isinstance(asset, dict):
        raise RuntimeError(f"no release asset for target {target}")
    extension = ".zip" if target.endswith("windows-msvc") else ".tar.gz"
    archive_name = f"vera-{target}{extension}"
    expected_size = asset.get("size")
    if asset.get("archive") != archive_name or type(expected_size) is not int or not 0 < expected_size <= MAX_ARCHIVE_BYTES:
        raise RuntimeError("invalid release archive metadata")
    checksum = asset.get("sha256", "")
    if not isinstance(checksum, str) or not re.fullmatch(r"[0-9a-f]{64}", checksum):
        raise RuntimeError("invalid release archive checksum")
    version_value = manifest["version"]
    install_dir = vera_home() / "bin" / version_value / target
    install_dir.mkdir(parents=True, exist_ok=True)
    binary_path = install_dir / binary_name()
    # Staging on the destination filesystem makes publication atomic.
    with tempfile.TemporaryDirectory(prefix=".install-", dir=install_dir) as temp_dir_str:
        temp_dir = Path(temp_dir_str)
        archive_path = temp_dir / archive_name
        staged_binary = temp_dir / binary_name()
        print(f"Downloading Vera {version_value} for {target}...", file=sys.stderr)
        download_file(str(asset["download_url"]), archive_path, expected_size)
        if sha256(archive_path) != checksum:
            raise RuntimeError(f"checksum mismatch for {archive_name}")
        extract_archive(archive_path, staged_binary, target)
        if os.name != "nt":
            staged_binary.chmod(0o755)
        receipt = {
            "version": version_value, "target": target,
            "size": staged_binary.stat().st_size, "sha256": sha256(staged_binary),
        }
        staged_receipt = temp_dir / "receipt.json"
        staged_receipt.write_text(json.dumps(receipt), encoding="utf-8")
        staged_binary.replace(binary_path)
        staged_receipt.replace(Path(f"{binary_path}.receipt.json"))
    return finish_install(binary_path, version_value, target, announce=True)


def run_binary(binary_path: Path, args: list[str]) -> int:
    if os.name != "nt":
        os.execv(str(binary_path), [str(binary_path), *args])
    result = subprocess.run([str(binary_path), *args], check=False)
    return result.returncode if result.returncode >= 0 else 128 - result.returncode


def run() -> int:
    command = sys.argv[1] if len(sys.argv) > 1 else "help"
    rest = sys.argv[2:]
    binary_path, version_value = ensure_binary_installed()

    if command == "install":
        print(f"Vera {version_value} installed.", file=sys.stderr)
        return run_binary(binary_path, ["agent", "install", *rest])

    if command == "help":
        return run_binary(binary_path, ["--help"])

    return run_binary(binary_path, [command, *rest])


def main() -> int:
    try:
        return run()
    except KeyboardInterrupt:
        print("Vera cancelled.", file=sys.stderr)
        return 130
    except Exception as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
