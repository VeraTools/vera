#!/usr/bin/env python3
"""Run semble on Vera's eval subset and produce comparable metrics.

Usage:
    /tmp/semble-env/bin/python eval/scripts/run_semble_benchmark.py
"""

import json
import math
import sys
import time
import tomllib
from pathlib import Path

# Add semble to path
sys.path.insert(0, "/tmp/semble-env/lib/python3.14/site-packages")

from semble import SembleIndex  # noqa: E402

SUBSET_REPOS = [
    "fastapi", "flask", "requests", "express", "zod", "axios",
    "tokio", "serde", "axum", "gin", "cobra", "gson",
    "curl", "fmtlib", "sinatra", "phoenix",
]
CLONE_ROOT = Path(".bench/semble-repos")
TASKS_DIR = Path("eval/tasks/semble-subset")
CORPUS_PATH = Path("eval/semble-subset-corpus.toml")
K = 10


def load_tasks():
    tasks = []
    for repo in SUBSET_REPOS:
        path = TASKS_DIR / f"{repo}.json"
        if path.exists():
            tasks.extend(json.loads(path.read_text()))
    return tasks


def load_benchmark_roots():
    with CORPUS_PATH.open("rb") as f:
        manifest = tomllib.load(f)
    return {
        repo["name"]: repo["benchmark_root"]
        for repo in manifest["repos"]
        if repo.get("benchmark_root")
    }


def is_match(result_path, gt):
    """Check if a semble result file path matches a ground truth entry."""
    gt_path = gt["file_path"]
    # Semble returns paths relative to benchmark_root, Vera ground truth is
    # relative to repo root. We match if either is a suffix of the other.
    return result_path == gt_path or result_path.endswith("/" + gt_path) or gt_path.endswith("/" + result_path)


def percentile(sorted_values, p):
    """Linear interpolation between nearest ranks, matching eval/src/metrics.rs."""
    if not sorted_values:
        return 0.0
    if len(sorted_values) == 1:
        return sorted_values[0]
    rank = p / 100.0 * (len(sorted_values) - 1)
    lower = int(rank)
    upper = min(lower + 1, len(sorted_values) - 1)
    frac = rank - lower
    return sorted_values[lower] * (1 - frac) + sorted_values[upper] * frac


def recall_at_k(results, ground_truth, k):
    if not ground_truth:
        return 0.0
    top_k = results[:k]
    found = sum(1 for gt in ground_truth if any(is_match(r, gt) for r in top_k))
    return found / len(ground_truth)


def mrr(results, ground_truth):
    for i, r in enumerate(results):
        if any(is_match(r, gt) for gt in ground_truth):
            return 1.0 / (i + 1)
    return 0.0


def ndcg_at_k(results, ground_truth, k):
    top_k = results[:k]
    used = [False] * len(ground_truth)

    dcg = 0.0
    for i, r in enumerate(top_k):
        best_rel = 0
        best_idx = -1
        for j, gt in enumerate(ground_truth):
            if not used[j] and is_match(r, gt):
                if gt.get("relevance", 1) > best_rel:
                    best_rel = gt["relevance"]
                    best_idx = j
        if best_idx >= 0:
            used[best_idx] = True
            dcg += best_rel / math.log2(i + 2)

    ideal_rels = sorted([gt.get("relevance", 1) for gt in ground_truth], reverse=True)
    idcg = sum(rel / math.log2(i + 2) for i, rel in enumerate(ideal_rels[:k]))
    return dcg / idcg if idcg > 0 else 0.0


def main():
    tasks = load_tasks()
    print(f"Loaded {len(tasks)} tasks", file=sys.stderr)
    benchmark_roots = load_benchmark_roots()

    missing_repos = [
        repo_name
        for repo_name in SUBSET_REPOS
        if not (CLONE_ROOT / repo_name).is_dir()
    ]
    if missing_repos:
        print(
            "Missing subset repositories; run eval/setup-semble-corpus.sh: "
            + ", ".join(missing_repos),
            file=sys.stderr,
        )
        return 1

    # Group tasks by repo
    by_repo = {}
    for task in tasks:
        by_repo.setdefault(task["repo"], []).append(task)

    all_metrics = []
    per_category = {}
    total_index_time = 0.0
    total_search_time = 0.0

    for repo_name in SUBSET_REPOS:
        repo_tasks = by_repo.get(repo_name, [])
        if not repo_tasks:
            continue

        repo_path = CLONE_ROOT / repo_name
        benchmark_root = benchmark_roots.get(repo_name)
        index_path = repo_path / benchmark_root if benchmark_root else repo_path

        print(f"  {repo_name}: indexing...", file=sys.stderr, end=" ", flush=True)
        t0 = time.monotonic()
        se = SembleIndex.from_path(str(index_path))
        index_time = time.monotonic() - t0
        total_index_time += index_time
        print(f"{index_time:.1f}s, searching {len(repo_tasks)} queries...", file=sys.stderr, end=" ", flush=True)

        for task in repo_tasks:
            t1 = time.monotonic()
            try:
                results = se.search(task["query"], top_k=K)
                result_paths = [r.chunk.file_path for r in results]
            except Exception as e:
                print(f"\n    WARNING: {task['id']} failed: {e}", file=sys.stderr)
                result_paths = []
            search_ms = (time.monotonic() - t1) * 1000
            total_search_time += search_ms

            gt = task["ground_truth"]
            m = {
                "task_id": task["id"],
                "category": task["category"],
                "recall_at_1": recall_at_k(result_paths, gt, 1),
                "recall_at_5": recall_at_k(result_paths, gt, 5),
                "recall_at_10": recall_at_k(result_paths, gt, K),
                "mrr": mrr(result_paths, gt),
                "ndcg": ndcg_at_k(result_paths, gt, K),
                "latency_ms": search_ms,
            }
            all_metrics.append(m)
            cat = task["category"]
            per_category.setdefault(cat, []).append(m)

        print("done", file=sys.stderr)

    # Aggregate
    n = len(all_metrics)
    if n == 0:
        print("No results", file=sys.stderr)
        return 1

    def avg(key):
        return sum(m[key] for m in all_metrics) / n

    print(f"\n=== Semble on Semble Subset ({n} tasks, {len(SUBSET_REPOS)} repos) ===")
    print(f"R@1:  {avg('recall_at_1'):.4f}")
    print(f"R@5:  {avg('recall_at_5'):.4f}")
    print(f"R@10: {avg('recall_at_10'):.4f}")
    print(f"MRR:  {avg('mrr'):.4f}")
    print(f"nDCG: {avg('ndcg'):.4f}")

    latencies = sorted(m["latency_ms"] for m in all_metrics)
    print(f"Latency p50: {percentile(latencies, 50):.1f}ms")
    print(f"Latency p95: {percentile(latencies, 95):.1f}ms")
    print(f"Index time: {total_index_time:.1f}s")

    print()
    for cat, ms in sorted(per_category.items()):
        nc = len(ms)
        cr1 = sum(m["recall_at_1"] for m in ms) / nc
        cr10 = sum(m["recall_at_10"] for m in ms) / nc
        cndcg = sum(m["ndcg"] for m in ms) / nc
        print(f"{cat:15s} R@1={cr1:.4f} R@10={cr10:.4f} nDCG={cndcg:.4f} ({nc} tasks)")

    # Save JSON
    report = {
        "tool": "semble",
        "tasks": n,
        "aggregate": {
            "recall_at_1": avg("recall_at_1"),
            "recall_at_5": avg("recall_at_5"),
            "recall_at_10": avg("recall_at_10"),
            "mrr": avg("mrr"),
            "ndcg": avg("ndcg"),
            "latency_p50_ms": percentile(latencies, 50),
            "latency_p95_ms": percentile(latencies, 95),
            "index_time_secs": total_index_time,
        },
        "per_category": {
            cat: {
                "recall_at_1": sum(m["recall_at_1"] for m in ms) / len(ms),
                "recall_at_10": sum(m["recall_at_10"] for m in ms) / len(ms),
                "ndcg": sum(m["ndcg"] for m in ms) / len(ms),
                "task_count": len(ms),
            }
            for cat, ms in per_category.items()
        },
        "per_task": all_metrics,
    }
    out = Path("benchmarks/semble-on-subset.json")
    out.write_text(json.dumps(report, indent=2))
    print(f"\nJSON saved to {out}", file=sys.stderr)


if __name__ == "__main__":
    raise SystemExit(main())
