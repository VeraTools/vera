#!/usr/bin/env python3
"""Verify the published Linux archive before staging a Docker binary."""

import hashlib
import json
import os
import shutil
import sys
import tarfile
import tempfile
from pathlib import Path

from generate_release_manifest import validate_manifest


def stage_binary(manifest_path: Path, archive: Path, tag: str, repo: str, output: Path) -> None:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    validate_manifest(manifest, tag, repo)
    target = "x86_64-unknown-linux-gnu"
    asset = manifest["assets"][target]
    if archive.name != asset["archive"] or archive.stat().st_size != asset["size"]:
        raise ValueError("release archive name or size mismatch")
    with archive.open("rb") as source:
        checksum = hashlib.file_digest(source, "sha256").hexdigest()
    if checksum != asset["sha256"]:
        raise ValueError("release archive checksum mismatch")
    directory = f"vera-{target}"
    expected = f"{directory}/vera"
    with tarfile.open(archive, "r:gz") as bundle:
        members = bundle.getmembers()
        binaries = [member for member in members if member.name == expected and member.isfile()]
        if len(binaries) != 1 or binaries[0].size <= 0 or any(
            not (member.name.rstrip("/") == directory and member.isdir())
            and member is not binaries[0] for member in members
        ):
            raise ValueError("unexpected release archive layout")
        output.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=output.parent, delete=False) as destination:
                temporary = Path(destination.name)
                with bundle.extractfile(binaries[0]) as binary:
                    shutil.copyfileobj(binary, destination)
            temporary.chmod(0o755)
            os.replace(temporary, output)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)


if __name__ == "__main__":
    if len(sys.argv) != 6:
        raise SystemExit("usage: verify_release_archive.py <manifest> <archive> <tag> <repo> <output>")
    stage_binary(Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], sys.argv[4], Path(sys.argv[5]))
