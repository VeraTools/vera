# Vera Benchmarks

This page holds the current Semble comparison and methodology. Older snapshots and ablations are in [Benchmark history](benchmarks-history.md).

## Provenance

Semble-derived runs use the task set from [Semble v0.5.5](https://github.com/MinishLab/semble/tree/v0.5.5) at commit `9218491`: 1,251 tasks across 63 pinned repositories. The converter and corpus manifests pin that commit and record a deterministic task-content hash. The 320-task tuning subset is an unchanged 16-repository slice of the same release.

Vera reports `vera-graded-2-1-task-mean-v1`: primary targets have relevance 2, secondary targets have relevance 1, duplicate matching chunks receive credit once, and scores are averaged over tasks. Historical Semble published tables use binary relevance and repository/language macro averages. The comparison below uses the same graded scorer for both tools. New JSON reports record both the Semble snapshot and metric contract; older reports without those fields are `unknown-legacy` and use Vera's graded calculation.

## Limits And Caveats

- The current release benchmark is deterministic and fully local, which makes it better for regression gating.
- Queries over a fixed index return identical results. Rebuilding indexes can reorder chunks with tied scores, so two fresh rebuilds of the same code differ by about 0.0004 nDCG@10 on the full suite. Treat smaller deltas as noise, and compare chunk and result output directly when a change should be ranking-neutral.
- The legacy public snapshot is still useful for older comparisons, but it should not be treated as the current retrieval baseline.
- Benchmark numbers in this repository show comparative behavior, not a promise that another machine or codebase will land on the same values.

## Current Results

### 2026-10-09 Semble Comparison (Vera row refreshed for v2.0.1)

The Semble benchmark uses 1,251 tasks across 63 repositories. Both tools used the same `minishlab/potion-code-16M-v2` embeddings, harness, graded relevance, and suffix-corrected path matching in the scorer.

| Tool | nDCG@10 | R@1 | R@5 | R@10 | MRR | Query p50 | Index time | Index size |
|------|---------|------|------|-------|-----|-----------|------------|------------|
| Vera v2.0.1 | 0.8436 | 0.6711 | **0.9199** | 0.9502 | 0.8258 | 6.3 ms | 89 s | **4.7 GB** |
| Semble 0.5.5, full rerank stack | **0.8514** | **0.6747** | 0.9177 | **0.9656** | **0.8348** | 2.3 ms | 100 s | 32 GB |

The Vera row is the median of three full-suite runs on 2026-10-09 at v2.0.1 on the AMD Ryzen 7 9800X3D host with shipped defaults (local potion-code-16M-v2 embeddings, no reranker), each rebuilding every index. The Semble row is unchanged from the 2026-08-23 comparison and was not re-measured on this host, so the timing columns are not bolded: compare them across tools loosely. On the same host, v2 indexes the suite 23% faster than v1.4.0 (89 s versus 115 s) with quality unchanged (v1.4.0: 0.8437 nDCG@10, 0.9189 R@5). Query p95 is 59 ms. Filter-during-scan in v1.4.0 halved filtered-query p50 from ~12 ms to 6.4 ms and p95 from ~139 ms to 65 ms (measured in-process by the eval harness) with nDCG parity within 0.000005.

On the 320-task tuning subset Vera scored `0.8538` versus Semble at `0.8494` nDCG; on the contamination-check independent set (10 repositories disjoint from Semble's 63, locally generated tasks) Vera scored `0.7674` versus Semble at `0.7655`. The aggregate table is the comparison summary; repo-level variation is not a substitute for the full result.

Vector search uses an exact flat SIMD scan over memory-mapped vectors, dual-written alongside sqlite-vec (`VERA_VECTOR_SCAN=vec0` selects the old path). On the largest corpus repo (zig, 336k chunks) the vector stage dropped from ~460 ms to ~21 ms per query; quality is unchanged because the scan is exact. Index size grew because vectors are stored in both backends during the transition.

Embedding alternatives and the dual-set reranker screening are documented in [models.md](models.md).

## Related Docs

- [Query-aware retrieval ADR](./adr/005-query-aware-retrieval.md)
- [Model selection and screening](./models.md)

- [Benchmark history](benchmarks-history.md)
