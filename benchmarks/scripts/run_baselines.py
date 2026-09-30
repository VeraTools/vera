#!/usr/bin/env python3
"""
Competitor Baseline Benchmark Runner for Vera Evaluation.

Runs ripgrep, cocoindex-code, and a custom vector-only baseline against the
full Vera benchmark task suite. Produces results in the same EvalReport JSON
format used by the Vera eval harness.

Usage:
    python3 benchmarks/scripts/run_baselines.py [--tool ripgrep|cocoindex|vector-only|all]

Environment:
    EMBEDDING_MODEL_BASE_URL, EMBEDDING_MODEL_ID, EMBEDDING_MODEL_API_KEY
    (from secrets.env, needed for vector-only baseline)
"""

from bench_common import TASKS_DIR, is_match, mrr, percentile, recall_at_k

import json
import math
import os
import re
import subprocess
import time
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
CORPUS_FILE = REPO_ROOT / "eval" / "corpus.toml"
BENCH_REPOS = REPO_ROOT / ".bench" / "repos"
RESULTS_DIR = REPO_ROOT / "benchmarks" / "results" / "competitor-baselines"


# ---------------------------------------------------------------------------
# Task loading (mirrors eval harness loader)
# ---------------------------------------------------------------------------

def load_tasks() -> list[dict]:
    """Load all benchmark tasks from the tasks directory."""
    tasks = []
    for path in sorted(TASKS_DIR.glob("*.json")):
        with open(path) as f:
            data = json.load(f)
        if isinstance(data, list):
            tasks.extend(data)
        else:
            tasks.append(data)
    tasks.sort(key=lambda t: t["id"])
    return tasks


def load_corpus() -> dict:
    """Load corpus manifest and return repo name -> SHA mapping."""
    import tomllib
    with open(CORPUS_FILE, "rb") as f:
        manifest = tomllib.load(f)
    return {r["name"]: r["commit"] for r in manifest.get("repos", [])}


# ---------------------------------------------------------------------------
# Metrics computation (mirrors eval harness metrics)
# ---------------------------------------------------------------------------




def ndcg(results: list[dict], ground_truth: list[dict], k: int = 10) -> float:
    top_k = results[:k]
    dcg = 0.0
    for i, r in enumerate(top_k):
        rel = max((gt.get("relevance", 1) for gt in ground_truth
                    if is_match(r, gt)), default=0)
        dcg += rel / math.log2(i + 2)

    ideal_rels = sorted([gt.get("relevance", 1) for gt in ground_truth],
                         reverse=True)[:k]
    ideal_dcg = sum(rel / math.log2(i + 2)
                     for i, rel in enumerate(ideal_rels))
    return dcg / ideal_dcg if ideal_dcg > 0 else 0.0


def compute_metrics(results: list[dict], ground_truth: list[dict]) -> dict:
    return {
        "recall_at_1": recall_at_k(results, ground_truth, 1),
        "recall_at_5": recall_at_k(results, ground_truth, 5),
        "recall_at_10": recall_at_k(results, ground_truth, 10),
        "mrr": mrr(results, ground_truth),
        "ndcg": ndcg(results, ground_truth, 10),
    }



def estimate_tokens(text: str) -> int:
    return math.ceil(len(text) / 4)


# ---------------------------------------------------------------------------
# ripgrep adapter
# ---------------------------------------------------------------------------

class RipgrepAdapter:
    """Run ripgrep searches against repos and convert results."""

    def __init__(self):
        self.name = "ripgrep"
        self.version = self._get_version()

    def _get_version(self) -> str:
        try:
            result = subprocess.run(["rg", "--version"], capture_output=True, text=True)
            first_line = result.stdout.strip().split("\n")[0]
            return first_line
        except Exception:
            return "ripgrep (unknown version)"

    def search(self, query: str, repo_path: str, max_results: int = 20) -> list[dict]:
        """
        Search using ripgrep. Strategy:
        - Extract key terms from the query (identifiers, significant words)
        - Search for each term and combine results weighted by match frequency
        - For identifier queries, also try exact match
        """
        stop_words = {"the", "a", "an", "is", "are", "how", "what", "where",
                      "when", "and", "or", "to", "in", "of", "for", "with",
                      "by", "on", "at", "from", "as", "its", "it", "be",
                      "do", "does", "did", "have", "has", "had", "that",
                      "this", "which", "their", "there", "been"}
        words = query.split()

        # Extract identifiers (CamelCase, snake_case, dotted, filenames) and keywords
        identifiers = []
        keywords = []
        for w in words:
            clean = w.strip(".,;:()\"'")
            if not clean:
                continue
            # Looks like a filename (has extension)
            if re.match(r'^[\w.-]+\.\w{1,5}$', clean):
                identifiers.append(clean)
            # Looks like code identifier
            elif ("_" in clean or "." in clean or
                (any(c.isupper() for c in clean[1:]))):
                identifiers.append(clean)
            elif clean.lower() not in stop_words and len(clean) > 2:
                keywords.append(clean)

        # Score files: identifiers are worth more
        file_scores: dict[str, float] = {}
        file_results: dict[str, dict] = {}

        # Search identifiers with higher weight
        for ident in identifiers:
            results = self._rg_search(ident, repo_path, 50)
            for r in results:
                key = f"{r['file_path']}:{r['line_start']}-{r['line_end']}"
                file_scores[key] = file_scores.get(key, 0) + r.get("score", 1.0) * 3
                file_results[key] = r

        # Search keywords with normal weight
        for kw in keywords[:6]:
            results = self._rg_search(kw, repo_path, 30)
            for r in results:
                key = f"{r['file_path']}:{r['line_start']}-{r['line_end']}"
                file_scores[key] = file_scores.get(key, 0) + r.get("score", 1.0)
                file_results[key] = r

        # If no identifiers or keywords found, try the whole query
        if not identifiers and not keywords:
            for w in words[:3]:
                results = self._rg_search(w, repo_path, 30)
                for r in results:
                    key = f"{r['file_path']}:{r['line_start']}-{r['line_end']}"
                    file_scores[key] = file_scores.get(key, 0) + r.get("score", 1.0)
                    file_results[key] = r

        # Boost results that match multiple terms
        ranked = sorted(file_scores.items(), key=lambda x: -x[1])
        return [file_results[key] for key, _ in ranked[:max_results]]

    def _rg_search(self, pattern: str, repo_path: str,
                    max_results: int = 20) -> list[dict]:
        """Execute a single rg search and parse results."""
        results = []

        # Search content
        try:
            cmd = [
                "rg", "--json", "--max-count", "50",
                "-i",  # case insensitive
                "--no-heading",
                pattern, repo_path
            ]
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        except (subprocess.TimeoutExpired, Exception):
            result = subprocess.CompletedProcess(args=[], returncode=1, stdout="")

        seen_files: dict[str, list[int]] = {}

        for line in result.stdout.strip().split("\n"):
            if not line:
                continue
            try:
                data = json.loads(line)
            except json.JSONDecodeError:
                continue

            if data.get("type") != "match":
                continue

            match_data = data.get("data", {})
            abs_path = match_data.get("path", {}).get("text", "")
            line_number = match_data.get("line_number", 0)

            # Convert to relative path
            if abs_path.startswith(repo_path):
                rel_path = abs_path[len(repo_path):].lstrip("/")
            else:
                rel_path = abs_path

            if not rel_path:
                continue

            if rel_path not in seen_files:
                seen_files[rel_path] = []
            seen_files[rel_path].append(line_number)

        # Also search filenames - if pattern looks like a filename
        if "." in pattern or "/" in pattern:
            try:
                file_cmd = ["rg", "--files", repo_path]
                file_result = subprocess.run(file_cmd, capture_output=True,
                                             text=True, timeout=10)
                for fline in file_result.stdout.strip().split("\n"):
                    if not fline:
                        continue
                    rel = fline
                    if fline.startswith(repo_path):
                        rel = fline[len(repo_path):].lstrip("/")
                    if pattern.lower() in rel.lower():
                        if rel not in seen_files:
                            seen_files[rel] = [1]  # file match
            except Exception:
                pass

        # Group consecutive lines in same file into ranges
        for file_path, lines in seen_files.items():
            lines.sort()
            # Group lines within 30 lines of each other
            groups = []
            current_group = [lines[0]]
            for ln in lines[1:]:
                if ln - current_group[-1] <= 30:
                    current_group.append(ln)
                else:
                    groups.append(current_group)
                    current_group = [ln]
            groups.append(current_group)

            for group in groups:
                line_start = max(1, group[0] - 5)
                line_end = group[-1] + 5
                results.append({
                    "file_path": file_path,
                    "line_start": line_start,
                    "line_end": line_end,
                    "score": len(group),  # more matches = higher score
                })

        # Sort by score descending
        results.sort(key=lambda r: -r["score"])
        return results[:max_results]

    def index(self, repo_path: str) -> tuple[float, int]:
        """ripgrep doesn't need indexing."""
        return (0.0, 0)


# ---------------------------------------------------------------------------
# cocoindex-code adapter
# ---------------------------------------------------------------------------

class CocoindexAdapter:
    """Run cocoindex-code searches against repos."""

    def __init__(self):
        self.name = "cocoindex-code"
        self.version = self._get_version()
        self._indexed_repos: set[str] = set()

    def _get_version(self) -> str:
        try:
            result = subprocess.run(
                ["pip", "show", "cocoindex-code"],
                capture_output=True, text=True
            )
            for line in result.stdout.split("\n"):
                if line.startswith("Version:"):
                    return f"cocoindex-code {line.split(':')[1].strip()}"
        except Exception:
            pass
        return "cocoindex-code 0.2.4"

    def _get_version_from_pipx(self) -> str:
        try:
            result = subprocess.run(
                ["pipx", "list", "--short"],
                capture_output=True, text=True
            )
            for line in result.stdout.split("\n"):
                if "cocoindex-code" in line:
                    return line.strip()
        except Exception:
            pass
        return "cocoindex-code 0.2.4"

    def index(self, repo_path: str) -> tuple[float, int]:
        """Index a repo with cocoindex-code."""
        if repo_path in self._indexed_repos:
            return (0.0, 0)

        start = time.time()
        try:
            # Initialize the project first
            subprocess.run(
                ["ccc", "init"],
                cwd=repo_path,
                capture_output=True, text=True,
                timeout=30
            )
            # Then index
            subprocess.run(
                ["ccc", "index"],
                cwd=repo_path,
                capture_output=True, text=True,
                timeout=300
            )
            elapsed = time.time() - start
            self._indexed_repos.add(repo_path)

            # Get storage size
            coco_dir = Path(repo_path) / ".cocoindex_code"
            storage = sum(f.stat().st_size for f in coco_dir.rglob("*")
                          if f.is_file()) if coco_dir.exists() else 0

            print(f"  [cocoindex] Indexed {repo_path} in {elapsed:.1f}s, "
                  f"storage: {storage // 1024}KB")
            return (elapsed, storage)
        except subprocess.TimeoutExpired:
            elapsed = time.time() - start
            print(f"  [cocoindex] Indexing timed out for {repo_path}")
            self._indexed_repos.add(repo_path)
            return (elapsed, 0)
        except Exception as e:
            elapsed = time.time() - start
            print(f"  [cocoindex] Indexing error for {repo_path}: {e}")
            return (elapsed, 0)

    def search(self, query: str, repo_path: str, max_results: int = 20) -> list[dict]:
        """Search using ccc search."""
        try:
            result = subprocess.run(
                ["ccc", "search", query, "--limit", str(max_results)],
                cwd=repo_path,
                capture_output=True, text=True,
                timeout=60
            )
            return self._parse_search_output(result.stdout, repo_path)
        except (subprocess.TimeoutExpired, Exception) as e:
            print(f"  [cocoindex] Search error: {e}")
            return []

    def _parse_search_output(self, output: str, repo_path: str) -> list[dict]:
        """Parse ccc search output into result dicts.

        Expected format:
            --- Result 1 (score: 0.740) ---
            File: docs/views.rst:1-18 [rst]
            <content lines>
        """
        results = []

        # Pattern: --- Result N (score: X.XXX) ---
        result_header = re.compile(r'---\s*Result\s+\d+\s*\(score:\s*([\d.]+)\)\s*---')
        # Pattern: File: path/to/file.ext:start-end [lang]
        file_header = re.compile(r'File:\s*(.+?):(\d+)-(\d+)\s*\[(\w+)\]')
        # Also match without line range: File: path/to/file.ext [lang]
        file_header_no_range = re.compile(r'File:\s*(.+?)\s*\[(\w+)\]')

        lines = output.split("\n")
        i = 0
        while i < len(lines):
            line = lines[i].strip()

            # Look for result header
            m = result_header.match(line)
            if m:
                score = float(m.group(1))
                # Next non-empty line should be File: header
                i += 1
                while i < len(lines) and not lines[i].strip():
                    i += 1

                if i < len(lines):
                    line_text = lines[i].strip()
                    fm = file_header.match(line_text)
                    if fm:
                        file_path = fm.group(1).strip()
                        line_start = int(fm.group(2))
                        line_end = int(fm.group(3))
                    else:
                        fm = file_header_no_range.match(line_text)
                        if fm:
                            file_path = fm.group(1).strip()
                            line_start = 1
                            line_end = 50
                        else:
                            i += 1
                            continue

                    # Remove repo_path prefix if present
                    if file_path.startswith(repo_path):
                        file_path = file_path[len(repo_path):].lstrip("/")

                    results.append({
                        "file_path": file_path,
                        "line_start": line_start,
                        "line_end": line_end,
                        "score": score,
                    })
            i += 1

        return results


# ---------------------------------------------------------------------------
# Vector-only baseline (custom embedding + cosine search)
# ---------------------------------------------------------------------------

class VectorOnlyAdapter:
    """
    Pure vector search baseline using Qwen3 embedding API.
    Chunks files, embeds them, then does cosine similarity search.
    """

    def __init__(self):
        self.name = "vector-only"
        self.version = self._get_version()
        self._indexes: dict[str, dict] = {}  # repo_path -> {"chunks": [...], "embeddings": [...]}
        self._api_base = os.environ.get("EMBEDDING_MODEL_BASE_URL", "")
        self._api_key = os.environ.get("EMBEDDING_MODEL_API_KEY", "")
        self._model_id = os.environ.get("EMBEDDING_MODEL_ID", "")

    def _get_version(self) -> str:
        model = os.environ.get("EMBEDDING_MODEL_ID", "unknown")
        return f"vector-only (embedding: {model})"

    def _embed_batch(self, texts: list[str]) -> list[list[float]]:
        """Get embeddings from the API."""
        import urllib.request
        import urllib.error

        if not self._api_base or not self._api_key:
            raise ValueError("EMBEDDING_MODEL_BASE_URL and EMBEDDING_MODEL_API_KEY required")

        url = f"{self._api_base}/embeddings"
        headers = {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self._api_key}",
        }
        payload = {
            "input": texts,
            "model": self._model_id,
        }

        req = urllib.request.Request(
            url,
            data=json.dumps(payload).encode(),
            headers=headers,
            method="POST"
        )

        try:
            with urllib.request.urlopen(req, timeout=120) as resp:
                data = json.loads(resp.read())
            return [item["embedding"] for item in data["data"]]
        except urllib.error.HTTPError as e:
            error_body = e.read().decode() if e.fp else ""
            print(f"  [vector] Embedding API error: {e.code} {error_body[:200]}")
            raise

    def _chunk_file(self, file_path: str, repo_path: str) -> list[dict]:
        """Simple line-based chunking of a file."""
        abs_path = os.path.join(repo_path, file_path)
        try:
            with open(abs_path, "r", errors="replace") as f:
                lines = f.readlines()
        except (OSError, UnicodeDecodeError):
            return []

        if not lines:
            return []

        chunks = []
        chunk_size = 50  # lines per chunk
        overlap = 10

        i = 0
        while i < len(lines):
            end = min(i + chunk_size, len(lines))
            content = "".join(lines[i:end])
            chunks.append({
                "file_path": file_path,
                "line_start": i + 1,
                "line_end": end,
                "content": content,
            })
            i += chunk_size - overlap

        return chunks

    def _get_source_files(self, repo_path: str, max_files: int = 500) -> list[str]:
        """Get source files using ripgrep's file listing.

        Limits to max_files to keep API costs/time reasonable.
        Prioritizes source code files over docs/tests.
        """
        extensions = {
            ".rs", ".py", ".js", ".ts", ".tsx", ".go", ".java", ".c", ".cpp",
            ".h", ".hpp", ".rb", ".toml", ".json", ".yaml", ".yml",
            ".css", ".html", ".sh", ".sql",
        }
        source_files = []
        other_files = []
        try:
            result = subprocess.run(
                ["rg", "--files", repo_path],
                capture_output=True, text=True, timeout=30
            )
            for line in result.stdout.strip().split("\n"):
                if not line:
                    continue
                rel = line
                if line.startswith(repo_path):
                    rel = line[len(repo_path):].lstrip("/")
                ext = os.path.splitext(rel)[1].lower()
                if ext not in extensions:
                    continue
                # Skip common non-source dirs
                if any(part in rel for part in [
                    "node_modules/", ".git/", "vendor/", "target/",
                    "__pycache__/", ".venv/", "dist/", "build/",
                    "test_fixtures/", "testdata/",
                ]):
                    continue
                # Prioritize: src/ and lib/ files first, then tests/docs
                if any(p in rel for p in ["src/", "lib/", "crates/", "packages/"]):
                    source_files.append(rel)
                else:
                    other_files.append(rel)
        except Exception:
            pass
        # Source files first, then fill with others up to limit
        all_files = source_files + other_files
        return all_files[:max_files]

    def index(self, repo_path: str) -> tuple[float, int]:
        """Index a repo by chunking and embedding all source files."""
        if repo_path in self._indexes:
            return (0.0, 0)

        start = time.time()
        files = self._get_source_files(repo_path)
        print(f"  [vector] Found {len(files)} source files in {os.path.basename(repo_path)}")

        # Chunk all files
        all_chunks = []
        for f in files:
            all_chunks.extend(self._chunk_file(f, repo_path))

        print(f"  [vector] Created {len(all_chunks)} chunks")

        if not all_chunks:
            self._indexes[repo_path] = {"chunks": [], "embeddings": []}
            return (time.time() - start, 0)

        # Embed in batches
        batch_size = 32
        all_embeddings = []
        for i in range(0, len(all_chunks), batch_size):
            batch = all_chunks[i:i + batch_size]
            texts = [c["content"][:2000] for c in batch]  # truncate long chunks
            try:
                embeddings = self._embed_batch(texts)
                all_embeddings.extend(embeddings)
            except Exception as e:
                print(f"  [vector] Embedding batch {i//batch_size} failed: {e}")
                all_embeddings.extend([[0.0] * 768] * len(batch))

            if (i // batch_size) % 10 == 0:
                print(f"  [vector] Embedded {min(i + batch_size, len(all_chunks))}/{len(all_chunks)} chunks")

        elapsed = time.time() - start
        storage = sum(len(json.dumps(e)) for e in all_embeddings)  # approximate

        self._indexes[repo_path] = {
            "chunks": all_chunks,
            "embeddings": all_embeddings,
        }

        print(f"  [vector] Indexed {os.path.basename(repo_path)} in {elapsed:.1f}s")
        return (elapsed, storage)

    def search(self, query: str, repo_path: str, max_results: int = 20) -> list[dict]:
        """Search by embedding the query and finding nearest chunks."""
        if repo_path not in self._indexes:
            return []

        index = self._indexes[repo_path]
        if not index["chunks"]:
            return []

        # Embed query
        try:
            query_emb = self._embed_batch([query])[0]
        except Exception:
            return []

        # Cosine similarity
        scores = []
        for i, emb in enumerate(index["embeddings"]):
            sim = self._cosine_similarity(query_emb, emb)
            scores.append((i, sim))

        # Sort by similarity
        scores.sort(key=lambda x: -x[1])

        results = []
        for idx, score in scores[:max_results]:
            chunk = index["chunks"][idx]
            results.append({
                "file_path": chunk["file_path"],
                "line_start": chunk["line_start"],
                "line_end": chunk["line_end"],
                "score": score,
            })

        return results

    @staticmethod
    def _cosine_similarity(a: list[float], b: list[float]) -> float:
        dot = sum(x * y for x, y in zip(a, b))
        norm_a = math.sqrt(sum(x * x for x in a))
        norm_b = math.sqrt(sum(x * x for x in b))
        if norm_a == 0 or norm_b == 0:
            return 0.0
        return dot / (norm_a * norm_b)


# ---------------------------------------------------------------------------
# Benchmark runner
# ---------------------------------------------------------------------------

def run_benchmark(adapter, tasks: list[dict], corpus_shas: dict,
                   num_timing_runs: int = 3) -> dict:
    """Run all tasks through an adapter and produce an EvalReport."""
    timestamp = datetime.now(timezone.utc).isoformat()
    print(f"\n{'='*60}")
    print(f"Running benchmark: {adapter.name}")
    print(f"{'='*60}")

    # Index all repos
    total_index_time = 0.0
    total_storage = 0
    repos_needed = set(t["repo"] for t in tasks)

    for repo_name in sorted(repos_needed):
        repo_path = str(BENCH_REPOS / repo_name)
        if not os.path.isdir(repo_path):
            print(f"  WARNING: Repo {repo_name} not found at {repo_path}")
            continue
        print(f"  Indexing {repo_name}...")
        idx_time, idx_size = adapter.index(repo_path)
        total_index_time += idx_time
        total_storage += idx_size

    # Run all tasks
    per_task = []
    all_latencies = []

    for task in tasks:
        repo_path = str(BENCH_REPOS / task["repo"])
        if not os.path.isdir(repo_path):
            continue

        # Run multiple times for timing
        latencies = []
        results = []
        for run in range(num_timing_runs):
            start = time.time()
            results = adapter.search(task["query"], repo_path)
            elapsed_ms = (time.time() - start) * 1000
            latencies.append(elapsed_ms)

        median_latency = sorted(latencies)[len(latencies) // 2]
        all_latencies.append(median_latency)

        # Compute metrics
        metrics = compute_metrics(results, task["ground_truth"])

        task_eval = {
            "task_id": task["id"],
            "category": task["category"],
            "retrieval_metrics": metrics,
            "latency_ms": median_latency,
            "result_count": len(results),
        }
        per_task.append(task_eval)

        status = "✓" if metrics["recall_at_10"] > 0 else "✗"
        print(f"  {status} {task['id']:<25} R@10={metrics['recall_at_10']:.2f} "
              f"MRR={metrics['mrr']:.4f} lat={median_latency:.1f}ms")

    # Compute aggregates
    per_category = compute_category_aggregates(per_task, all_latencies,
                                                total_index_time, total_storage)
    aggregate = compute_aggregate(per_task, all_latencies,
                                    total_index_time, total_storage)

    config = {}
    if hasattr(adapter, '_model_id'):
        config["embedding_model"] = adapter._model_id
    if hasattr(adapter, '_api_base'):
        # Don't expose full URL, just note it's configured
        config["embedding_api"] = "configured"

    report = {
        "tool_name": adapter.name,
        "timestamp": timestamp,
        "version_info": {
            "tool_version": adapter.version,
            "corpus_version": 1,
            "repo_shas": corpus_shas,
            "config": config,
        },
        "per_task": per_task,
        "per_category": per_category,
        "aggregate": aggregate,
    }

    return report


def compute_category_aggregates(per_task, all_latencies,
                                 index_time, storage) -> dict:
    by_cat = defaultdict(list)
    for t in per_task:
        by_cat[t["category"]].append(t)

    result = {}
    for cat, tasks in by_cat.items():
        lats = sorted([t["latency_ms"] for t in tasks])
        n = len(tasks)
        result[cat] = {
            "retrieval": {
                "recall_at_1": sum(t["retrieval_metrics"]["recall_at_1"] for t in tasks) / n,
                "recall_at_5": sum(t["retrieval_metrics"]["recall_at_5"] for t in tasks) / n,
                "recall_at_10": sum(t["retrieval_metrics"]["recall_at_10"] for t in tasks) / n,
                "mrr": sum(t["retrieval_metrics"]["mrr"] for t in tasks) / n,
                "ndcg": sum(t["retrieval_metrics"]["ndcg"] for t in tasks) / n,
            },
            "performance": {
                "latency_p50_ms": percentile(lats, 50),
                "latency_p95_ms": percentile(lats, 95),
                "index_time_secs": index_time,
                "storage_size_bytes": storage,
                "total_token_count": 0,
            },
            "task_count": n,
        }
    return result


def compute_aggregate(per_task, all_latencies, index_time, storage) -> dict:
    n = len(per_task)
    if n == 0:
        return {
            "retrieval": {"recall_at_1": 0, "recall_at_5": 0, "recall_at_10": 0,
                          "mrr": 0, "ndcg": 0},
            "performance": {"latency_p50_ms": 0, "latency_p95_ms": 0,
                            "index_time_secs": 0, "storage_size_bytes": 0,
                            "total_token_count": 0},
            "task_count": 0,
        }

    lats = sorted(all_latencies)
    total_tokens = sum(estimate_tokens(t["task_id"]) * t["result_count"]
                        for t in per_task)

    return {
        "retrieval": {
            "recall_at_1": sum(t["retrieval_metrics"]["recall_at_1"] for t in per_task) / n,
            "recall_at_5": sum(t["retrieval_metrics"]["recall_at_5"] for t in per_task) / n,
            "recall_at_10": sum(t["retrieval_metrics"]["recall_at_10"] for t in per_task) / n,
            "mrr": sum(t["retrieval_metrics"]["mrr"] for t in per_task) / n,
            "ndcg": sum(t["retrieval_metrics"]["ndcg"] for t in per_task) / n,
        },
        "performance": {
            "latency_p50_ms": percentile(lats, 50),
            "latency_p95_ms": percentile(lats, 95),
            "index_time_secs": index_time,
            "storage_size_bytes": storage,
            "total_token_count": total_tokens,
        },
        "task_count": n,
    }


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    import argparse
    parser = argparse.ArgumentParser(description="Run competitor baselines")
    parser.add_argument("--tool", default="all",
                        choices=["ripgrep", "cocoindex", "vector-only", "all"])
    parser.add_argument("--runs", type=int, default=3,
                        help="Number of timing runs per query")
    args = parser.parse_args()

    # Load tasks and corpus
    tasks = load_tasks()
    corpus_shas = load_corpus()
    print(f"Loaded {len(tasks)} benchmark tasks")
    print(f"Corpus repos: {', '.join(corpus_shas.keys())}")

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    tools_to_run = []
    if args.tool in ("ripgrep", "all"):
        tools_to_run.append(("ripgrep", RipgrepAdapter()))
    if args.tool in ("cocoindex", "all"):
        tools_to_run.append(("cocoindex", CocoindexAdapter()))
    if args.tool in ("vector-only", "all"):
        tools_to_run.append(("vector-only", VectorOnlyAdapter()))

    reports = {}
    for name, adapter in tools_to_run:
        try:
            report = run_benchmark(adapter, tasks, corpus_shas,
                                    num_timing_runs=args.runs)
            reports[name] = report

            # Save individual result
            output_path = RESULTS_DIR / f"{name}_results.json"
            with open(output_path, "w") as f:
                json.dump(report, f, indent=2)
            print(f"\n  Results saved to {output_path}")

        except Exception as e:
            print(f"\n  ERROR running {name}: {e}")
            import traceback
            traceback.print_exc()

    # Save combined results
    if reports:
        combined_path = RESULTS_DIR / "all_baselines.json"
        with open(combined_path, "w") as f:
            json.dump(reports, f, indent=2)
        print(f"\nCombined results saved to {combined_path}")

    # Print summary table
    if reports:
        print_comparison_table(reports)


def print_comparison_table(reports: dict):
    """Print a comparison table of all tool results."""
    print(f"\n{'='*80}")
    print("COMPETITOR BASELINE COMPARISON")
    print(f"{'='*80}\n")

    # Header
    tools = list(reports.keys())
    header = f"{'Metric':<20}"
    for t in tools:
        header += f" {t:>15}"
    print(header)
    print("-" * len(header))

    # Retrieval metrics
    for metric in ["recall_at_1", "recall_at_5", "recall_at_10", "mrr", "ndcg"]:
        label = metric.replace("_", " ").title()
        row = f"{label:<20}"
        for t in tools:
            val = reports[t]["aggregate"]["retrieval"][metric]
            row += f" {val:>15.4f}"
        print(row)

    print()
    # Performance metrics
    for metric, label in [
        ("latency_p50_ms", "Latency p50 (ms)"),
        ("latency_p95_ms", "Latency p95 (ms)"),
        ("index_time_secs", "Index time (s)"),
        ("storage_size_bytes", "Storage (bytes)"),
    ]:
        row = f"{label:<20}"
        for t in tools:
            val = reports[t]["aggregate"]["performance"][metric]
            if isinstance(val, float):
                row += f" {val:>15.2f}"
            else:
                row += f" {val:>15}"
        print(row)

    print()


if __name__ == "__main__":
    main()
