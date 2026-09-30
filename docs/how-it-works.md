# How Vera Works

Vera's search pipeline retrieves candidates, fuses results, applies deterministic ranking, and optionally reranks. The [ADRs](adr/000-decision-summary.md) and [benchmark evidence](benchmarks.md) record the implementation choices.

## Parsing: Tree-Sitter Chunks

Vera parses source files into ASTs using tree-sitter grammars compiled into the binary. Instead of splitting code into arbitrary line ranges, it extracts discrete structural units such as functions, classes, structs, traits, interfaces, methods, and `impl` blocks.

Configuration formats such as JSON, YAML, and TOML use whole-file chunks; Markdown and reStructuredText use section chunks. The byte cap can split oversized chunks in either case. Module-level gaps between symbols are also kept as chunks when they carry useful retrieval context.

Each chunk carries metadata: file path, line range, language, symbol name, and symbol type. This means search results map to actual code boundaries, not random slices.

Large symbols are split at logical boundaries when a definition exceeds the 200-line chunk limit. The chunker splits it into multiple chunks that share the bare symbol name with a distinct part index. Storage keeps the bare name; display renders it as `name (part N)`. This preserves identity across split parts: `vera structural definitions` finds split symbols by bare name, `vera references` resolves their single call site, `vera dead-code` deduplicates parts by (symbol, file), and JSON output carries the bare name with a `part_index` field. Languages without a tree-sitter grammar fall back to sliding-window chunking. See [features.md](features.md#adaptive-chunking) for the byte cap and model input windows.

During parsing, Vera also records file-level diagnostics such as tree-sitter error nodes, Tier 0 fallback, and outright parse failures. `vera stats` surfaces these later as index-health signals instead of silently dropping them on the floor.

## Retrieval: BM25 + Vector Search

Two retrieval paths run in parallel for every query:

**BM25 (keyword matching)** uses a Tantivy index over structured chunk text, including content, symbol names, file paths, and filename/path tokens. It handles exact identifier and config-style lookups. Searching for `parse_config` finds that exact function.

**Vector search (semantic matching)** embeds the query and compares it against pre-computed chunk embeddings. Vera writes vectors to sqlite-vec and a flat sidecar; the sidecar is memory-mapped for exact SIMD distance, while `VERA_VECTOR_SCAN=vec0` selects sqlite-vec. Storage publication details, tombstones, rowid stability, fsync ordering, and manifest recovery are documented in [Architecture](architecture.md#vector-storage). This catches conceptual matches. Searching "authentication middleware" finds relevant auth code even if those exact words do not appear.

When a query carries path, glob, or language filters, the flat SIMD scan applies those filters during the scan via an eligibility map. Ineligible chunks are skipped before hydration instead of being hydrated and filtered afterward. This is enabled by default (`retrieval.vector_filter_during_scan`, env `VERA_VECTOR_FILTER_DURING_SCAN`). On the full 1,251-task Semble suite it halved filtered-query latency (p50 from about 12 ms to 6.4 ms) with ranking parity verified by an on/off differential suite.

Neither path alone is sufficient. BM25 misses semantic matches. Vector search misses exact identifiers and ranks poorly. Combining them covers both.

## Fusion: Reciprocal Rank Fusion

Results from both retrieval paths are merged using Reciprocal Rank Fusion (RRF). RRF scores each result based on its rank in each list:

```
score(d) = 1/(k + rank_bm25(d)) + 1/(k + rank_vector(d))
```

A result that ranks high in both lists gets a high fused score. A result that ranks high in only one list still appears, but lower. The constant `k` (default: 60) controls how much weight goes to top-ranked vs. lower-ranked results.

RRF combines rankings without requiring keyword and vector scores to share a scale or a trained fusion model.

## Query-Aware Ranking

After fusion, Vera applies lightweight deterministic ranking logic before final reranking.

Current ranking combines BM25 and vector results with RRF (`k=60`), then applies a file-coherence boost, keyword path boost, content coverage, content-symbol definition boost, and exact-match concept pool tail injection capped at 4 definitions per file.

Filename-stem boost, definition boost, and recall-pool expansion have individual controls in [Configuration](configuration.md#retrievalranking). [ADR 006](adr/006-ranking-signals.md) and [ADR 007](adr/007-ranking-hypotheses.md) record the accepted mechanisms and rejected experiments.

This stage handles cases that dense retrieval alone is bad at:

- exact filename and exact identifier queries
- path-heavy config lookups
- noisy test and docs matches
- broad natural-language queries that need structural results instead of tiny helpers
- same-file and cross-file answer completion

For broad intent queries, Vera also keeps a deeper fused candidate pool before final truncation. This matters when raw RRF pushes the right `struct` or `impl` block just outside the requested top N and a tiny helper would otherwise win by default.

This is also where Vera adds a small amount of query-aware candidate expansion, such as pulling in related implementation blocks or same-file structural context when the initial hit is too narrow.

## Search Scores

When a response includes `score`, it is a pipeline-specific ranking value and may be rank-normalized. Compact CLI JSON omits scores. Use the returned ordering. Scores are not probabilities and cannot be compared across queries.

## Reranking: Cross-Encoder

Reranking is opt-in through `retrieval.reranking_enabled` and is off by default. When enabled, the top fused candidates are sent to a cross-encoder reranker. Unlike embeddings (which encode query and document separately), the cross-encoder reads the query and each candidate together as a single pair, scoring relevance jointly.

The default no-reranker path ends with the deterministic ranking stage. The 2026-08-23 dual-set cross-encoder screening scored every tested reranker below that heuristic baseline. See [models.md](models.md#reranking) for the scores and the recommended local override.

With Jina ONNX local models, the reranker runs on-device via ONNX Runtime. With Potion Code embeddings, enabling reranking uses the local CPU reranker unless an API reranker is configured; when reranking is off, deterministic ranking is the final stage. With API mode, reranking calls your configured endpoint. Obvious filename and path-dominant queries can skip reranking when lexical evidence is already decisive.

Large candidate sets are batched automatically to stay within the reranker's request limits. Oversized documents are truncated at newline boundaries before scoring. See [features.md](features.md#cross-encoder-reranking) for configuration details.

API rerankers use an explicit wire protocol, `retrieval.reranker_protocol`, with `generic` (`top_n`/`results` payload) and `voyage` (`top_k`/`data` payload) variants. When unset, hostname auto-detection picks a sensible default and can be overridden. Related knobs include `retrieval.reranker_task_instruction` and `retrieval.reranker_task_field` for providers that accept a task instruction, `retrieval.reranker_return_documents`, and `retrieval.reranker_rate_limit_wait_secs` (env `VERA_RERANK_RATE_LIMIT_WAIT_SECS`). Positive wait values enable capped quota-reset waits; unset, `null`, and `0` keep short generic retries before degrading to unreranked results. Permanent 4xx errors are not retried. See [Configuration](configuration.md#retrievalranking) for defaults and precedence.

## Storage

Everything lives in two places:

- **`.vera/`** in the project root. SQLite metadata (chunks, file hashes, file-level index state), Tantivy BM25 index, sqlite-vec vectors, and the flat vector sidecar (`vectors.f32`, `vectors.tombs`, and `vectors.manifest`). One directory per project.
- **`$XDG_DATA_HOME/vera/models/`** (or `~/.vera/models/` on existing installs): cached local model assets. Downloaded once by `vera setup`.

The index is a SQLite database, a Tantivy directory, and the flat vector sidecar files. No external services, no daemons, no background processes.

## Incremental Updates

`vera update .` compares content hashes against stored metadata, re-processes only changed files, and refreshes the persisted file-level index state. That keeps parse failures and fallback counts visible across incremental runs. See [features.md](features.md#incremental-updates) for details.

## Pipeline Summary

```
Query
  ├─→ BM25 search (Tantivy)        ──→ ranked candidates
  └─→ Vector search (flat SIMD; sqlite-vec fallback) ──→ ranked candidates
                                          │
                                    RRF fusion
                                          │
                             query-aware ranking and expansion
                                          │
                                    top candidates
                                          │
                            optional cross-encoder rerank
                                          │
                                    final ranked results
```

| Stage | What it does | Why it matters |
|-------|-------------|----------------|
| Tree-sitter parsing | Extracts symbols as chunks | Results map to real code boundaries |
| BM25 | Exact keyword matching | Catches identifiers, fast |
| Vector search | Semantic similarity | Catches conceptual matches |
| RRF fusion | Merges both result lists | Covers both exact and semantic |
| Query-aware ranking | Applies deterministic priors and candidate shaping | Fixes exact-match, config, and cross-file failure modes |
| Optional cross-encoder rerank | Joint query-document scoring | Adds a second relevance signal when enabled |
