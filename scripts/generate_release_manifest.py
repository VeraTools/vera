#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path


ARCHIVE_RE = re.compile(r"^vera-(?P<target>.+)\.(?P<extension>tar\.gz|zip)$")
TARGETS = {
    "x86_64-unknown-linux-gnu": "tar.gz",
    "x86_64-unknown-linux-musl": "tar.gz",
    "aarch64-unknown-linux-gnu": "tar.gz",
    "x86_64-apple-darwin": "tar.gz",
    "aarch64-apple-darwin": "tar.gz",
    "x86_64-pc-windows-msvc": "zip",
}


def validate_manifest(manifest: dict, tag: str, repo: str) -> None:
    if (manifest.get("tag"), manifest.get("version"), manifest.get("repo")) != (
        tag, tag.removeprefix("v"), repo
    ):
        raise ValueError("manifest does not identify the requested release")
    assets = manifest.get("assets", {})
    if assets.keys() != TARGETS.keys():
        raise ValueError("release manifest must contain exactly the six supported targets")
    for target, extension in TARGETS.items():
        asset = assets[target]
        archive = f"vera-{target}.{extension}"
        if asset.get("archive") != archive or asset.get("download_url") != (
            f"https://github.com/{repo}/releases/download/{tag}/{archive}"
        ):
            raise ValueError(f"incorrect release archive for {target}")
        if not re.fullmatch(r"[0-9a-f]{64}", asset.get("sha256", "")):
            raise ValueError(f"invalid checksum for {target}")
        if type(asset.get("size")) is not int or asset["size"] <= 0:
            raise ValueError(f"invalid archive size for {target}")


def build_manifest(release_dir: Path, tag: str, repo: str) -> dict[str, object]:
    assets: dict[str, object] = {}

    for archive_path in sorted(release_dir.iterdir()):
        match = ARCHIVE_RE.match(archive_path.name)
        if not match:
            if archive_path.name.endswith((".tar.gz", ".zip")):
                raise ValueError(f"unsupported archive: {archive_path.name}")
            continue

        target = match.group("target")
        if match.group("extension") != TARGETS.get(target) or target in assets:
            raise ValueError(f"unsupported or duplicate archive: {archive_path.name}")
        if not archive_path.is_file() or archive_path.is_symlink():
            raise ValueError(f"archive must be a regular file: {archive_path.name}")
        checksum = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        assets[target] = {
            "archive": archive_path.name,
            "download_url": f"https://github.com/{repo}/releases/download/{tag}/{archive_path.name}",
            "sha256": checksum,
            "size": archive_path.stat().st_size,
        }

    version = tag[1:] if tag.startswith("v") else tag
    manifest = {
        "version": version,
        "tag": tag,
        "repo": repo,
        "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "assets": assets,
    }
    validate_manifest(manifest, tag, repo)
    return manifest


def main() -> int:
    if len(sys.argv) != 4:
        print(
            "usage: generate_release_manifest.py <release-dir> <tag> <owner/repo>",
            file=sys.stderr,
        )
        return 1

    release_dir = Path(sys.argv[1]).resolve()
    manifest = build_manifest(release_dir, sys.argv[2], sys.argv[3])
    output_path = release_dir / "release-manifest.json"
    output_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(output_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
