#!/usr/bin/env python3
"""Preserve versioned images and promote only the newest published stable release."""

import json
import os
import re
import subprocess
import sys

from release_versions import newest_stable, published_releases



def image_digest(image: str) -> str | None:
    result = subprocess.run(
        ["docker", "buildx", "imagetools", "inspect", image, "--format", "{{json .Manifest.Digest}}"],
        capture_output=True, text=True, timeout=120,
    )
    if result.returncode:
        if any(message in result.stderr.lower() for message in (
            ": not found", "manifest unknown", "no such manifest"
        )):
            return None
        raise RuntimeError(result.stderr.strip())
    digest = json.loads(result.stdout)
    if not isinstance(digest, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise ValueError("registry returned an invalid image digest")
    return digest


def main() -> None:
    mode, repo, tag, image, variant = sys.argv[1:]
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[a-zA-Z0-9.]+)?", tag):
        raise ValueError("invalid release tag")
    if variant not in ("cpu", "cuda", "rocm", "openvino"):
        raise ValueError("unsupported Docker variant")
    versioned = f"{image}:{tag[1:]}-{variant}"
    digest = image_digest(versioned)
    if mode == "check":
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write(f"build={str(digest is None).lower()}\nimage={versioned}\n")
        return
    if mode != "promote" or digest is None:
        raise ValueError("versioned image must exist before promotion")
    latest = newest_stable(published_releases(repo))
    if tag == latest:
        subprocess.run([
            "docker", "buildx", "imagetools", "create", "--prefer-index=false",
            "--tag", f"{image}:{variant}", f"{image}@{digest}",
        ], check=True, timeout=180)
    else:
        print(f"Preserving mutable {variant} tag: newest stable release is {latest}")


if __name__ == "__main__":
    main()
