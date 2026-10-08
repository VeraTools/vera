#!/usr/bin/env python3
"""Create a representative subset of semble tasks for faster benchmarking.

Picks repos that cover diverse languages while keeping the task count manageable.
"""

import json
import hashlib
import sys
from pathlib import Path

# Representative subset: 2-3 repos per language family, covering the most popular ones
SUBSET_REPOS = [
    # Python
    "fastapi", "flask", "requests",
    # JavaScript/TypeScript
    "express", "zod", "axios",
    # Rust
    "tokio", "serde", "axum",
    # Go
    "gin", "cobra",
    # Java
    "gson",
    # C/C++
    "curl", "fmtlib",
    # Ruby
    "sinatra",
    # Elixir
    "phoenix",
]
EXPECTED_TASK_COUNT = 320
EXPECTED_TASKS_SHA256 = "60bac08d1e9d71f6d9208f6c15fcc7c354fab85d2eca5a95b6da9b6dc6608190"

def tasks_sha256(tasks):
    payload = json.dumps(
        sorted(tasks, key=lambda task: task["id"]),
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return hashlib.sha256(payload).hexdigest()

def main():
    tasks_dir = Path("eval/tasks/semble")
    out_dir = Path("eval/tasks/semble-subset")
    out_dir.mkdir(parents=True, exist_ok=True)

    total = 0
    subset_tasks = []
    for repo in SUBSET_REPOS:
        src = tasks_dir / f"{repo}.json"
        if not src.exists():
            print(f"  SKIP {repo} (no annotation file)", file=sys.stderr)
            continue
        tasks = json.loads(src.read_text())
        (out_dir / f"{repo}.json").write_text(json.dumps(tasks, indent=2) + "\n")
        total += len(tasks)
        subset_tasks.extend(tasks)
        print(f"  {repo}: {len(tasks)} tasks", file=sys.stderr)

    print(f"\nSubset: {total} tasks across {len(SUBSET_REPOS)} repos", file=sys.stderr)

    subset_hash = tasks_sha256(subset_tasks)
    if total != EXPECTED_TASK_COUNT or subset_hash != EXPECTED_TASKS_SHA256:
        raise SystemExit(
            "Semble subset does not match the pinned v0.5.5 slice "
            f"({total} tasks, sha256:{subset_hash})"
        )

    # Generate subset corpus.toml
    import tomllib
    with open("eval/semble-corpus.toml", "rb") as f:
        corpus = tomllib.load(f)

    subset_repos = [r for r in corpus["repos"] if r["name"] in SUBSET_REPOS]
    lines = [
        '# Semble benchmark subset (auto-generated)',
        '',
        '[corpus]',
        'version = 1',
        'description = "Semble benchmark subset for faster evaluation"',
        'clone_root = ".bench/semble-repos"',
        '',
        '[semble]',
        f'version = "{corpus["semble"]["version"]}"',
        f'commit = "{corpus["semble"]["commit"]}"',
        f'task_count = {total}',
        f'tasks_sha256 = "{subset_hash}"',
        '',
    ]
    for repo in subset_repos:
        lines.append('[[repos]]')
        lines.append(f'name = "{repo["name"]}"')
        lines.append(f'url = "{repo["url"]}"')
        lines.append(f'commit = "{repo["commit"]}"')
        lines.append(f'language = "{repo["language"]}"')
        lines.append('description = "semble benchmark repo"')
        if repo.get("benchmark_root"):
            lines.append(f'benchmark_root = "{repo["benchmark_root"]}"')
        lines.append('')

    Path("eval/semble-subset-corpus.toml").write_text('\n'.join(lines))
    print("Corpus manifest: eval/semble-subset-corpus.toml", file=sys.stderr)


if __name__ == "__main__":
    main()
