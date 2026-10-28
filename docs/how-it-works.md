# How Vera works

```text
                ┌─ BM25 keyword search (Tantivy) ─┐
query ──────────┤                                 ├─ rank fusion ─ optional reranker ─ code-aware ranking ─ results
                └─ vector search (embeddings) ────┘
```

Vera turns a repository into symbol-sized chunks, indexes each chunk two ways, and answers a query by combining both indexes and then ranking the result with rules written for code. This page walks through each stage. The reasoning and measurements behind each choice are in the [architecture decisions](adr/000-decision-summary.md) and [benchmarks](benchmarks.md).

## Parsing: chunks that follow the code

Vera parses each file with a tree-sitter grammar compiled into the binary and cuts it along symbol boundaries: functions, methods, classes, structs, traits, interfaces, `impl` blocks, and so on. Each chunk records its file path, line range, language, symbol name, and symbol type, so a result points at a real unit of code rather than an arbitrary slice of lines.

- Code between symbols (imports, module-level constants) is kept as its own chunk when it carries useful context.
- Symbols longer than 200 lines are split at natural boundaries. The parts keep the symbol's name with a part number, so `vera structural definitions` and `vera references` still treat them as one symbol.
- Config files (JSON, YAML, TOML, and similar) are indexed as whole files. Markdown and reStructuredText are split by section. Formats without a grammar fall back to line windows.
- Any chunk larger than 24 KB is split at line boundaries.

While parsing, Vera also records call sites, type relationships (`implements`, `extends`, trait impls), and per-file parse health. That data powers `vera references`, `vera structural`, `vera dead-code`, and the health section of `vera stats`.

## Two indexes per chunk

**Keyword index (BM25).** A Tantivy full-text index over chunk content, symbol names, and file paths, with identifier-aware tokenization (so `parseConfig` and `parse_config` both match "parse config") and stemming. It is what makes exact names, config keys, and file names easy to find.

**Vector index.** Each chunk is turned into an embedding, a list of numbers that places code with similar meaning close together. The query is embedded the same way, and Vera finds the nearest chunks with an exact scan over a memory-mapped vector file using SIMD instructions (by default; `VERA_VECTOR_SCAN=vec0` selects sqlite-vec instead). This finds "authentication middleware" even when the code never uses those words. With the default scan, `--lang` and `--path` filters skip ineligible chunks as the scan goes rather than filtering afterward.

Neither index is enough alone. Keyword search misses code described in different words; embedding search is weak on exact identifiers. Running both covers each one's blind spots.

## Fusion

The two result lists are merged with Reciprocal Rank Fusion (RRF). Each chunk's score is the sum of `1 / (k + rank)` over the lists it appears in, with `k = 60`. A chunk near the top of both lists comes out on top; a chunk that only one list found still appears, lower down. RRF only uses positions, so it needs no tuning to reconcile two different scoring scales.

## Code-aware ranking

After fusion, and after reranking when it is on, a deterministic ranking stage applies rules for how people search code:

- A chunk that defines the symbol named in the query outranks chunks that merely mention it.
- Files whose names or paths match query terms get a boost.
- Chunks that cover more of the query's terms rank higher.
- Results from the same file reinforce each other, but one file cannot crowd out everything else.
- Source code ranks above docs and tests unless you narrow the search with `--scope`. Generated files are left out unless you pass `--include-generated`.
- For broad questions, Vera keeps a deeper candidate pool so that the right class or `impl` block is not pushed out by a small helper function.

Each rule was measured on the benchmark before it shipped, and proposed rules that did not help are recorded as rejected in the [decision records](adr/000-decision-summary.md).

## Optional reranking

A reranker is a cross-encoder: instead of comparing precomputed embeddings, it reads the query and each candidate together and scores how well they match. When enabled, Vera sends the top fused candidates (50 by default) through it and reorders them before code-aware ranking runs.

Reranking is off by default. In screening, the tested local rerankers did not beat the code-aware ranking above, and they add latency. The Qwen API reranker did improve results when paired with Qwen embeddings. Obvious filename and path queries skip reranking because keyword evidence already settles them. If the reranker fails, Vera returns the unreranked results and says so. See [models](models.md#reranking).

## Search scores

When a result includes a `score`, it reflects position in that query's ranking and may be normalized. It is not a probability and cannot be compared between queries. Use the returned order.

## Storage

Everything for a project lives in `.vera/` at the project root:

- a SQLite database with chunk metadata, file hashes, call sites, and parse health
- a Tantivy directory for keyword search
- the embeddings, stored in SQLite (`sqlite-vec`) and in a flat file that is memory-mapped for fast exact scans

There is no database server, daemon, or background process. Downloaded models live in Vera's data directory (see [installation](installation.md#where-vera-keeps-files)).

Full builds are written to a staging directory and swapped in when complete, so an interrupted build never leaves a half-written index in place. Contributor-level storage details are in [architecture](architecture.md).

## Incremental updates

`vera update .` re-parses, re-chunks, and re-embeds only files that changed. Deleted files are removed. `vera watch .` runs the same update automatically as you edit.

To find changes without reading every file, Vera records each indexed file's size, modification and change times, and file identity (inode on Unix, file ID on Windows). A file whose metadata still matches is unchanged; any other file is read and its content hash compared with the stored one. The stale-index check before each search uses the same test. Files modified in the last few seconds and reStructuredText files (their content depends on included files) are always hashed. `vera update --verify .` ignores the recorded metadata and hashes every file, for filesystems that report unreliable metadata.
