#!/usr/bin/env python3
"""Reject a retry whose assets differ from an existing publication."""

import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path

from generate_release_manifest import validate_manifest


def preserve_assets(directory: Path, tag: str, repo: str) -> None:
    result = subprocess.run(
        ["gh", "api", f"repos/{repo}/releases/tags/{tag}"],
        text=True, capture_output=True, timeout=120,
    )
    if result.returncode:
        if "(HTTP 404)" in result.stderr:
            return
        raise RuntimeError(result.stderr.strip())
    published = json.loads(result.stdout)
    candidate = json.loads((directory / "release-manifest.json").read_text(encoding="utf-8"))
    validate_manifest(candidate, tag, repo)
    expected = {asset["archive"]: asset for asset in candidate["assets"].values()}
    with tempfile.TemporaryDirectory() as temporary:
        for asset in published["assets"]:
            name = asset["name"]
            if name not in expected and name != "release-manifest.json":
                continue
            if name in expected and asset.get("digest"):
                wanted = expected[name]
                if asset["digest"] != f"sha256:{wanted['sha256']}" or asset["size"] != wanted["size"]:
                    raise ValueError(f"published asset differs: {name}; preserve the existing release")
                continue
            subprocess.run([
                "gh", "release", "download", tag, "--repo", repo,
                "--pattern", name, "--dir", temporary, "--clobber",
            ], check=True, timeout=120)
            path = Path(temporary) / name
            if name == "release-manifest.json":
                existing = json.loads(path.read_text(encoding="utf-8"))
                validate_manifest(existing, tag, repo)
                if existing["assets"] != candidate["assets"]:
                    raise ValueError("published manifest differs; retry the failed publication job instead")
            else:
                with path.open("rb") as source:
                    checksum = hashlib.file_digest(source, "sha256").hexdigest()
                if checksum != expected[name]["sha256"] or path.stat().st_size != expected[name]["size"]:
                    raise ValueError(f"published asset differs: {name}; preserve the existing release")


if __name__ == "__main__":
    preserve_assets(Path(sys.argv[1]), sys.argv[2], sys.argv[3])
