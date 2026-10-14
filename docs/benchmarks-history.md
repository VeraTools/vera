# Benchmark history

This page archives benchmark runs that are no longer the current comparison: older version snapshots, model screenings, rejected experiments, and benchmarks from before Vera 1.0. The numbers are kept as they were measured, so you can see how Vera got where it is and why some ideas were dropped. For current results, see [Benchmarks](benchmarks.md#current-results).

A few terms used below:

- **Lane**: one named configuration of the evaluation harness (`vera-eval`), such as keyword-only search or a specific embedding model. Each lane runs the same tasks and is scored the same way.
- **Arm**: one condition in an A/B experiment, such as "with Vera" versus "control".
- **Ablation**: a run with one change turned on and one with it turned off, everything else equal, to measure what that single change contributes.

Unless a section says otherwise, scores use Vera's graded scoring described in [benchmark methodology](benchmarks.md#provenance), and "full suite" means the 1,251-task Semble v0.5.5 task set across 63 repositories.

## RRF constant sweep (2026-10-14)

This run checked whether the rank-fusion constant k, which controls how keyword and vector rankings are merged, should change from its default of 60. It used the full suite with v2.1.0 defaults (local potion-code-16M-v2, no reranker) and reused one index build for every value of k. Before this run, ordinary search ignored `retrieval.rrf_k` and always used k=60.

| k | nDCG@10 | R@1 | R@5 | R@10 | MRR |
|---|---------|------|------|-------|-----|
| 5 | 0.8441 | 0.6739 | 0.9175 | 0.9510 | 0.8270 |
| 10 | 0.8436 | 0.6719 | 0.9175 | 0.9510 | 0.8260 |
| 30 | 0.8443 | 0.6723 | 0.9187 | 0.9510 | 0.8269 |
| 60 (default) | 0.8435 | 0.6707 | 0.9195 | 0.9506 | 0.8255 |
| 120 | 0.8445 | 0.6719 | 0.9211 | 0.9522 | 0.8266 |

Every value lands within 0.001 nDCG@10 of k=60, far below the 0.5% improvement a ranking change needs to ship, so the default stays. The constant matters little because later ranking stages use only the fused positions plus code-aware priors.

## Vera v1.4.0 Semble row (2026-09-03)

This was the Vera row in the main comparison until the v2.0.1 measurement replaced it. It was measured on the AMD Ryzen 7 9800X3D machine with shipped defaults (local potion-code-16M-v2 embeddings, no reranker), as three interleaved runs of the full suite. For reference, the v1.2.0 row scored 0.8450 nDCG@10 on an earlier CPU.

| Tool | nDCG@10 | R@1 | R@5 | R@10 | MRR | Query p50 | Index time | Index size |
|------|---------|------|------|-------|-----|-----------|------------|------------|
| Vera v1.4.0 | 0.8437 | 0.6713 | 0.9189 | 0.9502 | 0.8258 | 6.4 ms | 115 s | 4.7 GB |

## Rejected Ranking and Chunking Hypotheses (2026-09-01)

Three ranking and chunking ideas were each implemented behind a switch and measured as ablations on the full suite (Semble v0.5.5 at `921849164e2632dd4f0e1c1370f82cfe15ed6d6c`), with local Potion Code v2 at revision `e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b`, no reranker, on an AMD Ryzen 7 9800X3D. Vera was at commit `6e01956fb644b9cce2d96d7e799b0b0798855c46`. Every run rebuilt its indexes. The 320-task subset and the 180-task independent set were measured too; [the measurement report](https://github.com/VeraTools/vera/issues/196#issuecomment-5486933078) has those numbers and the full provenance.

| Hypothesis | Full nDCG without | Full nDCG with | Relative delta | Index time without → with | Storage without → with | Decision |
|---|---:|---:|---:|---:|---:|---|
| Multiplicative 0.3× path penalty | 0.843879 | 0.841765 | -0.251% | 110.37 s → 111.95 s | 4.69 GB → 4.69 GB | Rejected: regresses |
| Uniform 5× candidate pool | 0.843480 | 0.846349 | +0.340% | 106.74 s → 106.64 s | 4.69 GB → 4.69 GB | Rejected: below 0.5% bar |
| 750-character chunks | 0.843937 | 0.841982 | -0.232% | 105.74 s → 154.28 s | 4.69 GB → 6.55 GB | Rejected: regresses with higher cost |

The 750-character chunks also raised full-suite query latency, from 11.37 ms to 18.96 ms at p50 and from 128 ms to 287 ms at p95. Earlier chunk-size experiments had failed the same way: a 2048-token Jina window lowered nDCG by 0.23% for 88% more indexing time, and a 2048-byte Potion cap lowered full-suite nDCG by 0.21% for 24% more indexing time and 15% more storage.

All three switches stayed off and were removed in v2.0.0. [ADR 007](adr/007-ranking-hypotheses.md) explains the reasoning behind each idea and the compatibility decision. Removing the unused code did not change search quality, and these results do not claim that it did.

## Historical v1.0 full-pipeline benchmark

These results describe Vera around the 1.0 release, when the default setup used local Jina ONNX models on a GPU. The v1.0.0 release candidate was measured on 2026-08-16 on the full suite in the `vera-cuda` lane: hybrid keyword and vector search, rank fusion, and a local ONNX cross-encoder reranker on CUDA. The older "BM25 scoped filters" full-suite row (0.7267 nDCG@10, measured in May 2026) was a keyword-only lane run on an older harness. Results from different dates used different versions and settings and are not a controlled comparison.

### Model lanes and partial runs

`vera-eval` still has the historical lanes `vera-bm25`, `vera-cuda`, `vera-cpu`, and `vera-potion`. Use `--task-id` and `--category` to run a subset of tasks without editing the task files or answer keys:

```bash
cargo run -p vera-eval -- run --tool vera-bm25 \
  --task-id symbol-lookup-001,symbol-lookup-002 \
  --category symbol_lookup --json-only
```

To test candidate models, pass a JSON or TOML file with one or more lanes (`--lane-spec`). Each lane records its backend, model source, pooling, prefixes, dimensions, maximum length, whether it reranks, and an optional revision. The evaluator writes one report per lane, or a JSON array when you request more than one.

JSON example:

```json
{
  "lanes": [
    {
      "name": "harrier-270m",
      "backend": "custom-onnx",
      "repo": "onnx-community/harrier-oss-v1-270m-ONNX",
      "onnx_file": "onnx/model.onnx",
      "onnx_data_file": "onnx/model.onnx_data",
      "tokenizer_file": "tokenizer.json",
      "pooling": "last-token",
      "query_prefix": "Instruct: Given a code search query, retrieve relevant code passages that answer the query\nQuery:",
      "dim": 640,
      "max_length": 512,
      "rerank": false,
      "revision": "d59c919d0159aea2c19ed7d04288fcdd048d0f9c"
    },
    {
      "name": "potion-control",
      "backend": "potion",
      "rerank": false
    }
  ]
}
```

Lane fields:

- The `custom-onnx` backend runs on the CPU by default. Set `execution_provider` to `cuda`, `rocm`, `coreml`, `openvino`, or another supported provider to change that.
- `repo` downloads a model from Hugging Face; `dir` points at a model directory you already have.
- `revision` pins a Hugging Face download to a commit SHA (omitted means `main`). Pinned models are cached under `models/<repo>/revisions/<revision>/` and the revision is part of the model identity, so an index built with one revision is never reused for another. `revision` works only on Hugging Face `repo` lanes, not on `dir`, `bm25`, `api`, or `potion` lanes.
- API lanes take `model_id`, `query_prefix`, and `environment` for endpoint settings.

Every report records the resolved lane settings, the Vera Git SHA, a timestamp, the command, a redacted summary of the environment, the corpus SHAs, and a SHA-256 of the selected task IDs.

The older Python runner accepts the same task selection for its retrieval modes:

```bash
python3 benchmarks/scripts/run_vera_benchmarks.py \
  --modes bm25-only hybrid-norerank \
  --task-id intent-001,intent-002 --category intent --skip-index
```

### Historical embedding model screening (2026-08-21)

This screening compared embedding models to choose a new default; it led to potion-code-16M-v2 and predates the current comparison. Every lane ran hybrid keyword and vector search with the reranker off, so the table isolates the embedding model. Each model was pinned to a commit SHA (see `revision` above). Measured at Vera commit `69bc514` on one CUDA GPU.

Subset (320 tasks, 16 repos):

| Lane | nDCG@10 | Recall@1 | Recall@5 | Recall@10 | MRR | p50 ms | p95 ms | Index s | Storage |
|------|---------|----------|----------|-----------|-----|--------|--------|---------|---------|
| BM25 (no embeddings) | 0.7792 | 0.5911 | 0.8609 | 0.9141 | 0.7683 | 5.0 | 17.1 | 12 | 323 MB |
| Potion Code 16M | 0.7656 | 0.5786 | 0.8547 | 0.9000 | 0.7508 | 13.2 | 57.6 | 17 | 418 MB |
| potion-code-16M-v2 (API) | 0.7930 | 0.5990 | 0.8812 | 0.9328 | 0.7769 | 13.5 | 56.8 | 56 | 419 MB |
| F2LLM-v2-80M (API) | 0.7897 | 0.6068 | 0.8875 | 0.9141 | 0.7774 | 20.3 | 62.0 | 163 | 419 MB |
| F2LLM-v2-160M (API) | 0.7932 | 0.6021 | 0.8859 | 0.9234 | 0.7807 | 29.7 | 103.5 | 300 | 609 MB |
| F2LLM-v2-330M (API) | 0.7971 | 0.6052 | 0.8938 | 0.9328 | 0.7805 | 31.3 | 90.8 | 427 | 609 MB |
| jina v5 nano | 0.7956 | 0.6005 | 0.8859 | 0.9328 | 0.7788 | 44.9 | 148.6 | 147 | 672 MB |
| CodeRankEmbed 137M | 0.7949 | 0.6021 | 0.8875 | 0.9328 | 0.7774 | 21.3 | 84.7 | 375 | 670 MB |
| Harrier 270M | 0.7938 | 0.6068 | 0.8813 | 0.9281 | 0.7799 | 24.2 | 77.4 | 2391 | 609 MB |
| Harrier 0.6B fp32 | 0.7971 | 0.5974 | 0.8906 | 0.9422 | 0.7791 | 27.4 | 94.3 | 7082 | 800 MB |
| BitNet-270M i2_s (API) | 0.7743 | 0.5849 | 0.8656 | 0.9062 | 0.7620 | 51.0 | 124.5 | 18061 | 607 MB |

Full suite (1,251 tasks, 63 repos):

| Lane | nDCG@10 | Recall@1 | Recall@5 | Recall@10 | MRR | p50 ms | p95 ms | Index s |
|------|---------|----------|----------|-----------|-----|--------|--------|---------|
| jina v5 nano | 0.7452 | 0.5560 | 0.8320 | 0.8846 | 0.7184 | 22.9 | 128.8 | 1823 |
| F2LLM-v2-330M (API) | 0.7425 | 0.5564 | 0.8343 | 0.8778 | 0.7164 | 57.9 | 222.4 | 3446 |
| CodeRankEmbed 137M | 0.7401 | 0.5512 | 0.8311 | 0.8802 | 0.7123 | 27.0 | 124.5 | 3308 |
| potion-code-16M-v2 (API) | 0.7388 | 0.5516 | 0.8296 | 0.8766 | 0.7129 | 16.8 | 84.6 | 443 |

What it showed:

- Apart from BitNet and the older Potion Code 16M, every model scored within 0.008 nDCG@10 of the others on both task sets (0.7897 to 0.7971 on the subset, 0.7388 to 0.7452 on the full suite). Code-specialized models showed no advantage with embeddings truncated at 512 tokens, as Vera did at the time.
- BitNet-270M scored below keyword-only search. Its 1.58-bit GGUF also loads only in the microsoft/BitNet fork (upstream llama.cpp rejects its tensor types, and the fork's CUDA path fails an assertion), so it was ruled out on both counts.
- Because quality was so close, the choice came down to license and operating cost. Harrier's indexing times (16 and 48 times Jina's on the subset) ruled out both sizes as a default.

Precision and serving notes:

- Jina ran its fp16 export on the GPU (the automatic switch from the quantized model to fp16). CodeRankEmbed and Harrier ran in fp32: CodeRankEmbed has no fp16 export, and Harrier's fp16 export uses GroupQueryAttention with attention bias, which the ONNX Runtime CUDA kernel rejects.
- Harrier 0.6B fp32 stores its weights in several files (`model.onnx_data_1`), which Vera's downloader did not support at the time, so it ran from a pinned local directory.
- Rows marked `(API)` are models Vera cannot run natively. They were served by a local OpenAI-compatible server and measured through the `api` lane: potion-code-16M-v2 (model2vec, CPU) and the F2LLM-v2 family (transformers, bf16 on CUDA, truncated at 512 tokens to match the other lanes), each pinned to an upstream commit (e9d2a44; 19a4fd85, 0e04993a, 1b8f0301).
- BitNet-270M ran on the microsoft/BitNet fork's `llama-server` (0b341e58) on the CPU, with the i2_s GGUF pinned at 5f1c2fd. A proxy in front truncated inputs to 510 tokens (the server errors instead of truncating) and restarted the server when it crashed under concurrent requests.

The raw output of these screening runs was not kept; they predate the current rules for keeping benchmark artifacts, so the tables above are the record.

### Independent Contamination Check (2026-08-22)

MinishLab publishes both potion-code-16M-v2 and the Semble benchmark, so a good score on Semble's tasks alone cannot rule out that the model was tuned on them. This check reran the leading candidates on a separate task set: 10 pinned repositories that are not among Semble's 63 (celery, jinja, rayon, viper, lodash, ky, sidekiq, okhttp, zlib, nlohmann/json), with 180 Semble-style tasks written for this check (100 intent, 50 cross_file, 30 symbol_lookup) and verified file-level answers. Harness, reranker setting (off), and model revisions were the same as in the screening above.

| Lane | nDCG@10 | Recall@1 | Recall@5 | Recall@10 | MRR | p50 ms | Index s |
|------|---------|----------|----------|-----------|-----|--------|---------|
| jina v5 nano | 0.7149 | 0.3889 | 0.7972 | 0.8750 | 0.7148 | 13.3 | 61 |
| CodeRankEmbed 137M | 0.7069 | 0.4000 | 0.7731 | 0.8546 | 0.7212 | 14.8 | 153 |
| F2LLM-v2-330M (API) | 0.7061 | 0.3944 | 0.7704 | 0.8574 | 0.7178 | 21.6 | 167 |
| potion-code-16M-v2 (API) | 0.7019 | 0.3935 | 0.7741 | 0.8574 | 0.7050 | 10.1 | 26 |
| BM25 (no embeddings) | 0.6506 | 0.3630 | 0.7278 | 0.7852 | 0.6682 | 3.3 | 5 |

What it showed:

- The order from the Semble screening held: Jina first, then CodeRankEmbed, F2LLM-330M, and potion-v2 within about 0.005 of each other, with keyword-only search far behind.
- If potion-v2 had an unfair edge on Semble's tasks, it would look relatively weaker here. Instead its gap to Jina grew from -0.0026 on the Semble subset to -0.0130 on these repositories, which is not the pattern a home-field advantage produces.
- Every neural model scored 0.06 to 0.09 lower in absolute terms on this set (smaller repositories, LLM-written queries), and the margin of neural models over keyword-only search roughly tripled. Leaked tasks would push the other way.
- potion-v2 remained the best candidate with a permissive license and indexed 6 times faster than the next models.

The first Jina run accidentally had the reranker on (the old bare `--tool vera-cuda` lane enables reranking, while the screening lane sets `rerank: false`). It scored 0.7309, with the difference coming from the reranker, and its p50 was about 2.6 s because every query was reranked. The table shows the corrected run without reranking.

The raw output was not kept, for the same reason as the screening runs.

### Historical main results

The v1.0.0 release candidate compared with the codebase before the release ablations, both on the full suite:

| Variant | nDCG@10 | Recall@1 | Recall@5 | Mean search latency |
|---------|---------|----------|----------|---------------------|
| Baseline (pre-ablation HEAD) | 0.7209 | 0.5336 | 0.8088 | 310 ms |
| v1.0.0-rc (C1+C2 merged) | 0.7327 | 0.5476 | 0.8144 | 2421 ms |

### Ablations

Each change below was measured on the full suite against the stated base.

| Ablation | What it changes | nDCG delta | R@1 delta | Latency effect | Decision |
|----------|-----------------|------------|-----------|----------------|----------|
| C1: rerank no-surplus skip | Skip reranking when the fused pool has no surplus over the requested limit | -0.0002 | +0.0000 | -2.4 ms mean | Shipped |
| C2: rerank path-glob searches | Always rerank path-scoped searches (removes the old skip heuristic) | +0.0113 | +0.0132 | scoped queries now pay rerank cost | Shipped |
| GA: structural graph augmentation | Append bounded caller/implementation chunks into the rerank pool (`VERA_GRAPH_AUGMENT=1`) | +0.0047 | +0.0080 | +2018 ms mean (reranks the expanded pool) | Rejected: gain too small for the latency, R@5 did not move; implementation removed in v2.0.0 |

Artifacts:

- [2026-08-16-vera-cuda-v1-full.json](../benchmarks/results/semble/2026-08-16-vera-cuda-v1-full.json) (shipped configuration, full suite)

The result files for the baseline, C1, C2, and GA runs were removed from the tree and remain in git history.

### Agent-Level Benchmark

This benchmark measures whether coding agents answer questions more cheaply with Vera available. It used 10 cross-file tracing questions about the Flask codebase; the questions and harness are documented in [benchmarks/agent-bench/README.md](../benchmarks/agent-bench/README.md). A fresh `droid exec` agent answered each question in up to four arms:

- `with-vera`: a Vera index and the Vera agent skill installed.
- `with-vera-qwen`: the same setup using the Qwen API embedding and reranker models.
- `with-semble`: a Semble index and instructions for the Semble CLI.
- `control`: both tools blocked (they exit with status 127), leaving grep and file reads.

A judge model graded each answer blind against a verified answer key, on a 0 to 10 scale per question. Tool calls, tokens, and wall time came from the agent's own event stream.

The first two runs (claude-opus-5 and kimi-k3, medium reasoning effort) compared `with-vera` against `control`:

| Tested model | Arm | Mean score | Tool calls | Input tokens | Wall time |
|--------------|-----|------------|------------|--------------|-----------|
| claude-opus-5 (medium effort) | with-vera | 10.0/10 | 186 | 298 | 1367 s |
| claude-opus-5 (medium effort) | control | 10.0/10 | 173 | 212 | 1252 s |
| kimi-k3 (medium effort) | with-vera | 10.0/10 | 219 | 230,567 | 1312 s |
| kimi-k3 (medium effort) | control | 9.9/10 | 190 | 278,041 | 1198 s |

The input-token column counts only tokens that were not served from cache; for claude-opus-5 nearly everything was a cache read, which is why its numbers are so small. On that uncached count, kimi-k3 used 17% fewer tokens with Vera (230.6k vs 278.0k). On total prompt tokens (uncached input plus cache reads, the measure used in the four-arm table below), kimi-k3 used 15% more with Vera (3.21M vs 2.78M), with two questions (q09 and q10) causing all of the overrun, and claude-opus-5 used 58% more (4.94M vs 3.13M; lower on only 3 of 10 questions). Both runs used an older binary and an earlier integration (the skill only, with no `AGENTS.md` snippet and no isolated agent home directory), so they say nothing about the current setup. Treat them as history, not as evidence across models.

A later four-arm run with GLM-5.3 (high reasoning effort) added the Qwen and Semble arms. At that effort level GLM-5.3 answers every question correctly with grep alone, so all four arms scored 10.0 on every valid question and the score cannot tell them apart. What does differ is how much context each arm used and how often the agent called the search tool. Prompt tokens here are uncached input plus cache reads, which this setup reports separately:

| Arm | Score | Tool calls | Prompt tokens | Vera/Semble activations | Wall |
|-----|-------|------------|---------------|-------------------------|------|
| with-vera (local Potion) | 10.0/10 | 521 | 15.39 M (-27%) | 10 in 5 cells | 6.9 h |
| with-vera-qwen (Qwen API pair) | 10.0/10 | 453 | **10.91 M (-48%)** | 16 in 8 cells | 6.7 h |
| with-semble (Semble CLI) | 10.0/10 | 566 | 18.49 M (-12%) | 5 in 5 cells | 8.5 h |
| control (neither) | 10.0/10 | 598 | 21.00 M | 0 | 7.7 h |

A "cell" is one question answered in one arm, and "activations" counts calls to `vera` or `semble`.

What it showed:

- Quality was equal, and both Vera arms used less context than the Semble arm or the control. With the Qwen embedding and reranker models, context use dropped by almost half.
- Agents chose when to search. They read the tool instructions, called `vera` on 5 of 10 questions with the local backend and 8 of 10 with Qwen, called Semble on 5 of 10, and skipped both on tracing questions where following the file structure was cheaper.
- Across the 10 paired questions, only the Qwen arm's 48% reduction is statistically significant (paired t = -3.6, bootstrap 95% CI -61.8% to -12.9%, Wilcoxon p = 0.037). The confidence intervals for the local arm's -27% and Semble's -12% include zero at this sample size.

Per-question scores and token counts are in [`benchmarks/agent-bench/results/results.glm-5.3-free-high.json`](../benchmarks/agent-bench/results/results.glm-5.3-free-high.json).

Limitations: 10 questions, 1 repository, 1 run per cell, and no statistical power analysis. This model and effort level already answer every question correctly, so comparing answer quality would need harder questions, a lower effort budget, or tighter time limits where search efficiency matters.

## Historical v0.7.0 benchmark

Before the Semble suite, Vera was measured on a small in-house suite. This is the benchmark used for the v0.7.0 retrieval pipeline:

- 21 tasks
- 4 repos: `ripgrep`, `flask`, `fastify`, `turborepo`
- local Jina embedding and reranker models
- CUDA ONNX backend
- the same pinned repositories and the same harness for every version below

### Accuracy improvements from `v0.4.0` to `v0.7.0`

| Version | Recall@1 | Recall@5 | Recall@10 | MRR@10 | nDCG@10 |
|--------|----------|----------|-----------|--------|---------|
| `v0.4.0` | 0.2421 | 0.5040 | 0.5159 | 0.5016 | 0.4570 |
| `v0.5.0` | 0.3135 | 0.5635 | 0.6349 | 0.5452 | 0.5293 |
| `v0.7.0` | **0.7183** | **0.7778** | **0.8254** | **0.9095** | **0.8361** |

From `v0.4.0` to `v0.7.0`, Vera improved by:

- `+0.4762` Recall@1
- `+0.2738` Recall@5
- `+0.3095` Recall@10
- `+0.4079` MRR@10
- `+0.3791` nDCG@10

The per-version result files were removed from the tree and remain in git history.

### Historical performance snapshot

`v0.7.0` with local Jina models on CUDA ONNX:

| Measure | Result |
|---------|--------|
| Search latency p50 | `3716 ms` |
| Search latency p95 | `4185 ms` |

### Recent local tuning loop

These runs tuned retrieval after `v0.7.0`. They were used to catch regressions and are not part of the release snapshot above.

Method:

- `benchmarks/scripts/run_local_binary_benchmarks.py` on the same 21-task, 4-repo suite
- model paths forced on CUDA, so quantized and fp16 runs could not silently switch models
- changes judged on the whole suite, not on how often Vera was used or on a single missed task

The result files from these runs were removed from the tree; see git history for the originals.

The fix to the candidate pool, compared with fp16 before the fix:

| Metric | Pre-fix fp16 | Current fp16 |
|--------|--------------|--------------|
| Recall@1 | 0.7183 | **0.7659** |
| Recall@5 | 0.8254 | **0.8968** |
| Recall@10 | 0.8254 | **0.8968** |
| MRR@10 | 0.9206 | **0.9683** |
| nDCG@10 | 0.8425 | **0.9027** |

What changed:

- `intent-004` (`file type detection and filtering`) went from a miss to a perfect hit by returning `crates/ignore/src/types.rs:224-301` instead of a small helper method.
- `cross-file-002` improved because the deeper pool kept the second relevant blueprint registration chunk long enough to rank it.
- No task got worse in the fp16 rerun of the whole suite.

Cost:

- Search latency rose in the fp16 tuning run (`p50 4103 ms`, `p95 10772 ms`).
- Most of the extra time came from broad intent queries, which now searched a deeper candidate pool before truncation.

Quantized model notes:

- The full forced-quantized 21-task rerun completed and matched the fp16 scores on this suite.
- On that machine, the quantized model searched slightly faster (`p50 3617 ms` vs `4103 ms`) but indexed more slowly.
- The original failure was a large `turborepo` embedding batch that hit a CUDA ONNX memory spike inside `MultiHeadAttention`. Vera now retries such local batches at smaller sizes instead of stopping the index.
- The sequence-aware batch scaler kept the same scores as `oom-fix-jina-cuda-onnx-quantized-embed` and cut quantized indexing time on every repository in the same run (`ripgrep 13.08s -> 12.96s`, `fastify 15.24s -> 14.57s`, `turborepo 55.28s -> 54.71s`, `flask 6.37s -> 5.82s`).
- The scaler saves what it learns about GPU batch sizes in `adaptive-batch-scaler.json` in the Vera data directory. When you compare cold indexing speed, delete that file first or run every candidate from the same warmed state.

### Historical Semble comparison

These are the first runs on Semble v0.5.5's task set, from May 2026. Rows labeled Semble come from Semble's own harness; rows labeled Vera use Vera's graded scoring described in [methodology](benchmarks.md#provenance).

**320-task subset** (16 repos, used while tuning):

| Tool | Backend | Recall@1 | Recall@10 | MRR | nDCG@10 | Search p50 | Search p95 | Index time |
|------|---------|----------|-----------|-----|---------|------------|------------|------------|
| Semble | Potion Code CPU | **0.6630** | **0.9479** | **0.8223** | **0.8311** | **1.43 ms** | **15.41 ms** | 26.06 s |
| Vera | BM25 scoped filters | 0.5802 | 0.9172 | 0.7618 | 0.7752 | 5.05 ms | 23.33 ms | 10.40 s |
| Vera | BM25 ranked (v4) | 0.5792 | 0.8781 | 0.7520 | 0.7567 | 2.92 ms | 10.37 ms | **10.28 s** |
| Vera | Potion Code CPU (v5) | 0.5792 | 0.8797 | 0.7490 | 0.7550 | 10.11 ms | 41.08 ms | 16.62 s |
| Vera | Potion Code CPU (v4) | 0.5792 | 0.8797 | 0.7490 | 0.7550 | 10.68 ms | 49.14 ms | 16.72 s |
| Vera | BM25 ranked (v3) | 0.5573 | 0.8750 | 0.7376 | 0.7477 | 2.92 ms | 10.37 ms | 10.28 s |
| Vera | Potion Code CPU (v3) | 0.5510 | 0.8891 | 0.7340 | 0.7468 | 13.95 ms | 54.26 ms | 16.40 s |
| Vera | Jina CUDA ONNX | 0.5276 | 0.8578 | 0.7058 | 0.7233 | 23.50 ms | 6236.60 ms | 151.20 s |

The version labels are tuning iterations, not releases:

- v3 added English stemming and matching concepts to file names.
- v4 added a stronger definition boost, definition detection from content, extraction of embedded symbols, proportional stem matching, and stronger penalties for noisy results. Potion v4 also ran keyword and embedding search in parallel, which cut p50 by 24%.
- v5 kept v4's scores and reduced vector over-fetching from 2 times to 1.5 times the candidate pool, which lowered Potion CPU p95 latency on the subset.

**Full 1,251-task Semble suite** (63 repos, required before claiming parity):

| Metric | Vera BM25 (v3) | Vera BM25 (v4) | Vera BM25 scoped filters |
|--------|---------------:|---------------:|-------------------------:|
| nDCG@10 | 0.7010 | 0.7074 | **0.7267** |
| Recall@1 | 0.5357 | **0.5449** | 0.5448 |
| Recall@5 | 0.7748 | 0.7832 | **0.8144** |
| Recall@10 | 0.8128 | 0.8160 | **0.8654** |
| MRR | 0.6857 | 0.6943 | **0.7043** |

v4 nDCG by category: symbol_lookup 0.8944, intent 0.6987 (+0.0087), cross_file 0.6141 (+0.0051). Every category improved, with 100 tasks better and 72 worse across the full suite.

"Scoped filters" means keyword search pulls a larger candidate pool from Tantivy only when search filters are active, then keeps the chunks that match before final ranking. On the full suite, tasks with no correct answer in the top 10 dropped from 188 to 139. Recall@10 improved on 82 tasks and got worse on 4. Search p95 rose from 24.96 ms to 54.94 ms.

The Jina CUDA run loads the CUDA build of ONNX Runtime through `ORT_DYLIB_PATH`. Do not compare its latency against a run that used the CPU build.

The May result files were removed from the tree and remain in git history.

### Optional CodeRankEmbed Preset

This short check, with reranking off so differences between embedding models show directly, decided whether CodeRankEmbed was worth offering as a built-in local embedding preset:

- 6 tasks
- 2 repos: `flask`, `ripgrep`
- local CUDA ONNX backend
- reranking disabled

| Model | Recall@1 | Recall@5 | Recall@10 | MRR | nDCG | Search p50 | Flask index | Ripgrep index |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Jina preset | 0.5556 | 0.5556 | 0.5556 | 0.8462 | 0.6442 | 761.9 ms | 5.8 s | 11.9 s |
| CodeRankEmbed preset | 0.7222 | 0.7222 | 0.7222 | 1.0000 | 0.8108 | 611.4 ms | 14.7 s | 29.1 s |

Six tasks is a small sample. The current embedding comparison is in [models](models.md#embedding-screening).

## Vera vs ColGREP

ColGREP is a code search tool built on late interaction, where the query and the code are matched token by token. These ColGREP numbers were recorded on the same 21-task, 4-repo suite as Vera v0.7.0, and the comparison helped guide work on Vera's ranking and chunk selection.

| Metric | Vera `v0.7.0` | ColGREP (149M) | ColGREP Edge (17M) |
|--------|---------------|----------------|--------------------|
| Recall@1 | **0.7183** | 0.5710 | 0.5240 |
| Recall@5 | **0.7778** | 0.6670 | 0.5710 |
| Recall@10 | **0.8254** | 0.7140 | 0.7140 |
| MRR@10 | **0.9095** | 0.6170 | 0.5660 |
| nDCG@10 | **0.8361** | 0.5610 | 0.5240 |

Indexing time, 4 repos combined:

| Tool | Total time | Hardware |
|------|-----------|----------|
| Vera `v0.7.0` | `~70 s` | RTX 4080 |
| ColGREP (149M, CPU) | `~180 s` | Ryzen 5 7600X3D 6c/12t |
| ColGREP Edge (17M, CPU) | `~160 s` | Ryzen 5 7600X3D 6c/12t |

Vera ran on a GPU and ColGREP on a CPU, so the indexing times are not a like-for-like comparison.

## Legacy public API benchmark

This is the oldest published snapshot, still cited in some older docs and release notes:

- 17 tasks
- 3 repos: `ripgrep`, `flask`, `fastify`
- a mix of API and local model runs

### Retrieval quality

| Metric | ripgrep | cocoindex-code | vector-only | Vera hybrid |
|--------|---------|----------------|-------------|-------------|
| Recall@1 | 0.1548 | 0.1587 | 0.0952 | **0.4265** |
| Recall@5 | 0.2817 | 0.3730 | 0.4921 | **0.6961** |
| Recall@10 | 0.3651 | 0.5040 | 0.6627 | **0.7549** |
| MRR@10 | 0.2625 | 0.3517 | 0.2814 | **0.6009** |
| nDCG@10 | 0.2929 | 0.5206 | 0.7077 | **0.8008** |

### Local vs API models

On this 17-task benchmark, the local Jina models came close to the much larger Qwen3-Embedding-8B API model:

| Metric | Jina local (ONNX) | Qwen3-8B (API) |
|--------|-------------------|----------------|
| MRR@10 | **0.68** | 0.60 |
| Recall@5 | 0.65 | **0.73** |
| Recall@10 | 0.73 | **0.75** |
| nDCG@10 | 0.72 | **0.81** |

### Performance snapshot

From the same benchmark:

| Measure | Result |
|---------|--------|
| BM25-only search p95 | `3.5 ms` |
| Hybrid search p95 | `6749 ms` |
| `ripgrep` index time | `65.1 s` |
| `flask` index time | `20.2 s` |
| `fastify` index time | `41.8 s` |

## Related docs

- [Current benchmark results](benchmarks.md#current-results)
- [Models and backends](models.md)
