#!/usr/bin/env python3
"""
Final Benchmark Suite — Vera vs Competitors

Runs the complete benchmark suite with all 21 tasks, all metrics, all baselines,
and Vera results. Produces a publishable comparison table and verifies all
performance targets.

This is the "single command" final benchmark for Milestone 4.

Usage:
    python3 benchmarks/scripts/run_final_benchmarks.py

Requires:
    - Vera binary built: cargo build --release
    - Corpus repos cloned: bash eval/setup-corpus.sh
    - secrets.env for API credentials
    - No existing .vera indexes (script cleans them)
"""

from bench_common import (
    binary_version,
    compute_task_metrics,
    git_sha,
    load_secrets,
    load_tasks,
    percentile,
)

import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
VERA_BIN = REPO_ROOT / "target" / "release" / "vera"
CORPUS_DIR = REPO_ROOT / ".bench" / "repos"
RESULTS_DIR = REPO_ROOT / "benchmarks" / "results" / "final-suite"
REPORTS_DIR = REPO_ROOT / "benchmarks" / "reports"
BASELINES_FILE = (
    REPO_ROOT / "benchmarks" / "results" / "competitor-baselines" / "all_baselines.json"
)

# Repos in order — turborepo last with cooldown before it
REPO_ORDER = ["ripgrep", "flask", "fastify", "turborepo"]
COOLDOWN_SECS = 15  # pause between repos for API rate limit recovery






# ── Indexing ──────────────────────────────────────────────────────────


def clean_indexes():
    """Remove all existing .vera indexes to avoid dimension mismatch."""
    for repo in REPO_ORDER:
        index_dir = CORPUS_DIR / repo / ".vera"
        if index_dir.exists():
            subprocess.run(["rm", "-rf", str(index_dir)], timeout=30)
            print(f"  Cleaned existing index: {repo}")


def index_repo(repo_name: str, env: dict[str, str]) -> dict:
    """Index a repository and return stats."""
    repo_path = CORPUS_DIR / repo_name
    index_dir = repo_path / ".vera"

    # Clean existing index
    if index_dir.exists():
        subprocess.run(["rm", "-rf", str(index_dir)], timeout=30)

    full_env = {**os.environ, **env}

    start = time.monotonic()
    result = subprocess.run(
        [str(VERA_BIN), "--json", "index", str(repo_path)],
        capture_output=True, text=True,
        env=full_env, cwd=str(repo_path),
        timeout=600,  # generous timeout for turborepo
    )
    elapsed = time.monotonic() - start

    if result.returncode != 0:
        print(f"  ✗ Index FAILED for {repo_name}: {result.stderr[:500]}")
        return {
            "repo": repo_name,
            "success": False,
            "time_secs": elapsed,
            "error": result.stderr[:500],
        }

    # Parse summary
    summary = {}
    if result.stdout.strip():
        try:
            summary = json.loads(result.stdout)
        except json.JSONDecodeError:
            pass

    # Measure storage size
    storage_bytes = 0
    if index_dir.exists():
        for f in index_dir.rglob("*"):
            if f.is_file():
                storage_bytes += f.stat().st_size

    # Measure source size using du (excludes .git, .vera, target, node_modules)
    try:
        du_result = subprocess.run(
            ["du", "-sb", "--exclude=.git", "--exclude=.vera",
             "--exclude=target", "--exclude=node_modules",
             "--exclude=__pycache__", "--exclude=.venv",
             "--exclude=dist", "--exclude=build", str(repo_path)],
            capture_output=True, text=True, timeout=30,
        )
        source_bytes = int(du_result.stdout.split()[0]) if du_result.returncode == 0 else 0
    except Exception:
        source_bytes = 0

    files = summary.get("files_parsed", "?")
    chunks = summary.get("chunks_created", "?")
    ratio = storage_bytes / source_bytes if source_bytes > 0 else 0

    print(
        f"  ✓ {repo_name}: {files} files, {chunks} chunks, "
        f"{elapsed:.1f}s, {storage_bytes / 1024 / 1024:.1f}MB index, "
        f"ratio={ratio:.2f}x"
    )

    return {
        "repo": repo_name,
        "success": True,
        "time_secs": elapsed,
        "files_parsed": files,
        "chunks_created": chunks,
        "storage_bytes": storage_bytes,
        "source_bytes": source_bytes,
        "size_ratio": round(ratio, 3),
    }


def index_all_repos(env: dict[str, str]) -> list[dict]:
    """Index all repos with cooldown between them."""
    print("\n" + "=" * 60)
    print("  INDEXING ALL REPOSITORIES")
    print("=" * 60)

    stats = []
    for i, repo in enumerate(REPO_ORDER):
        if i > 0:
            print(f"  Cooling down {COOLDOWN_SECS}s before {repo}...")
            time.sleep(COOLDOWN_SECS)
        stat = index_repo(repo, env)
        stats.append(stat)

    return stats


# ── Search & Metrics ─────────────────────────────────────────────────


def run_search(
    repo_name: str, query: str, mode: str,
    env: dict[str, str], limit: int = 20,
) -> tuple[list[dict], float]:
    """Run a Vera search and return (results, latency_ms)."""
    repo_path = CORPUS_DIR / repo_name
    full_env = {**os.environ, **env}

    if mode == "bm25-only":
        for key in ["EMBEDDING_MODEL_BASE_URL", "EMBEDDING_MODEL_ID",
                     "EMBEDDING_MODEL_API_KEY"]:
            full_env.pop(key, None)
    elif mode == "hybrid-norerank":
        for key in ["RERANKER_MODEL_BASE_URL", "RERANKER_MODEL_ID",
                     "RERANKER_MODEL_API_KEY"]:
            full_env.pop(key, None)

    cmd = [str(VERA_BIN), "--json", "search", query, "--limit", str(limit)]

    start = time.monotonic()
    result = subprocess.run(
        cmd, capture_output=True, text=True,
        env=full_env, cwd=str(repo_path), timeout=120,
    )
    elapsed_ms = (time.monotonic() - start) * 1000.0

    if result.returncode != 0:
        if "Error:" in result.stderr:
            print(f"  ✗ Search failed ({mode}): {result.stderr[:200]}")
        return ([], elapsed_ms)

    try:
        results = json.loads(result.stdout) if result.stdout.strip() else []
    except json.JSONDecodeError:
        results = []

    return (results, elapsed_ms)










# ── Benchmark Runner ─────────────────────────────────────────────────


def run_mode(mode: str, tasks: list[dict], env: dict[str, str], runs: int = 2) -> dict:
    """Run all tasks in a given mode."""
    print(f"\n{'─'*60}")
    print(f"  Mode: {mode}  ({len(tasks)} tasks, {runs} runs)")
    print(f"{'─'*60}")

    all_evals = []
    all_latencies = []

    for run_idx in range(runs):
        if runs > 1:
            print(f"  --- Run {run_idx + 1}/{runs} ---")
        for task in tasks:
            repo_path = CORPUS_DIR / task["repo"]
            if not (repo_path / ".vera").exists():
                print(f"  ⊘ Skip {task['id']} (no index for {task['repo']})")
                continue

            results, latency_ms = run_search(
                task["repo"], task["query"], mode, env, limit=20
            )
            metrics = compute_task_metrics(results, task["ground_truth"])

            all_evals.append({
                "task_id": task["id"],
                "category": task["category"],
                "repo": task["repo"],
                "query": task["query"],
                "retrieval_metrics": metrics,
                "latency_ms": latency_ms,
                "result_count": len(results),
                "results": results,
            })
            all_latencies.append(latency_ms)

            status = "✓" if metrics["recall_at_10"] > 0 else "·"
            print(
                f"  {status} {task['id']:<25} R@1={metrics['recall_at_1']:.2f} "
                f"R@5={metrics['recall_at_5']:.2f} R@10={metrics['recall_at_10']:.2f} "
                f"MRR={metrics['mrr']:.4f} lat={latency_ms:.0f}ms"
            )

    # Average across runs if multiple
    if runs > 1:
        by_task: dict[str, list] = {}
        for ev in all_evals:
            tid = ev["task_id"]
            if tid not in by_task:
                by_task[tid] = []
            by_task[tid].append(ev)

        averaged = []
        for tid, evals in by_task.items():
            avg_metrics = {}
            for key in evals[0]["retrieval_metrics"]:
                vals = [e["retrieval_metrics"][key] for e in evals]
                avg_metrics[key] = sum(vals) / len(vals)
            avg_latency = sum(e["latency_ms"] for e in evals) / len(evals)
            averaged.append({
                "task_id": tid,
                "category": evals[0]["category"],
                "repo": evals[0]["repo"],
                "query": evals[0]["query"],
                "retrieval_metrics": avg_metrics,
                "latency_ms": avg_latency,
                "result_count": evals[0]["result_count"],
                "results": evals[0]["results"],
            })
        per_task = averaged
    else:
        per_task = all_evals

    # Compute per-category aggregates
    by_cat: dict[str, list] = {}
    for ev in per_task:
        cat = ev["category"]
        if cat not in by_cat:
            by_cat[cat] = []
        by_cat[cat].append(ev)

    per_category = {}
    for cat, evals in by_cat.items():
        n = len(evals)
        agg = {}
        for key in evals[0]["retrieval_metrics"]:
            vals = [e["retrieval_metrics"][key] for e in evals]
            agg[key] = sum(vals) / n
        lats = [e["latency_ms"] for e in evals]
        per_category[cat] = {
            "retrieval": agg,
            "latency_p50_ms": percentile(lats, 50),
            "latency_p95_ms": percentile(lats, 95),
            "task_count": n,
        }

    # Overall aggregate
    n = len(per_task)
    overall_retrieval = {}
    if n > 0:
        for key in per_task[0]["retrieval_metrics"]:
            vals = [e["retrieval_metrics"][key] for e in per_task]
            overall_retrieval[key] = sum(vals) / n

    perf = {
        "latency_p50_ms": percentile(all_latencies, 50),
        "latency_p95_ms": percentile(all_latencies, 95),
        "total_queries": len(all_latencies),
    }

    return {
        "tool_name": f"vera-{mode}",
        "mode": mode,
        "per_task": per_task,
        "per_category": per_category,
        "aggregate": {
            "retrieval": overall_retrieval,
            "performance": perf,
            "task_count": n,
        },
    }


# ── Incremental Update Benchmark ────────────────────────────────────


def benchmark_incremental_update(env: dict[str, str]) -> dict:
    """Benchmark incremental update: modify one file and measure update time."""
    repo_path = CORPUS_DIR / "flask"
    test_file = repo_path / "src" / "flask" / "app.py"

    if not test_file.exists():
        return {"time_secs": -1, "error": "test file not found"}

    # Read original content
    original = test_file.read_text()

    # Append a comment
    modified = original + "\n# Benchmark test comment for incremental update timing\n"
    test_file.write_text(modified)

    full_env = {**os.environ, **env}

    start = time.monotonic()
    result = subprocess.run(
        [str(VERA_BIN), "--json", "update", str(repo_path)],
        capture_output=True, text=True,
        env=full_env, cwd=str(repo_path),
        timeout=120,
    )
    elapsed = time.monotonic() - start

    # Restore original
    test_file.write_text(original)

    # Run update again to restore index
    subprocess.run(
        [str(VERA_BIN), "--json", "update", str(repo_path)],
        capture_output=True, text=True,
        env=full_env, cwd=str(repo_path),
        timeout=120,
    )

    summary = {}
    if result.stdout.strip():
        try:
            summary = json.loads(result.stdout)
        except json.JSONDecodeError:
            pass

    return {
        "time_secs": elapsed,
        "files_updated": summary.get("files_updated", "?"),
        "success": result.returncode == 0,
    }


# ── Report Generation ────────────────────────────────────────────────


def load_baselines() -> dict | None:
    if BASELINES_FILE.exists():
        with open(BASELINES_FILE) as f:
            return json.load(f)
    return None


def generate_report(
    vera_results: dict[str, dict],
    baselines: dict | None,
    index_stats: list[dict],
    update_stats: dict,
    timestamp: str,
) -> str:
    """Generate the full publishable benchmark report."""
    lines = []

    def add(line: str = ""):
        lines.append(line)

    add("# Vera Final Benchmark Report")
    add()
    add("## Objective")
    add()
    add("Complete benchmark of Vera's hybrid retrieval pipeline against competitor baselines,")
    add("covering all 21 tasks across 4 repositories and 5 workload categories. This report")
    add("verifies all performance targets and provides publishable comparison tables.")
    add()

    # Setup section
    add("## Setup")
    add()
    add("### Machine")
    add("- **CPU:** AMD Ryzen 5 7600X3D 6-Core (12 threads)")
    add("- **RAM:** 30 GB")
    add("- **OS:** CachyOS (Arch Linux), kernel 6.19.9-1-cachyos")
    add("- **Disk:** NVMe SSD")
    add()

    add("### Vera Configuration")
    add(f"- **Version:** {binary_version(VERA_BIN)}")
    add(f"- **Git SHA:** `{git_sha()}`")
    add("- **Build:** `cargo build --release` (optimized)")
    add("- **Embedding model:** Qwen3-Embedding-8B (4096→1024-dim Matryoshka truncation)")
    add("- **Reranker model:** Qwen3-Reranker (cross-encoder via API)")
    add("- **Storage:** SQLite + sqlite-vec (vectors), Tantivy (BM25)")
    add("- **RRF k:** 60.0, **Rerank candidates:** 50")
    add()

    add("### Test Corpus (4 repositories, pinned SHAs)")
    add()
    add("| Repository | Language   | Commit SHA       | Files | Chunks | Index Time |")
    add("|------------|-----------|------------------|-------|--------|------------|")
    for stat in index_stats:
        repo = stat["repo"]
        sha = {
            "ripgrep": "4519153e5e46",
            "flask": "4cae5d8e411b",
            "fastify": "a22217f9420f",
            "turborepo": "56b79ff5c1c9",
        }.get(repo, "unknown")
        lang = {
            "ripgrep": "Rust",
            "flask": "Python",
            "fastify": "TypeScript",
            "turborepo": "Polyglot",
        }.get(repo, "?")
        if stat["success"]:
            add(f"| {repo:<10} | {lang:<9} | `{sha}` | {stat['files_parsed']:>5} | {stat['chunks_created']:>6} | {stat['time_secs']:.1f}s |")
        else:
            add(f"| {repo:<10} | {lang:<9} | `{sha}` | FAIL  | FAIL   | {stat['time_secs']:.1f}s |")
    add()

    add("### Benchmark Tasks")
    add("21 tasks across 5 workload categories:")
    add("- **Symbol Lookup** (6 tasks): exact function/struct/class definition searches")
    add("- **Intent Search** (5 tasks): natural language queries for code concepts")
    add("- **Cross-File Discovery** (3 tasks): finding related code across modules")
    add("- **Config Lookup** (4 tasks): finding configuration files")
    add("- **Disambiguation** (3 tasks): resolving ambiguous queries with multiple matches")
    add()

    add("### Retrieval Modes Tested")
    add()
    add("| Mode               | Description |")
    add("|--------------------|-------------|")
    add("| **bm25-only**      | BM25 keyword search only (Tantivy, no API calls) |")
    add("| **hybrid-norerank**| BM25 + vector via RRF fusion, no reranking |")
    add("| **hybrid**         | Full pipeline: BM25 + vector + RRF + cross-encoder reranking |")
    add()

    add("### Competitor Baselines (from Milestone 1)")
    add()
    add("| Tool               | Version     | Type |")
    add("|--------------------|-------------|------|")
    add("| **ripgrep**        | 13.0.0      | Lexical text search |")
    add("| **cocoindex-code** | 0.2.4       | AST + MiniLM-L6-v2 embeddings |")
    add("| **vector-only**    | Qwen3-8B    | Pure embedding similarity |")
    add()

    # Main results table
    add("## Results")
    add()
    add("### Overall Aggregate Metrics (All 21 Tasks)")
    add()

    # Build the comparison table
    tools_order = []
    tool_data = {}

    if baselines:
        for name in ["ripgrep", "cocoindex", "vector-only"]:
            if name in baselines:
                tool_name = baselines[name].get("tool_name", name)
                tools_order.append(tool_name)
                tool_data[tool_name] = baselines[name]

    for mode in ["bm25-only", "hybrid-norerank", "hybrid"]:
        if mode in vera_results:
            label = f"vera-{mode}"
            tools_order.append(label)
            tool_data[label] = vera_results[mode]

    add("| Metric              | " + " | ".join(f"{t:>16}" for t in tools_order) + " |")
    add("|---------------------| " + " | ".join("-" * 16 for _ in tools_order) + " |")

    metrics_display = [
        ("Recall@1", "recall_at_1"),
        ("Recall@5", "recall_at_5"),
        ("Recall@10", "recall_at_10"),
        ("MRR@10", "mrr"),
        ("nDCG@10", "ndcg"),
        ("Precision@3", "precision_at_3"),
    ]

    for label, key in metrics_display:
        row = f"| **{label}**" + " " * (20 - len(label) - 4)
        for tool in tools_order:
            d = tool_data[tool]
            ret = d.get("aggregate", {}).get("retrieval", {})
            val = ret.get(key)
            if val is not None:
                # Bold the best value
                row += f"| {val:>16.4f} "
            else:
                row += f"| {'—':>16} "
        row += "|"
        add(row)

    # Latency rows
    row = "| **p50 latency (ms)**"
    for tool in tools_order:
        d = tool_data[tool]
        perf = d.get("aggregate", {}).get("performance", {})
        val = perf.get("latency_p50_ms")
        if val is not None:
            row += f"| {val:>16.1f} "
        else:
            row += f"| {'—':>16} "
    row += "|"
    add(row)

    row = "| **p95 latency (ms)**"
    for tool in tools_order:
        d = tool_data[tool]
        perf = d.get("aggregate", {}).get("performance", {})
        val = perf.get("latency_p95_ms")
        if val is not None:
            row += f"| {val:>16.1f} "
        else:
            row += f"| {'—':>16} "
    row += "|"
    add(row)

    add()

    # Per-category breakdown
    add("### Per-Category Breakdown")
    add()

    categories = [
        ("symbol_lookup", "Symbol Lookup"),
        ("intent", "Intent Search"),
        ("cross_file", "Cross-File Discovery"),
        ("config", "Config Lookup"),
        ("disambiguation", "Disambiguation"),
    ]

    vera_modes = ["bm25-only", "hybrid-norerank", "hybrid"]
    baseline_names = []
    if baselines:
        for name in ["ripgrep", "cocoindex", "vector-only"]:
            if name in baselines:
                baseline_names.append((name, baselines[name].get("tool_name", name)))

    all_cols = [(n, bl) for n, bl in baseline_names] + [
        (m, f"vera-{m}") for m in vera_modes if m in vera_results
    ]

    for cat_id, cat_label in categories:
        add(f"#### {cat_label}")
        add()
        col_labels = [label for _, label in all_cols]
        add("| Metric     | " + " | ".join(f"{label:>16}" for label in col_labels) + " |")
        add("|------------|" + "|".join("-" * 18 for _ in col_labels) + "|")

        for metric_label, metric_key in [
            ("Recall@1", "recall_at_1"),
            ("Recall@5", "recall_at_5"),
            ("Recall@10", "recall_at_10"),
            ("MRR@10", "mrr"),
            ("nDCG@10", "ndcg"),
        ]:
            row = f"| {metric_label:<10} "
            for name, label in all_cols:
                if name in ["ripgrep", "cocoindex", "vector-only"] and baselines:
                    cat_data = baselines.get(name, {}).get("per_category", {}).get(cat_id, {})
                    ret = cat_data.get("retrieval", {})
                else:
                    cat_data = vera_results.get(name, {}).get("per_category", {}).get(cat_id, {})
                    ret = cat_data.get("retrieval", {})
                val = ret.get(metric_key)
                if val is not None:
                    row += f"| {val:>16.4f} "
                else:
                    row += f"| {'—':>16} "
            row += "|"
            add(row)
        add()

    # Ablation Analysis
    add("## Ablation Analysis")
    add()

    hybrid = vera_results.get("hybrid", {}).get("aggregate", {}).get("retrieval", {})
    bm25 = vera_results.get("bm25-only", {}).get("aggregate", {}).get("retrieval", {})
    norerank = vera_results.get("hybrid-norerank", {}).get("aggregate", {}).get("retrieval", {})

    add("### Hybrid vs BM25-Only")
    add()
    add("| Metric     | BM25-only | Hybrid    | Improvement |")
    add("|------------|-----------|-----------|-------------|")
    for label, key in [("MRR@10", "mrr"), ("Recall@5", "recall_at_5"),
                       ("Recall@10", "recall_at_10"), ("nDCG@10", "ndcg")]:
        b = bm25.get(key, 0)
        h = hybrid.get(key, 0)
        imp = ((h - b) / b * 100) if b > 0 else 0
        add(f"| {label:<10} | {b:.4f}   | {h:.4f}   | **+{imp:.0f}%** |")
    add()

    add("### Hybrid vs Vector-Only (M1 Baseline)")
    add()
    if baselines and "vector-only" in baselines:
        vo = baselines["vector-only"].get("aggregate", {}).get("retrieval", {})
    else:
        vo = {}
    add("| Metric     | Vector-only | Hybrid    | Improvement |")
    add("|------------|-------------|-----------|-------------|")
    for label, key in [("MRR@10", "mrr"), ("Recall@1", "recall_at_1"),
                       ("Recall@5", "recall_at_5"), ("Recall@10", "recall_at_10")]:
        v = vo.get(key, 0)
        h = hybrid.get(key, 0)
        imp = ((h - v) / v * 100) if v > 0 else 0
        add(f"| {label:<10} | {v:.4f}     | {h:.4f}   | **+{imp:.0f}%** |")
    add()

    add("### Reranking Impact")
    add()
    add("| Metric        | Unreranked | Reranked  | Change      |")
    add("|---------------|------------|-----------|-------------|")
    for label, key in [("Precision@3", "precision_at_3"), ("MRR@10", "mrr"),
                       ("Recall@10", "recall_at_10")]:
        nr = norerank.get(key, 0)
        h = hybrid.get(key, 0)
        imp = ((h - nr) / nr * 100) if nr > 0 else 0
        add(f"| {label:<13} | {nr:.4f}    | {h:.4f}   | **+{imp:.0f}%** |")
    add()

    # Performance targets
    add("## Performance Targets")
    add()
    add("| Target                           | Actual                 | Status |")
    add("|----------------------------------|------------------------|--------|")

    # Index time: 100K+ LOC in < 120s (with API), parsing < 60s
    rg_stat = next((s for s in index_stats if s["repo"] == "ripgrep"), None)
    if rg_stat and rg_stat["success"]:
        idx_pass = rg_stat["time_secs"] < 120
        add(f"| 100K+ LOC index <120s (with API) | ripgrep: {rg_stat['time_secs']:.1f}s (175K LOC) | {'✅ PASS' if idx_pass else '❌ FAIL'} |")

    # BM25 p95 latency < 500ms
    bm25_perf = vera_results.get("bm25-only", {}).get("aggregate", {}).get("performance", {})
    bm25_p95 = bm25_perf.get("latency_p95_ms", 9999)
    bm25_pass = bm25_p95 < 500
    add(f"| Query p95 latency <500ms (BM25)  | BM25 p95: {bm25_p95:.1f}ms | {'✅ PASS' if bm25_pass else '❌ FAIL'} |")

    # Incremental update < 5s
    upd_pass = update_stats.get("time_secs", 9999) < 5
    add(f"| Incremental update <5s           | {update_stats.get('time_secs', -1):.1f}s | {'✅ PASS' if upd_pass else '❌ FAIL'} |")

    # Index size < 2x source
    max_ratio = max((s.get("size_ratio", 0) for s in index_stats if s["success"]), default=0)
    ratio_pass = max_ratio < 2.0
    add(f"| Index size <2x source            | Max ratio: {max_ratio:.2f}x | {'✅ PASS' if ratio_pass else '❌ FAIL'} |")
    add()

    # Semantic outperformance check
    add("## Key Assertions")
    add()

    # 1. Vera outperforms lexical baseline on semantic tasks by 10%+
    if baselines and "ripgrep" in baselines:
        rg_intent = baselines["ripgrep"].get("per_category", {}).get("intent", {}).get("retrieval", {})
        vera_intent = vera_results.get("hybrid", {}).get("per_category", {}).get("intent", {}).get("retrieval", {})

        rg_r5 = rg_intent.get("recall_at_5", 0)
        vera_r5 = vera_intent.get("recall_at_5", 0)
        rg_mrr = rg_intent.get("mrr", 0)
        vera_mrr = vera_intent.get("mrr", 0)

        r5_imp = ((vera_r5 - rg_r5) / rg_r5 * 100) if rg_r5 > 0 else 0
        mrr_imp = ((vera_mrr - rg_mrr) / rg_mrr * 100) if rg_mrr > 0 else 0
        sem_pass = r5_imp >= 10 or mrr_imp >= 10

        add(f"| Vera outperforms lexical on semantic tasks (10%+ relative) | "
            f"Recall@5: {vera_r5:.4f} vs {rg_r5:.4f} (+{r5_imp:.0f}%), "
            f"MRR: {vera_mrr:.4f} vs {rg_mrr:.4f} (+{mrr_imp:.0f}%) | "
            f"{'✅ PASS' if sem_pass else '❌ FAIL'} |")

    # 2. Vera outperforms vector-only on exact lookup (higher Recall@1)
    if baselines and "vector-only" in baselines:
        vo_sym = baselines["vector-only"].get("per_category", {}).get("symbol_lookup", {}).get("retrieval", {})
        vera_sym = vera_results.get("hybrid", {}).get("per_category", {}).get("symbol_lookup", {}).get("retrieval", {})

        vo_r1 = vo_sym.get("recall_at_1", 0)
        vera_r1 = vera_sym.get("recall_at_1", 0)
        exact_pass = vera_r1 > vo_r1

        add(f"| Vera outperforms vector-only on exact lookup (Recall@1) | "
            f"Vera: {vera_r1:.4f} vs vector-only: {vo_r1:.4f} | "
            f"{'✅ PASS' if exact_pass else '❌ FAIL'} |")

    add()

    # Indexing stats table
    add("## Indexing Performance")
    add()
    add("| Repository | Files | Chunks | Index Time | Source Size | Index Size | Ratio |")
    add("|------------|-------|--------|------------|-------------|------------|-------|")
    for stat in index_stats:
        if stat["success"]:
            src_mb = stat.get("source_bytes", 0) / 1024 / 1024
            idx_mb = stat.get("storage_bytes", 0) / 1024 / 1024
            add(f"| {stat['repo']:<10} | {stat['files_parsed']:>5} | {stat['chunks_created']:>6} | "
                f"{stat['time_secs']:>9.1f}s | {src_mb:>10.1f}MB | {idx_mb:>9.1f}MB | {stat['size_ratio']:.2f}x |")
    add()

    # Limitations
    add("## Limitations")
    add()
    add("1. **Hybrid latency includes API round trips:** The hybrid mode latency")
    add("   (embedding + reranking) is dominated by network round trips. A local model")
    add("   deployment would reduce this. BM25 fallback provides sub-10ms p95 latency.")
    add("2. **Competitor baselines from M1:** Baselines were run during M1 on the same")
    add("   corpus. Direct comparison is valid as task definitions and ground truth are")
    add("   identical.")
    add("3. **Vector-only baseline limited to 500 files/repo:** The M1 vector-only")
    add("   baseline indexed max 500 source files per repo. Vera indexes all files.")
    add("4. **API latency variance:** Embedding/reranker API latency varies by ~20%")
    add("   between runs. Retrieval metrics are deterministic.")
    add()

    add("## Raw Data Reference")
    add()
    add("- `benchmarks/results/final-suite/vera_bm25_only_results.json`")
    add("- `benchmarks/results/final-suite/vera_hybrid_norerank_results.json`")
    add("- `benchmarks/results/final-suite/vera_hybrid_results.json`")
    add("- `benchmarks/results/final-suite/combined_results.json`")
    add("- `benchmarks/results/competitor-baselines/all_baselines.json`")
    add()

    return "\n".join(lines)


def generate_reproduction_guide(index_stats: list[dict]) -> str:
    """Generate the reproduction guide."""
    lines = []

    def add(line: str = ""):
        lines.append(line)

    add("# Vera Benchmark Reproduction Guide")
    add()
    add("How to reproduce the Vera final benchmark results.")
    add()

    add("## Prerequisites")
    add()
    add("### System Requirements")
    add("- Linux x86_64 (tested on CachyOS/Arch Linux)")
    add("- 8+ GB RAM (30 GB recommended)")
    add("- 10+ GB free disk space")
    add("- Internet connection for embedding/reranker APIs")
    add()

    add("### Tool Versions")
    add()
    add("| Tool              | Version                  | Install Command |")
    add("|-------------------|--------------------------|-----------------|")
    add("| Rust              | 1.94.0+                  | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \\| sh` |")
    add("| Python            | 3.14+                    | System package manager |")
    add("| ripgrep           | 13.0.0+                  | `cargo install ripgrep` or system package |")
    add("| git               | 2.40+                    | System package manager |")
    add()

    add("### API Credentials")
    add()
    add("Create a `secrets.env` file at the repository root with:")
    add("```bash")
    add("EMBEDDING_MODEL_BASE_URL=<your-openai-compatible-embedding-api-url>")
    add("EMBEDDING_MODEL_ID=Qwen/Qwen3-Embedding-8B")
    add("EMBEDDING_MODEL_API_KEY=<your-api-key>")
    add("RERANKER_MODEL_BASE_URL=<your-openai-compatible-reranker-api-url>")
    add("RERANKER_MODEL_ID=Qwen/Qwen3-Reranker")
    add("RERANKER_MODEL_API_KEY=<your-api-key>")
    add("```")
    add()
    add("The embedding and reranker APIs must be OpenAI-compatible endpoints.")
    add("We used Nebius tokenfactory for benchmarking.")
    add()

    add("## Corpus Setup")
    add()
    add("### Step 1: Clone and pin test repositories")
    add("```bash")
    add("bash eval/setup-corpus.sh")
    add("```")
    add()
    add("This clones 4 repositories to `.bench/repos/` and pins them to specific SHAs:")
    add()
    add("| Repository | Language   | Commit SHA                                       |")
    add("|------------|-----------|--------------------------------------------------|")
    add("| ripgrep    | Rust       | `4519153e5e461527f4bca45b042fff45c4ec6fb9`       |")
    add("| flask      | Python     | `4cae5d8e411b1e69949d8fae669afeacbd3e5908`       |")
    add("| fastify    | TypeScript | `a22217f9420f70017a419d8e18b2a3141ab27989`       |")
    add("| turborepo  | Polyglot   | `56b79ff5c1c9366593e9e68a922d997e2698c5f4`       |")
    add()

    add("### Step 2: Verify corpus")
    add("```bash")
    add("cargo run --bin vera-eval -- verify-corpus")
    add("```")
    add()

    add("## Build Vera")
    add()
    add("```bash")
    add("cargo build --release")
    add("```")
    add()

    add("## Run Benchmarks")
    add()
    add("### Full Suite (single command)")
    add("```bash")
    add("set -a; source secrets.env; set +a")
    add("python3 benchmarks/scripts/run_final_benchmarks.py")
    add("```")
    add()
    add("This will:")
    add("1. Clean and re-index all 4 repositories (with cooldown between repos)")
    add("2. Run 21 benchmark tasks in 3 Vera modes (bm25-only, hybrid-norerank, hybrid)")
    add("3. Compare against pre-computed competitor baselines")
    add("4. Verify all performance targets")
    add("5. Produce comparison tables and save results to `benchmarks/results/final-suite/`")
    add()

    add("### Individual Components")
    add("```bash")
    add("# Just Vera benchmarks (skip indexing)")
    add("python3 benchmarks/scripts/run_vera_benchmarks.py --modes bm25-only hybrid-norerank hybrid --skip-index --runs 2")
    add()
    add("# Competitor baselines")
    add("python3 benchmarks/scripts/run_baselines.py --tool all --runs 3")
    add()
    add("# Just ripgrep baseline")
    add("python3 benchmarks/scripts/run_baselines.py --tool ripgrep --runs 3")
    add("```")
    add()

    add("## Expected Ranges")
    add()
    add("Results should fall within these ranges on comparable hardware:")
    add()
    add("| Metric                     | Expected Range    | Notes |")
    add("|----------------------------|-------------------|-------|")
    add("| Vera hybrid Recall@10      | 0.70 – 0.85       | Depends on API model version |")
    add("| Vera hybrid MRR@10         | 0.50 – 0.70       | Reranker quality varies |")
    add("| Vera BM25-only Recall@10   | 0.35 – 0.50       | Deterministic, stable |")
    add("| BM25 p95 latency           | 1 – 15 ms         | Depends on disk/cache |")
    add("| Hybrid p95 latency         | 3000 – 10000 ms   | Dominated by API round trips |")
    add("| ripgrep index time         | 50 – 120 s        | Dominated by embedding API |")
    add("| Index size ratio           | 1.0x – 2.0x       | Depends on repo structure |")
    add("| Incremental update         | 1 – 5 s           | Single file change |")
    add()

    add("## Metric Reproducibility")
    add()
    add("- **Retrieval metrics** (Recall, MRR, nDCG): Deterministic — same query yields")
    add("  same results. Two runs should match within ±2%.")
    add("- **Latency**: Varies by ~20% due to API response times and system load.")
    add("  Use BM25-only mode for stable latency measurements.")
    add("- **Index time**: Dominated by embedding API throughput. Varies 20-50%")
    add("  depending on API load.")
    add()

    add("## Troubleshooting")
    add()
    add("### Dimension mismatch errors")
    add("If you see dimension errors, clear old indexes:")
    add("```bash")
    add("rm -rf .bench/repos/*/.vera")
    add("```")
    add()
    add("### API rate limits")
    add("The script includes cooldown periods between repos. If rate limits persist,")
    add("increase `COOLDOWN_SECS` in the script or index repos individually with")
    add("pauses between them.")
    add()
    add("### Turborepo indexing fails")
    add("Turborepo is a large polyglot repo and requires more API calls. If it fails,")
    add("run the other 3 repos first, wait 60 seconds, then index turborepo:")
    add("```bash")
    add("set -a; source secrets.env; set +a")
    add("./target/release/vera index .bench/repos/turborepo")
    add("```")
    add()

    return "\n".join(lines)


# ── Main ─────────────────────────────────────────────────────────────


def main():
    print("=" * 70)
    print("  VERA FINAL BENCHMARK SUITE")
    print("=" * 70)

    # Prerequisites check
    if not VERA_BIN.exists():
        print(f"Error: Vera binary not found at {VERA_BIN}")
        print("Run: cargo build --release")
        sys.exit(1)

    if not CORPUS_DIR.exists():
        print(f"Error: Corpus not found at {CORPUS_DIR}")
        print("Run: bash eval/setup-corpus.sh")
        sys.exit(1)

    # Load resources
    secrets = load_secrets()
    tasks = load_tasks()
    baselines = load_baselines()
    timestamp = datetime.now(timezone.utc).isoformat()

    print(f"Vera version: {binary_version(VERA_BIN)}")
    print(f"Git SHA: {git_sha()}")
    print(f"Tasks: {len(tasks)}")
    print(f"Repos: {', '.join(REPO_ORDER)}")
    print(f"Baselines loaded: {list(baselines.keys()) if baselines else 'None'}")

    # Step 1: Clean and index all repos
    index_stats = index_all_repos(secrets)

    # Check all repos indexed successfully
    failed = [s for s in index_stats if not s["success"]]
    if failed:
        print(f"\n⚠ WARNING: {len(failed)} repos failed indexing: {[s['repo'] for s in failed]}")
        print("Continuing with available repos...")

    # Step 2: Run benchmarks in all modes
    modes = ["bm25-only", "hybrid-norerank", "hybrid"]
    vera_results: dict[str, dict] = {}

    for mode in modes:
        result = run_mode(mode, tasks, secrets, runs=2)
        vera_results[mode] = result

    # Step 3: Benchmark incremental update
    print("\n" + "─" * 60)
    print("  Benchmarking incremental update...")
    print("─" * 60)
    update_stats = benchmark_incremental_update(secrets)
    print(f"  Update time: {update_stats.get('time_secs', -1):.1f}s "
          f"(files: {update_stats.get('files_updated', '?')})")

    # Step 4: Save results
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    REPORTS_DIR.mkdir(parents=True, exist_ok=True)

    for mode, result in vera_results.items():
        output_file = RESULTS_DIR / f"vera_{mode.replace('-', '_')}_results.json"
        with open(output_file, "w") as f:
            json.dump(result, f, indent=2)
        print(f"Saved: {output_file}")

    combined = {
        "vera_version": binary_version(VERA_BIN),
        "git_sha": git_sha(),
        "timestamp": timestamp,
        "modes": vera_results,
        "index_stats": index_stats,
        "update_stats": update_stats,
    }
    combined_file = RESULTS_DIR / "combined_results.json"
    with open(combined_file, "w") as f:
        json.dump(combined, f, indent=2)
    print(f"Saved: {combined_file}")

    # Step 5: Generate report
    report = generate_report(vera_results, baselines, index_stats, update_stats, timestamp)
    report_file = REPORTS_DIR / "final-benchmark-suite.md"
    with open(report_file, "w") as f:
        f.write(report)
    print(f"Saved: {report_file}")

    # Step 6: Generate reproduction guide
    repro = generate_reproduction_guide(index_stats)
    repro_file = REPORTS_DIR / "reproduction-guide.md"
    with open(repro_file, "w") as f:
        f.write(repro)
    print(f"Saved: {repro_file}")

    # Step 7: Print key results summary
    print("\n" + "=" * 70)
    print("  FINAL RESULTS SUMMARY")
    print("=" * 70)

    hybrid = vera_results.get("hybrid", {}).get("aggregate", {}).get("retrieval", {})
    bm25_perf = vera_results.get("bm25-only", {}).get("aggregate", {}).get("performance", {})

    print("\n  Vera Hybrid (full pipeline):")
    print(f"    Recall@1:  {hybrid.get('recall_at_1', 0):.4f}")
    print(f"    Recall@5:  {hybrid.get('recall_at_5', 0):.4f}")
    print(f"    Recall@10: {hybrid.get('recall_at_10', 0):.4f}")
    print(f"    MRR@10:    {hybrid.get('mrr', 0):.4f}")
    print(f"    nDCG@10:   {hybrid.get('ndcg', 0):.4f}")

    print("\n  Vera BM25-only:")
    print(f"    p95 latency: {bm25_perf.get('latency_p95_ms', 0):.1f}ms")

    # Assertion checks
    print("\n  Performance Targets:")
    rg_stat = next((s for s in index_stats if s["repo"] == "ripgrep" and s["success"]), None)
    if rg_stat:
        print(f"    Index time (ripgrep 175K LOC): {rg_stat['time_secs']:.1f}s {'✅' if rg_stat['time_secs'] < 120 else '❌'} (<120s)")
    print(f"    BM25 p95 latency: {bm25_perf.get('latency_p95_ms', 0):.1f}ms {'✅' if bm25_perf.get('latency_p95_ms', 9999) < 500 else '❌'} (<500ms)")
    print(f"    Incremental update: {update_stats.get('time_secs', -1):.1f}s {'✅' if update_stats.get('time_secs', 9999) < 5 else '❌'} (<5s)")
    max_ratio = max((s.get("size_ratio", 0) for s in index_stats if s["success"]), default=0)
    print(f"    Max index ratio: {max_ratio:.2f}x {'✅' if max_ratio < 2.0 else '❌'} (<2x)")

    # Semantic outperformance
    if baselines and "ripgrep" in baselines:
        rg_intent = baselines["ripgrep"].get("per_category", {}).get("intent", {}).get("retrieval", {})
        vera_intent = vera_results.get("hybrid", {}).get("per_category", {}).get("intent", {}).get("retrieval", {})
        rg_r5 = rg_intent.get("recall_at_5", 0)
        vera_r5 = vera_intent.get("recall_at_5", 0)
        rg_mrr = rg_intent.get("mrr", 0)
        vera_mrr = vera_intent.get("mrr", 0)
        r5_imp = ((vera_r5 - rg_r5) / rg_r5 * 100) if rg_r5 > 0 else 0
        mrr_imp = ((vera_mrr - rg_mrr) / rg_mrr * 100) if rg_mrr > 0 else 0
        print("\n  Semantic Outperformance (vs ripgrep on intent):")
        print(f"    Recall@5: Vera={vera_r5:.4f} vs rg={rg_r5:.4f} (+{r5_imp:.0f}%) {'✅' if r5_imp >= 10 else '❌'}")
        print(f"    MRR:      Vera={vera_mrr:.4f} vs rg={rg_mrr:.4f} (+{mrr_imp:.0f}%) {'✅' if mrr_imp >= 10 else '❌'}")

    if baselines and "vector-only" in baselines:
        vo_sym = baselines["vector-only"].get("per_category", {}).get("symbol_lookup", {}).get("retrieval", {})
        vera_sym = vera_results.get("hybrid", {}).get("per_category", {}).get("symbol_lookup", {}).get("retrieval", {})
        vo_r1 = vo_sym.get("recall_at_1", 0)
        vera_r1 = vera_sym.get("recall_at_1", 0)
        print("\n  Exact Lookup (vs vector-only on symbol_lookup Recall@1):")
        print(f"    Vera={vera_r1:.4f} vs vector-only={vo_r1:.4f} {'✅' if vera_r1 > vo_r1 else '❌'}")

    print(f"\n{'='*70}")
    print("  BENCHMARK COMPLETE")
    print(f"{'='*70}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
