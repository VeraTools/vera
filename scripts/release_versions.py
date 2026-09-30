#!/usr/bin/env python3
"""Select stable release versions numerically across publication channels."""

import json
import re
import subprocess
import sys


def newest_stable(releases: list[dict]) -> str | None:
    versions = {}
    for release in releases:
        tag = release.get("tag_name", "")
        match = re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", tag)
        if match and not release.get("draft") and not release.get("prerelease"):
            versions[tuple(map(int, match.groups()))] = tag
    return versions[max(versions)] if versions else None


def published_releases(repo: str) -> list[dict]:
    pages = json.loads(subprocess.check_output(
        ["gh", "api", "--paginate", "--slurp", f"repos/{repo}/releases?per_page=100"],
        text=True, timeout=120,
    ))
    return [release for page in pages for release in page]


if __name__ == "__main__":
    releases = published_releases(sys.argv[1])
    if sys.argv[3:] == ["--candidate"]:
        releases.append({"tag_name": sys.argv[2]})
    print(str(sys.argv[2] == newest_stable(releases)).lower())
