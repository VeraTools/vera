# Architecture

## Workspace crates

| Crate | Purpose | Entry point |
|-------|---------|-------------|
| `vera-core` | Parsing, indexing, storage, embedding, retrieval | `lib.rs` |
| `vera-cli` | CLI (clap derive macros) | `main.rs` |
| `vera-mcp` | MCP server (JSON-RPC over stdio) | `server.rs` |
| `vera-serve` | HTTP inference server (OpenAI/Cohere-compatible) | `lib.rs` |
| `vera-eval` (`eval/`) | Benchmark harness and metrics | `src/main.rs` |
| `tree-sitter-{sql,proto,vue,dockerfile,astro,scss}` | In-repo C grammars, built with `cc` | `build.rs` |

## vera-core modules

### `parsing/`: language parsing and symbol extraction

Files: `mod.rs` (public API), `languages.rs` (grammar dispatch), `extractor/` (maps AST nodes to `SymbolType`), `chunker.rs` (symbol-aware, whole-file, and Tier 0 chunking).

Data flow: file, then grammar lookup, then tree-sitter parse (with diagnostics), then node classification, then chunks.

### `embedding/`: embedding generation

`EmbeddingProvider` trait with three implementations:
- `OpenAiProvider` (`provider.rs`): HTTP calls to OpenAI-compatible endpoints
- `LocalEmbeddingProvider` (`local_provider.rs`): ONNX Runtime inference for the opt-in Jina and custom ONNX models
- `Model2VecProvider` (`model2vec_provider.rs`): the default `potion-code-16M-v2` static embeddings on CPU

`DynamicProvider` (`dynamic.rs`) picks one at runtime from the saved backend, `VERA_BACKEND`, or backend CLI flags.

### `retrieval/`: search pipeline

1. The query enters `search_service.rs`.
2. BM25 (`bm25.rs`) and vector search (`vector.rs`) run in parallel.
3. Results are fused with RRF in `hybrid.rs`. k defaults to 60 and is set by `retrieval.rrf_k`. `fuse_rrf_multi_weighted` generalizes fusion to N ranked lists.
4. If reranking is enabled, a cross-encoder reorders the top candidates (`reranker.rs` for API endpoints, `local_reranker.rs` for local models).
5. Query-aware ranking applies deterministic priors (`ranking/`, `search_service.rs`). `ranking/score.rs` runs the stages; `path.rs` and `content.rs` hold path and content signals, `query.rs` classifies queries, and `diversification.rs` keeps one file from crowding out the rest.
6. The final `Vec<SearchResult>` is returned.

Deep search (`--deep`): `rag_fusion.rs` runs a quick BM25 search to collect symbol names and file paths, then passes them as hints to the LLM (`completion_client.rs`), which splits the query into targeted sub-queries (2 by default). Sub-queries reuse the active `SearchContext`, and the results merge with weighted RRF, with the original query weighted 2x. Without a completion endpoint, deep search follows symbols iteratively instead (`iterative_search.rs`).

Structural search:
- `references.rs`: exact caller lookups from the stored call graph, returned as search-style snippets
- `structural.rs`: structural queries for definitions, environment variable reads, routes, SQL, and implementations

### `storage/`: persistent storage

#### Vector storage

- `metadata.rs`: SQLite with chunk metadata, file paths, content hashes, and per-file index state used for health reporting
- `bm25.rs`: the Tantivy full-text BM25 index
- `vector.rs`: dual sqlite-vec and flat SIMD embedding storage. The default
  exact scan reads `.vera/vectors.f32` through `memmap2` and uses SimSIMD L2;
  `VERA_VECTOR_SCAN=vec0` selects sqlite-vec for rollback and ablation. The
  sidecar uses `.vera/vectors.tombs` for deleted rowids and
  `.vera/vectors.manifest` as its generation and size commit marker. A missing
  or inconsistent sidecar is rebuilt by streaming `vec_chunks`. Flat updates
  preserve SQLite rowids. Watch-mode batches write changed vector rows at their
  rowid-derived offsets, update the bitmap, fsync the data, and publish the
  manifest last; initial builds and recovery use atomic full-file publication.
  True compaction is deferred until a remapping protocol is available.

Everything is stored in `.vera/` at the project root.

### `indexing/`: index build and update

- `pipeline.rs`: full build. Discovers, parses, chunks, embeds, and stores files in bounded windows, writes to `.vera.build` and swaps it in, keeps `.vera.old` for crash recovery, and stores parse diagnostics.
- `update.rs`: incremental update. Detects changes by content hash, reprocesses only changed files, and refreshes per-file index state.

### Other modules

- `types.rs`: the `Language` enum (65 languages plus `Unknown`), `Chunk`, `SearchResult`, and `SymbolType`
- `config.rs`: public configuration types and shared environment parsing. `config/` splits out indexing, retrieval, embedding and model identity, backend selection, and hardware detection; callers still import from `config::*`.
- `local_models/`: local embedding presets, custom ONNX embedding configs, and ONNX Runtime and model files under the Vera data directory (XDG paths on Linux)
- `discovery/`: file discovery with gitignore support and binary and size filtering
- `git_scope.rs`: turns `--changed`, `--since`, and `--base` into exact paths relative to the indexed directory, which may sit below the git repository root
- `chunk_text.rs`: builds the embedding and BM25 text for each chunk, truncated at line boundaries to fit a byte budget

## vera-cli

`main.rs` parses arguments with clap. `commands/` contains the CLI subcommand implementations: `agent`, `backend`, `config`, `doctor`, `explain_path`, `grep`, `index`, `mcp`, `overview`, `references` (also used by `dead-code`), `repair`, `search`, `serve`, `setup`, `stats`, `structural`, `uninstall`, `update`, `upgrade`, and `watch`. `helpers.rs` re-exports shared helpers from `helpers/`: `filters.rs` (search filters and git scopes), `index_root.rs` (index discovery and staleness warnings), `output.rs` (result rendering, output budgets, and progress display), and `runtime.rs` (backend flags and interrupt handling).

## vera-mcp

`server.rs` routes JSON-RPC requests. `tools.rs` exposes seven MCP tools: `search_code`, `get_stats`, `get_overview`, `regex_search`, `structural_search`, `find_references`, and `explain_path`. `tools/schemas.rs` defines their input schemas, `tools/handlers.rs` handles calls, and `tools/runtime.rs` holds the shared Tokio runtime, the cached search context, and per-repository file watchers. `search_code`, `structural_search`, and `find_references` index the project and start a file watcher on first use. The search, references, and overview tools also accept git scopes for changed files.

## Adding a new language

1. Add a `Language` variant in `types.rs`, in alphabetical order.
2. Add the extension mapping in `Language::from_extension()`.
3. Add the grammar dependency to `vera-core/Cargo.toml`.
4. Wire the grammar in `tree_sitter_grammar()` in `parsing/languages.rs`.
5. Add node classification in `classify_node()` in `parsing/extractor/classify.rs`.
6. Write tests for the extension mapping, grammar loading, and symbol extraction.
7. Add the language to [supported languages](supported-languages.md).
