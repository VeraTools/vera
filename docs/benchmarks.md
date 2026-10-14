# Benchmarks

Vera's search quality is measured on a fixed, public benchmark with known correct answers, and every ranking change has to hold up on the full suite before it ships. This page has the current results and how they were produced. Older snapshots, model screenings, and rejected experiments are in [benchmark history](benchmarks-history.md).

## Current results

### Semble benchmark, full suite

The benchmark is the task set published with [Semble v0.5.5](https://github.com/MinishLab/semble/tree/v0.5.5): 1,251 natural-language and symbol queries across 63 pinned open-source repositories in 19 languages. Each task lists the code locations that answer it. Semble is a local code search tool from MinishLab, who also publish the `potion-code-16M-v2` embedding model, so their benchmark is a demanding home-turf comparison.

Both tools used the same embedding model, the same tasks, and the same nDCG@10 formula. See [Scoring](#scoring) for one difference in how answers were matched.

| Tool | nDCG@10 | Recall@1 | Recall@5 | Recall@10 | MRR | Query p50 | Index time | Index size |
|------|--------:|---------:|---------:|----------:|----:|----------:|-----------:|-----------:|
| Vera 2.0.1 | 0.8436 | 0.6711 | **0.9199** | 0.9502 | 0.8258 | 6.3 ms | 89 s | **4.7 GB** |
| Semble 0.5.5 | **0.8514** | **0.6747** | 0.9177 | **0.9656** | **0.8348** | 2.3 ms | 100 s | 32 GB |

How to read it:

- **Quality.** The two tools are within 1% on nDCG@10. Vera places a correct answer in the top five slightly more often; Semble has a small edge at rank 1 and rank 10.
- **Index size.** Vera's on-disk index for all 63 repositories is about a seventh the size of Semble's.
- **Speed.** Both answer a query in single-digit milliseconds at the median. Semble's query is faster because it searches vectors held in memory by a running process. Vera searches its on-disk index through memory mapping, so a new CLI process can search without loading the index first. Vera's p95 query latency is 59 ms.
- **Timing caveat.** The Vera row is the median of three full runs on 2026-10-09 on an AMD Ryzen 7 9800X3D, with shipped defaults (local `potion-code-16M-v2`, no reranker) and every index rebuilt each run. The Semble row comes from the 2026-08-23 comparison and was not re-measured on that machine, so the timing columns are not a direct comparison.

### Results outside the full suite

| Task set | Vera | Semble |
|---|---:|---:|
| 320-task tuning subset (16 repositories) | **0.8538** | 0.8494 |
| Independent set (180 tasks, 10 repositories) | **0.7674** | 0.7655 |

Scores are nDCG@10. The independent set is a contamination check: 10 repositories that are not in Semble's benchmark (celery, jinja, rayon, viper, lodash, ky, sidekiq, okhttp, zlib, nlohmann/json), with tasks written in the same style and checked answers. Because MinishLab built both the benchmark and the embedding model, this set checks that neither the model nor Vera's ranking only looks good on tasks it may have been tuned on. The two tools stay level on it.

### API backend

With the Qwen API pair (`qwen/qwen3-embedding-8b` embeddings and `qwen/qwen3-reranker-8b` reranking through OpenRouter), Vera scored 0.8647 nDCG@10 on the 320-task subset, compared with 0.8538 for the local default, and Recall@1 rose by three percentage points. This was measured on the subset only. See [models](models.md) for other backends.

## Agent benchmark

Retrieval scores say whether the right code comes back. The agent benchmark asks a different question: does a coding agent work more efficiently with Vera available?

Setup: 10 cross-file questions about the Flask codebase, each answered by a fresh coding agent (GLM-5.3, high reasoning effort) in four configurations. A judge model graded each answer blind against a verified answer key.

| Configuration | Score | Tool calls | Context used (prompt tokens) |
|---|---:|---:|---:|
| Vera, local Potion backend | 10.0/10 | 521 | 15.4 M (27% less) |
| Vera, Qwen API backend | 10.0/10 | 453 | **10.9 M (48% less)** |
| Semble CLI | 10.0/10 | 566 | 18.5 M (12% less) |
| No search tool (grep and file reads) | 10.0/10 | 598 | 21.0 M |

Every configuration answered every question correctly, so this run shows the difference in cost, not in answer quality. With 10 questions, only the Qwen backend's 48% reduction is statistically significant (paired t = -3.6, Wilcoxon p = 0.037); the other differences are within run-to-run noise at this sample size. Agents did not use the search tool on every question: they called Vera on 5 of 10 questions with the local backend and 8 of 10 with Qwen, and Semble on 5 of 10.

The question set, harness, per-question results, and two earlier runs on other models are in [benchmark history](benchmarks-history.md#agent-level-benchmark) and [`benchmarks/agent-bench/`](../benchmarks/agent-bench/README.md).

## Methodology

### Provenance

The task set is Semble v0.5.5 at commit `9218491`. The converter and corpus manifests pin that commit and record a hash of the task content, so a changed task file is detected. The 320-task tuning subset is an unchanged 16-repository slice of the same release.

### Scoring

Vera's harness scores results with graded relevance: primary answer locations count as relevance 2, secondary locations as 1, and a location matched by several chunks is credited once. Scores are averaged over tasks. Semble's own published tables use binary relevance averaged per repository and language, so their numbers are not directly comparable with the table above. The comparison on this page scores both tools with Vera's graded formula, with one difference in matching: a Vera result counts only if its file matches and its line range overlaps the answer's, while a Semble result counts if its file matches. In the full suite, 1,457 of the 1,491 answer locations cover a whole file, so the two rules differ only for the other 34.

### What the numbers do and do not show

- Queries against a fixed index return identical results. Rebuilding an index can reorder chunks with tied scores, so two fresh builds of the same code differ by about 0.0004 nDCG@10. Smaller differences are noise.
- Ranking changes must clear a 0.5% aggregate improvement on the full suite to ship. Experiments that missed that bar are recorded in [benchmark history](benchmarks-history.md#rejected-ranking-and-chunking-hypotheses-2026-09-01) and in the [architecture decisions](adr/000-decision-summary.md).
- Benchmark numbers show how tools compare on this workload. Your repository and hardware will produce different absolute values.

### Reproduce

```bash
bash eval/setup-semble-corpus.sh          # clone the 63 pinned repositories
cargo build --locked --release
cargo run --locked --release --bin vera-eval -- run \
  --tool vera-potion \
  --tasks-dir eval/tasks/semble \
  --corpus eval/semble-corpus.toml \
  --json-only
```

## Earlier comparisons

Before the Semble suite, Vera was measured on smaller in-house suites against other tools. These results predate Vera 1.0 and used different models, so they are history rather than current claims:

- On a 21-task suite over ripgrep, Flask, Fastify, and Turborepo, Vera 0.7.0 scored 0.836 nDCG@10 against 0.561 for ColGREP.
- On a 17-task suite over ripgrep, Flask, and Fastify, Vera's hybrid search scored 0.801 nDCG@10, against 0.521 for cocoindex-code, 0.708 for vector-only search, and 0.293 for ripgrep.

Full tables are in [benchmark history](benchmarks-history.md).
