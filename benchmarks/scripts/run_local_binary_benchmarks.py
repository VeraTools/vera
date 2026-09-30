#!/usr/bin/env python3
"""
Run the Vera task suite against an arbitrary local binary.

This is meant for evidence-driven retrieval tuning loops where we want to
compare old releases, regression points, and candidate builds under the same
corpus and the same CLI flags.
"""

from __future__ import annotations

import argparse
import json
import math
import shutil
import subprocess
import time
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path
from statistics import median
from typing import Any

from bench_common import (
    binary_version,
    git_sha,
    load_tasks,
    mrr,
    ndcg_at_k,
    recall_at_k,
)


REPO_ROOT = Path(__file__).resolve().parent.parent.parent
CORPUS_DIR = REPO_ROOT / ".bench" / "repos"
RESULTS_DIR = REPO_ROOT / "benchmarks" / "results" / "local-binaries"
METRIC_KEYS = ["recall_at_1", "recall_at_5", "recall_at_10", "mrr", "ndcg"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path, help="Path to Vera binary")
    parser.add_argument("--label", required=True, help="Short run label")
    parser.add_argument(
        "--extra-arg",
        action="append",
        default=[],
        help="Extra CLI arg passed to both index and search, repeatable",
    )
    parser.add_argument("--limit", type=int, default=20, help="Search result limit")
    parser.add_argument(
        "--output",
        type=Path,
        help="Output JSON path (defaults under benchmarks/results/local-binaries)",
    )
    parser.add_argument(
        "--compare",
        action="append",
        default=[],
        type=Path,
        help="Optional prior result JSON to diff against, repeatable",
    )
    return parser.parse_args()



def repo_order(tasks: list[dict[str, Any]]) -> list[str]:
    seen = set()
    ordered = []
    for task in tasks:
        repo = task["repo"]
        if repo not in seen:
            seen.add(repo)
            ordered.append(repo)
    return ordered


def run_json(cmd: list[str], cwd: Path) -> tuple[Any, float]:
    start = time.monotonic()
    proc = subprocess.run(cmd, cwd=cwd, text=True, capture_output=True, timeout=600)
    elapsed = time.monotonic() - start
    if proc.returncode != 0:
        stderr = proc.stderr.strip()[:500]
        raise RuntimeError(f"command failed: {' '.join(cmd)}\n{stderr}")
    stdout = proc.stdout.strip()
    return (json.loads(stdout) if stdout else [], elapsed)









def aggregate_metrics(rows: list[dict[str, Any]]) -> dict[str, float]:
    return {
        key: sum(row["metrics"][key] for row in rows) / len(rows) if rows else 0.0
        for key in METRIC_KEYS
    }


def aggregate_latency(rows: list[dict[str, Any]]) -> dict[str, float]:
    latencies = [row["latency_ms"] for row in rows]
    if not latencies:
        return {"p50_ms": 0.0, "p95_ms": 0.0}
    ordered = sorted(latencies)
    p95_index = max(0, math.ceil(len(ordered) * 0.95) - 1)
    return {
        "p50_ms": median(ordered),
        "p95_ms": ordered[p95_index],
    }


def summarize_per_category(rows: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        grouped[row["category"]].append(row)

    summary = {}
    for category, items in sorted(grouped.items()):
        summary[category] = {
            "count": len(items),
            "metrics": aggregate_metrics(items),
            "latency": aggregate_latency(items),
        }
    return summary


def index_repo(binary: Path, repo_name: str, extra_args: list[str]) -> dict[str, Any]:
    repo_path = CORPUS_DIR / repo_name
    shutil.rmtree(repo_path / ".vera", ignore_errors=True)
    cmd = [str(binary), "--json", "index", str(repo_path), *extra_args]
    summary, elapsed = run_json(cmd, repo_path)
    storage_bytes = 0
    for file_path in (repo_path / ".vera").rglob("*"):
        if file_path.is_file():
            storage_bytes += file_path.stat().st_size
    return {
        "repo": repo_name,
        "elapsed_secs": elapsed,
        "files_parsed": summary.get("files_parsed"),
        "chunks_created": summary.get("chunks_created"),
        "storage_bytes": storage_bytes,
    }


def run_task(
    binary: Path,
    task: dict[str, Any],
    extra_args: list[str],
    limit: int,
) -> dict[str, Any]:
    repo_path = CORPUS_DIR / task["repo"]
    cmd = [str(binary), "--json", "search", task["query"], "--limit", str(limit), *extra_args]
    results, elapsed = run_json(cmd, repo_path)
    metrics = {
        "recall_at_1": recall_at_k(results, task["ground_truth"], 1),
        "recall_at_5": recall_at_k(results, task["ground_truth"], 5),
        "recall_at_10": recall_at_k(results, task["ground_truth"], 10),
        "mrr": mrr(results, task["ground_truth"]),
        "ndcg": ndcg_at_k(results, task["ground_truth"], 10),
    }
    return {
        "task_id": task["id"],
        "category": task["category"],
        "repo": task["repo"],
        "query": task["query"],
        "latency_ms": elapsed * 1000.0,
        "metrics": metrics,
        "results": results,
    }


def print_summary(result: dict[str, Any]) -> None:
    agg = result["aggregate"]["metrics"]
    latency = result["aggregate"]["latency"]
    print(
        f"{result['label']}: "
        f"R@1 {agg['recall_at_1']:.4f} | "
        f"R@5 {agg['recall_at_5']:.4f} | "
        f"R@10 {agg['recall_at_10']:.4f} | "
        f"MRR {agg['mrr']:.4f} | "
        f"nDCG {agg['ndcg']:.4f} | "
        f"p50 {latency['p50_ms']:.0f}ms | "
        f"p95 {latency['p95_ms']:.0f}ms"
    )


def print_comparison(result: dict[str, Any], compare_path: Path) -> None:
    baseline = json.loads(compare_path.read_text())
    current = result["aggregate"]["metrics"]
    previous = baseline["aggregate"]["metrics"]
    deltas = []
    for key, label in [
        ("recall_at_1", "R@1"),
        ("recall_at_5", "R@5"),
        ("recall_at_10", "R@10"),
        ("mrr", "MRR"),
        ("ndcg", "nDCG"),
    ]:
        delta = current[key] - previous[key]
        deltas.append(f"{label} {delta:+.4f}")
    print(f"vs {baseline['label']}: {' | '.join(deltas)}")


def main() -> None:
    args = parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        raise SystemExit(f"binary not found: {binary}")

    tasks = load_tasks()
    output_path = args.output or RESULTS_DIR / f"{args.label}.json"
    output_path.parent.mkdir(parents=True, exist_ok=True)

    print(f"Benchmarking {binary}")
    print(f"Label: {args.label}")
    print(f"Extra args: {args.extra_arg or 'none'}")

    indexing = []
    for repo_name in repo_order(tasks):
        stat = index_repo(binary, repo_name, args.extra_arg)
        indexing.append(stat)
        print(
            f"INDEX {repo_name}: {stat['files_parsed']} files, "
            f"{stat['chunks_created']} chunks, {stat['elapsed_secs']:.1f}s"
        )

    per_task = []
    for task in tasks:
        row = run_task(binary, task, args.extra_arg, args.limit)
        per_task.append(row)
        print(
            f"TASK {row['task_id']}: "
            f"R1={row['metrics']['recall_at_1']:.2f} "
            f"R5={row['metrics']['recall_at_5']:.2f} "
            f"MRR={row['metrics']['mrr']:.3f}"
        )

    result = {
        "label": args.label,
        "binary": str(binary),
        "binary_version": binary_version(binary),
        "git_sha": git_sha(),
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "extra_args": args.extra_arg,
        "limit": args.limit,
        "indexing": indexing,
        "per_task": per_task,
        "per_category": summarize_per_category(per_task),
        "aggregate": {
            "metrics": aggregate_metrics(per_task),
            "latency": aggregate_latency(per_task),
        },
    }

    output_path.write_text(json.dumps(result, indent=2))
    print_summary(result)
    for compare_path in args.compare:
        print_comparison(result, compare_path)
    print(f"Wrote {output_path}")


if __name__ == "__main__":
    main()
