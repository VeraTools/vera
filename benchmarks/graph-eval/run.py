#!/usr/bin/env python3
"""Measure reference-resolution correctness, not retrieval quality.

`vera references <symbol>` matches call sites by name alone, so every
definition sharing a final segment collapses onto one answer. This harness
asks for the callers of an ambiguous symbol and checks which of the returned
call sites actually belong to the definition under test.

Each case names a target definition, a regex that identifies call sites
reaching that definition, and a regex that identifies call sites reaching a
different definition of the same name. Classification therefore stays
auditable: the rules live in cases.json next to the evidence that produced
them.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, NoReturn

REPO_ROOT = Path(__file__).resolve().parents[2]
CASES_FILE = Path(__file__).resolve().parent / "cases.json"
CORPUS_ROOT = REPO_ROOT / ".bench" / "semble-repos"
VERA_BINARY = REPO_ROOT / "target" / "release" / "vera"
RUNS_ROOT = Path.home() / ".cache" / "graph-eval"
SOURCE_SUFFIXES = {".py", ".js", ".mjs", ".ts"}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"graph-eval: error: {message}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--setup-only", action="store_true", help="Copy and index the repos")
    group.add_argument("--run", metavar="DIR", help="Query and score an existing run directory")
    parser.add_argument("--case", metavar="ID", help="Run a single case")
    parser.add_argument("--limit", type=int, default=200, help="Result limit per query")
    return parser.parse_args()


def load_cases() -> dict[str, Any]:
    if not CASES_FILE.is_file():
        fail(f"cases file is missing: {CASES_FILE}")
    return json.loads(CASES_FILE.read_text(encoding="utf-8"))


def repos_used(cases: list[dict[str, Any]]) -> list[str]:
    seen: list[str] = []
    for case in cases:
        if case["repo"] not in seen:
            seen.append(case["repo"])
    return seen


def vera_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PATH"] = os.pathsep.join([str(VERA_BINARY.parent), env.get("PATH", "")])
    env["VERA_LOCAL"] = "1"
    return env


def setup_run(cases: list[dict[str, Any]]) -> Path:
    if not VERA_BINARY.is_file():
        fail(f"release binary is missing: {VERA_BINARY} (build it before running this harness)")
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = RUNS_ROOT / stamp
    run_dir.mkdir(parents=True, exist_ok=False)
    for repo in repos_used(cases):
        source = CORPUS_ROOT / repo
        if not source.is_dir():
            fail(f"corpus repo is missing: {source}")
        destination = run_dir / repo
        subprocess.run(
            [
                "rsync",
                "-a",
                "--exclude",
                ".git",
                "--exclude",
                ".vera",
                f"{source}/",
                f"{destination}/",
            ],
            check=True,
            timeout=600,
        )
        log = run_dir / f"{repo}-index.log"
        with log.open("w", encoding="utf-8") as output:
            result = subprocess.run(
                ["nice", "-n", "10", str(VERA_BINARY), "index", "."],
                cwd=destination,
                env=vera_env(),
                stdout=output,
                stderr=subprocess.STDOUT,
                timeout=3600,
                check=False,
            )
        if result.returncode != 0:
            fail(f"indexing {repo} failed; see {log}")
    (run_dir / "cases.json").write_text(CASES_FILE.read_text(encoding="utf-8"), encoding="utf-8")
    return run_dir


def strip_comments(text: str, suffix: str) -> list[str]:
    """Return lines with docstring and comment bodies blanked out.

    Documentation shows example calls (`app.add_url_rule("/", ...)`) that are
    not call sites. Counting them as ground truth would penalize the index for
    correctly ignoring prose.
    """
    lines = text.splitlines()
    output: list[str] = []
    in_docstring: str | None = None
    in_block = False
    for line in lines:
        cleaned = line
        if suffix == ".py":
            rest = line
            while True:
                if in_docstring:
                    end = rest.find(in_docstring)
                    if end == -1:
                        cleaned = ""
                        rest = ""
                        break
                    rest = rest[end + 3 :]
                    cleaned = rest
                    in_docstring = None
                    continue
                start = min(
                    (index for index in (rest.find('"""'), rest.find("'''")) if index != -1),
                    default=-1,
                )
                if start == -1:
                    break
                marker = rest[start : start + 3]
                after = rest[start + 3 :]
                if marker in after:
                    rest = after[after.find(marker) + 3 :]
                    cleaned = rest
                    continue
                in_docstring = marker
                cleaned = rest[:start]
                break
            if in_docstring and cleaned == line:
                cleaned = ""
        else:
            if in_block:
                end = cleaned.find("*/")
                if end == -1:
                    cleaned = ""
                else:
                    cleaned = cleaned[end + 2 :]
                    in_block = False
            start = cleaned.find("/*")
            if start != -1 and "*/" not in cleaned[start:]:
                cleaned = cleaned[:start]
                in_block = True
            comment = cleaned.find("//")
            if comment != -1:
                cleaned = cleaned[:comment]
        output.append(cleaned)
    return output


def scan_ground_truth(repo_dir: Path, pattern: re.Pattern[str]) -> set[tuple[str, int]]:
    """Every call site in the repo that the case's target rule matches."""
    hits: set[tuple[str, int]] = set()
    for path in repo_dir.rglob("*"):
        if not path.is_file() or path.suffix not in SOURCE_SUFFIXES:
            continue
        if any(part in {".git", ".vera", "node_modules", "dist"} for part in path.parts):
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        relative = str(path.relative_to(repo_dir))
        for number, line in enumerate(strip_comments(text, path.suffix), start=1):
            if pattern.search(line):
                hits.add((relative, number))
    return hits


def query_callers(
    repo_dir: Path, symbol: str, limit: int, receiver: str | None = None
) -> list[dict[str, Any]]:
    # --compact drops call-site bodies. The classifier reads the source file
    # itself, so bodies only spend the output budget and would push real call
    # sites out of the answer.
    command = [str(VERA_BINARY), "references", symbol, "--json", "--compact", "--limit", str(limit)]
    if receiver:
        command += ["--receiver", receiver]
    result = subprocess.run(
        command,
        cwd=repo_dir,
        env=vera_env(),
        text=True,
        capture_output=True,
        timeout=300,
        check=False,
    )
    if result.returncode != 0:
        return []
    text = result.stdout.strip()
    if not text:
        return []
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        fail(f"could not parse `vera references {symbol} --json` output")
    if isinstance(payload, list):
        return [hit for hit in payload if isinstance(hit, dict) and "file_path" in hit]
    if isinstance(payload, dict) and isinstance(payload.get("results"), list):
        return payload["results"]
    return []


def classify(
    repo_dir: Path,
    hit: dict[str, Any],
    target: re.Pattern[str],
    other: re.Pattern[str],
) -> tuple[str, tuple[str, int] | None]:
    """Label one returned hit by reading the lines it points at."""
    relative = hit.get("file_path", "")
    path = repo_dir / relative
    start = int(hit.get("line_start", 0) or 0)
    end = int(hit.get("line_end", start) or start)
    if not path.is_file() or start <= 0:
        return "unknown", None
    lines = strip_comments(path.read_text(encoding="utf-8", errors="replace"), path.suffix)
    window = range(max(1, start), min(len(lines), end) + 1)
    for number in window:
        if target.search(lines[number - 1]):
            return "target", (relative, number)
    for number in window:
        if other.search(lines[number - 1]):
            return "other", (relative, number)
    return "unknown", None


def score_case(
    run_dir: Path, case: dict[str, Any], limit: int, use_receivers: bool = False
) -> dict[str, Any]:
    repo_dir = run_dir / case["repo"]
    if not (repo_dir / ".vera").is_dir():
        fail(f"{case['repo']} is not indexed in {run_dir}; run --setup-only first")
    target = re.compile(case["target_call"])
    other = re.compile(case["other_call"])
    receivers = case.get("target_receivers", []) if use_receivers else []
    if receivers:
        hits = []
        seen: set[tuple[str, int]] = set()
        for receiver in receivers:
            for hit in query_callers(repo_dir, case["symbol"], limit, receiver):
                key = (hit.get("file_path", ""), int(hit.get("line_start", 0) or 0))
                if key not in seen:
                    seen.add(key)
                    hits.append(hit)
    else:
        hits = query_callers(repo_dir, case["symbol"], limit)
    truth = scan_ground_truth(repo_dir, target)
    found: set[tuple[str, int]] = set()
    counts = {"target": 0, "other": 0, "unknown": 0}
    confusions: list[str] = []
    for hit in hits:
        label, location = classify(repo_dir, hit, target, other)
        counts[label] += 1
        if label == "target" and location:
            found.add(location)
        if label == "other":
            confusions.append(f"{hit.get('file_path')}:{hit.get('line_start')}")
    resolved = counts["target"] + counts["other"]
    precision = counts["target"] / resolved if resolved else None
    recall = len(found) / len(truth) if truth else None
    return {
        "id": case["id"],
        "repo": case["repo"],
        "category": case["category"],
        "symbol": case["symbol"],
        "returned": len(hits),
        "target_hits": counts["target"],
        "confusions": counts["other"],
        "unclassified": counts["unknown"],
        "ground_truth": len(truth),
        "precision": precision,
        "recall": recall,
        "confusion_examples": confusions[:5],
    }


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    by_category: dict[str, dict[str, int]] = {}
    totals = {"target": 0, "other": 0, "truth": 0, "found": 0}
    for row in rows:
        bucket = by_category.setdefault(row["category"], {"target": 0, "other": 0, "cases": 0})
        bucket["target"] += row["target_hits"]
        bucket["other"] += row["confusions"]
        bucket["cases"] += 1
        totals["target"] += row["target_hits"]
        totals["other"] += row["confusions"]
        totals["truth"] += row["ground_truth"]
        totals["found"] += round((row["recall"] or 0) * row["ground_truth"])
    resolved = totals["target"] + totals["other"]
    return {
        "cases": len(rows),
        "precision": totals["target"] / resolved if resolved else None,
        "recall": totals["found"] / totals["truth"] if totals["truth"] else None,
        "confusion_rate": sum(1 for row in rows if row["confusions"] > 0) / len(rows)
        if rows
        else None,
        "by_category": by_category,
    }


def percent(value: float | None) -> str:
    return "-" if value is None else f"{value * 100:.0f}%"


def main() -> None:
    args = parse_args()
    document = load_cases()
    cases = document["cases"]
    if args.case:
        cases = [case for case in cases if case["id"] == args.case]
        if not cases:
            fail(f"unknown case: {args.case}")
    if args.setup_only:
        run_dir = setup_run(cases)
        print(f"Setup complete: {run_dir}")
        return

    run_dir = Path(args.run).resolve()
    if not run_dir.is_dir():
        fail(f"run directory does not exist: {run_dir}")
    modes = {"name-only": False}
    if any(case.get("target_receivers") for case in cases):
        modes["receiver"] = True

    report: dict[str, Any] = {}
    for label, use_receivers in modes.items():
        rows = [score_case(run_dir, case, args.limit, use_receivers) for case in cases]
        summary = summarize(rows)
        report[label] = {"summary": summary, "cases": rows}

        print(f"\n{label} lookup")
        print(f"{'case':<26}{'ret':>5}{'ok':>5}{'confused':>10}{'truth':>7}{'prec':>7}{'recall':>8}")
        for row in rows:
            print(
                f"{row['id']:<26}{row['returned']:>5}{row['target_hits']:>5}"
                f"{row['confusions']:>10}{row['ground_truth']:>7}"
                f"{percent(row['precision']):>7}{percent(row['recall']):>8}"
            )
        print(
            f"overall precision {percent(summary['precision'])}, "
            f"recall {percent(summary['recall'])}, "
            f"cases with a confusion {percent(summary['confusion_rate'])}"
        )

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output = run_dir / f"results.{stamp}.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"\nWrote {output}")


if __name__ == "__main__":
    main()
