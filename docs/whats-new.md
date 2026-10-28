# What's new

Release notes from v1.0 onward, newest first. Current benchmark results are on the [benchmarks page](benchmarks.md), and every command is described in the [feature guide](features.md). Upgrading from 1.x? Read the [v2 migration guide](migration-v2.md) first.

## v2.3.0

- `vera search "query" --also <dir>` searches your project and other indexed trees in one call. Repeat `--also` for up to 8 indexes. Results are merged by rank with a small head start for the current project; results from other trees show absolute paths, and `--json` adds a `repo` field. MCP `search_code` takes a matching `repos` array. Neither ever creates an index. See [querying another index](features.md#querying-another-index).
- Searches and `vera update` start faster on large repositories. Vera now records each file's size, timestamps, and file identity, and skips reading files whose recorded metadata still matches. Files changed in the last few seconds and reStructuredText files are still hashed every time. On a 51,000-file tree, a warm search went from 483 ms to 197 ms, a no-op `vera update` from 756 ms to 130 ms (and from 660 MB to 77 MB of memory), and an update after 1,000 file changes from 11.9 s to 6.5 s. Existing indexes pick this up without a rebuild: the first search after upgrading hashes the files once and records their metadata.
- `vera update --verify` hashes every file instead of trusting recorded metadata, for filesystems with unreliable timestamps.
- File discovery walks directories in parallel.
- Tantivy is updated to 0.27, which moves zstd to 0.14.3 for RUSTSEC-2026-0391 and related zstd advisories. Tantivy 0.27 ships an updated English stemmer that stems about 0.1% of words differently ("added" now stems to "add", "internal" stays "internal"). Existing indexes keep working; run `vera index .` to rebuild if keyword search misses one of those words.
- `--timing` on `search` reports every stage (index lookup, freshness check, setup, search stages, output) and the end-to-end total. `grep` and `structural` report index lookup, freshness check, search, output, and total.
- An MCP server started in a subdirectory of an indexed repository uses that repository's index instead of creating a nested one, and the `repo` argument accepts any directory inside an indexed repository. The server refuses to auto-index the filesystem root or your home directory.

## v2.2.0

- `vera index <path> --full-text-only` builds a keyword (BM25) index without embeddings, so large trees such as a dependency cache index quickly with no model or API calls. Search, grep, structural queries, and the MCP tools all work on it. See [keyword-only index](features.md#keyword-only-index).
- `vera index <path> --dry-run` reports the files, size, estimated chunks, and rough indexing time for a directory without writing an index. See [dry run](features.md#dry-run).
- `vera -C <dir>` runs a command as if started in `<dir>`, like `git -C`, so you can query a second index (for example `vera -C ~/.cargo/registry/src search "retry backoff"`) without leaving your project. MCP tools take a matching optional `repo` argument; a `repo` other than the server's directory must already be indexed. See [querying another index](features.md#querying-another-index).
- `vera grep` accepts `-C` as the short form of `--context`.
- `vera watch` run from a subdirectory watches the index that covers it, as `vera update` already did. Stale-index warnings after `vera -C` name the command that refreshes that index.
- Re-indexing a directory that no longer has any indexable files removes its old index instead of leaving stale results behind.
- Incremental updates no longer fail now and then with a BM25 "lock busy" error when another thread of the same process starts a child process (such as `git`) at the same moment.
- Piping output into a command that exits early, such as `vera stats | head -1`, now ends quietly instead of panicking (Linux and macOS).
- Updates tokio to 1.54.1 for RUSTSEC-2026-0386.

## v2.1.5

- Agent guidance now allows normal search tools when no index exists and explains long first-index runs and reusing finished API-mode embeddings. Index-required commands share the same no-index guidance.
- Indexing keeps its artifacts out of git through [local excludes](installation.md#index-and-search), including in linked worktrees and nested project directories.
- `vera references` accepts repeatable `--path` filters for callers and callees, using the same path rules as search.

## v2.1.4

- `vera setup --api` and `vera backend --api` turn reranking on when you configure a reranker, including with the Qwen preset, and turn it off when you set up without one. Before, setup saved the reranker but left reranking off, so searches were never reranked. If you set up API mode with an earlier version, run setup again or `vera config set retrieval.reranking_enabled true`; `vera doctor` now warns when a reranker is saved but reranking is off.
- Switching from API mode to a local backend drops the saved API reranker and turns reranking off with it, so it does not silently switch to the slower local CPU reranker. Reranking stays on if your shell sets all three `RERANKER_MODEL_*` variables, or if you turned it on for a local backend with no API reranker saved.
- When a local model fails to load, the hint now links to the manual install guide instead of naming an outdated model path.
- Security update: rustls is updated to 0.23.46 for [RUSTSEC-2026-0346](https://rustsec.org/advisories/RUSTSEC-2026-0346.html), [0347](https://rustsec.org/advisories/RUSTSEC-2026-0347.html), [0348](https://rustsec.org/advisories/RUSTSEC-2026-0348.html), and [0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html).

## v2.1.3

- `vera update` now repairs an index whose vectors are missing (for example, after `.vera/vectors.db` was deleted). Files without stored vectors are re-embedded instead of being reported as up to date, and the search warning for this case tells you to run `vera update .`.

## v2.1.2

- Search warns when the index has chunks but no stored vectors, such as after `.vera/vectors.db` was deleted. Before, the CLI and MCP server quietly returned keyword-only results.

## v2.1.1

- MCP search results now say when they are degraded: keyword-only because semantic search failed or was skipped (an expired embedding key, for example), or unreranked because the reranker failed. The CLI already printed these warnings on stderr, but MCP clients never saw them.
- `retrieval.rrf_k` now applies to ordinary searches. It used to affect only the merging of multiple queries. The default (60) is unchanged, and an invalid saved value falls back to it.

## v2.1.0

- `vera uninstall` once again removes the `vera` command that the npm and pip installers create. Since v2.0.0 it had left that command behind and reported a partial uninstall.
- `vera uninstall` deletes the data directory only when its contents match what Vera creates. If `VERA_HOME` points at a shared directory, Vera leaves it in place and says so.
- The npm and pip installers no longer overwrite a `vera` command they did not create (such as one from `cargo install`), and they rewrite their own command only during `install`.
- The npm and pip installers store data in the same directory Vera uses (`~/.local/share/vera` on Linux), unless `~/.vera` already holds an installation.
- `npm install -g @vera-ai/cli` installs a command named `vera-ai`, matching pip. It used to be named `cli`.
- Running `install` without a terminal, as in CI, installs the binary and skips the agent selector instead of failing.
- `vera upgrade --apply` installs the exact new version, so if a package registry has not caught up yet, the upgrade fails with a clear message instead of reinstalling the current release. It no longer changes globally installed npm or Bun packages.

## v2.0.1

- Environment variables now override saved settings only while Vera runs and are never written to `config.json`. Before, running `vera config set` with a variable such as `VERA_MAX_OUTPUT_CHARS` set saved that value permanently, and once a value was saved, the variable was ignored. If an earlier `config set` saved a value you did not intend, set the key again or remove it from `config.json`.
- GPU indexing again uses batches of at most 16 inputs, as in v1. v2.0.0 ran batches of 64 to 128 on cards with 8 GB or more, which used up to 4 times the GPU memory and indexed 4 to 12% slower on an RTX 4080.
- When an API batch is split because it exceeds the model's context and a later part fails, the finished parts are kept, and a retry resends only the unfinished inputs.

## v2.0.0

v2 removes the controls for ranking and chunking experiments that were measured and rejected in v1.x (path penalties, a larger candidate pool, character-capped chunks, and structural graph augmentation). Default indexes keep working. Indexes built with the experimental character cap need a full rebuild. The [v2 migration guide](migration-v2.md) lists the removed settings.

Setup changes:

- The setup wizard lists local CPU first, then API mode, with Qwen through OpenRouter as the first API preset. Both paths offer to install agent skills and index the project; indexing defaults to Yes for local backends and No for API mode.
- The agent installer only adds or updates the clients you select. An empty selection changes nothing, and shared skill directories are grouped when you remove them.
- API setup and `vera backend` save the same protocol, endpoint, and task settings.
- The npm and pip wrappers verify the exact-version release archive and reuse a completed download offline. An interrupted download, an unsafe archive entry, or a checksum failure never leaves a binary in the cache.

### API indexing reliability

- API indexing sends two requests of 128 inputs at a time with a 120-second timeout (previously 16 inputs and 60 seconds). Saved configs that held the old defaults are upgraded once; other values you set stay. Against a test endpoint with slow responses, the old defaults failed 10 of 10 runs and the new ones finished all 10.
- Retries back off exponentially with jitter and honor `Retry-After` up to 60 seconds. Failed or timed-out batches move to the back of the queue and are resent up to twice. Authentication errors and other 4xx errors fail immediately.
- One slow response no longer keeps the other request slots idle.
- When an API `vera index` fails, finished embeddings are kept in `<repo>/.vera.resume/` and reused on the next run (reported as `embeddings_reused`). Add `.vera.resume/` to `.gitignore`. An index left half-written by an interrupted `vera update` is refused until `vera update` repairs it.
- `vera index --json` and `vera update --json` report request, retry, timeout, and failed-batch counts, plus time spent in each stage. Slow runs print progress to stderr even without a terminal. On failure, stdout stays empty and the error is the last line on stderr.
- Error messages no longer include endpoint URLs, and tokens and credentials in provider responses are redacted.
- Potion Code embeds each chunk on its own, so a chunk's vector no longer depends on which other chunks shared its batch, and local indexing is faster. Older indexes keep working; run `vera index` to refresh their vectors.

### Search and configuration

- `vera search --json --rerank-status` reports whether results were reranked, and if not, why. When reranking is enabled but the reranker is missing or broken, search prints a `reranker unavailable` warning.
- Saved `embedding.query_prefix` and `embedding.document_prefix` values now reach API requests. Saved ignore and exclude settings are no longer reset when the matching CLI flags are absent. Every documented config key works with `vera config get` and `vera config set`.
- JSON search output stays within `retrieval.max_output_chars` even when result content needs escaping.
- Each Docker image sets its own `VERA_BACKEND`, so the GPU images now use the GPU. See [Docker](docker.md).

## v1.4.2

- Security update: rustls is updated to 0.23.45 for [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285.html).

## v1.4.1

### Working with agents

- Commands work from any subdirectory. `search`, `grep`, `structural`, `references`, `overview`, `stats`, and `update` find the nearest `.vera/` index above the current directory and run against that repository root, so agents no longer need to `cd` first. An empty `.vera/` directory (including an old Vera data directory at `~/.vera`) is never mistaken for an index.
- An absolute `--path` inside the repository is converted to a root-relative path instead of matching nothing.
- `vera grep "pattern" -n`, a habit carried over from grep, no longer errors: a trailing bare `-n` means the default limit of 20. Errors about `--limit` print a one-line hint.
- A stale-index warning prints at most once every 10 minutes per index. Set `VERA_STALE_WARNING_ALWAYS=1` to print it on every run.
- The update-available hint prints only in an interactive terminal, and reranker retry messages moved to debug logging, so agent output stays clean.
- The skill installed by `vera agent install` was rewritten around mistakes agents actually made: which command to use for the first search, a command for each kind of question, what to do when the top hit is a usage site, and how to write root-relative `--path` filters.

### Install and platform fixes

- `vera uninstall` reports a Vera launcher it cannot prove it created and leaves it in place, instead of silently ignoring it.
- Linux release binaries now need only glibc 2.28, so they run on Ubuntu 20.04, Debian 10, RHEL 8, and newer.
- The musl binary bundles a working C++ runtime, which fixes the v1.4.0 crash on repositories containing `.sql` files.

### Docs

- Agent benchmark claims now match what the data supports: only the Qwen backend's context saving is statistically significant, and the two older runs are labeled as history.
- The README presents the CLI plus the agent skill as the recommended agent setup, with the MCP server as an option.

## v1.4.0

### Faster filtered search

When a query uses `--path`, a glob, or `--lang`, the vector scan now skips chunks that do not match the filter, instead of loading the whole index and filtering afterward. Across the full 1,251-task Semble suite (Ryzen 7 9800X3D, local Potion Code embeddings, three runs each), median query time dropped from 12.0 ms to 6.35 ms and p95 from 139 ms to 65 ms. Results are identical to the old path. To turn it off, set `retrieval.vector_filter_during_scan` in config or `VERA_VECTOR_FILTER_DURING_SCAN` in the environment. [ADR 008](adr/008-filter-during-scan-default.md) has the measurements.

### Agent benchmark

A new run with GLM-5.3 (high reasoning effort) answered 10 cross-file questions about Flask in four setups: Vera with the local backend, Vera with the Qwen API backend, the Semble CLI, and grep and file reads only. Every setup answered all 10 correctly. Compared with grep and file reads, agents used 27% less context with local Vera, 48% less with the Qwen backend, and 12% less with Semble. With only 10 questions, the 48% reduction is the only statistically significant result. Two earlier runs on other models used an older integration and did not show the same savings, so they are kept as history. Full results: [agent-level benchmark](benchmarks-history.md#agent-level-benchmark).

## v1.3.1

- When every `--path` pattern matches nothing, `vera grep` says so and suggests a wildcard directory form.
- On Windows, `vera setup` now runs a Direct3D 12 check to decide whether DirectML can work, instead of accepting any non-Basic display adapter.
- `vera uninstall` recognizes the `vera` command the installers wrote by its exact contents, and handles a non-default `CARGO_HOME`.

## v1.3.0

### Search correctness

- Long symbols that are split into several chunks keep their identity. `vera structural definitions` finds them by bare name, `vera references` resolves their call sites, and `vera dead-code` lists each symbol once per file. JSON output carries the bare name plus a part index; text output shows `(part N)`.
- Filtered vector searches no longer stop at 4,096 candidates, so path and language filters find matches that rank far down the list. The incorrect truncation warning is gone. The sqlite-vec fallback (`VERA_VECTOR_SCAN=vec0`) warns only when its limit could actually drop results, and says how to fix it.
- Each query opens the vector store once instead of twice.

### Indexing progress

- While files are still being parsed, the embedding progress bar shows activity without a percentage. Once parsing finishes it switches to a fixed total and fills to 100%. Cancelled or failed runs no longer look successful. Output with `--no-progress`, `--json`, or no terminal is unchanged.

### Ranking and reranking

- Three ranking signals (filename-stem boost, definition boost, and recall-pool expansion) can each be turned off with a config key (`retrieval.ranking_filename_stem_boost`, `retrieval.ranking_definition_boost`, `retrieval.ranking_recall_pool_expansion`) or a `VERA_RANKING_*` environment variable.
- Three more ranking and chunking ideas were tested on the full benchmark and stayed off because none reached the 0.5% improvement required to ship. v2.0.0 removed them. The measurements are in [benchmark history](benchmarks-history.md#rejected-ranking-and-chunking-hypotheses-2026-09-01).
- Reranker requests use the right format for generic endpoints (`top_n` and `results`) and Voyage (`top_k` and `data`). A protocol set in config overrides detection from the hostname. Permanent 4xx errors are not retried, `Retry-After` and `X-RateLimit-Reset` waits are capped, and if reranking fails, search returns unreranked results.

### Setup

- The interactive `vera setup` offers a Qwen preset through OpenRouter that needs a single API key and picks the protocol automatically.
- `vera doctor` accepts intentionally blank environment variables and checks DirectML more accurately. `vera uninstall` reports more precisely what it found. The help for `retrieval.max_output_chars` now says `0` means unlimited.

### Compatibility

No breaking CLI changes. Existing indexes open and search as before. New indexes store split-symbol part numbers.

## v1.2.0

### Query latency

- Exact-name matching no longer re-reads the list of indexed files on every query. The list is cached per open index and refreshed as soon as the database changes. On the zig repository (about 336,000 chunks), that step dropped from 18.4 ms to 1.4 ms at the median, and total warm query time from 31.1 ms to 14.1 ms. Full-suite p95 improved from 73.3 ms to 64.5 ms.

### Indexing speed and memory

- Full builds parse, embed, and store at the same time instead of in sequence, so embedding never waits on disk writes. Indexing the full Semble corpus dropped from 213.5 s to about 137 s (36% faster); the zig repository alone went from 62.7 s to 36.7 s.
- Full builds process the repository in bounded windows instead of holding every chunk and embedding in memory. On a 638 MB corpus with 27,252 chunks, peak memory dropped from 2,746 MB to 640 MB. Search results over the new indexes are identical to before.
- Full builds write to a `.vera.build` staging directory and keep the previous index in `.vera.old` until the swap finishes. If a crash interrupts the swap, the next run restores the previous index and cleans up. File discovery and watchers always skip these two directories.

### Ranking

- The definition boost no longer treats definitions in test, example, and benchmark directories as the real definition unless the query asks for them. Before, a test fixture could outrank the real class. Modules whose names only look like tests, such as click's `testing.py`, keep their boost.

### Search and storage

- Filtered keyword searches are faster at the tail: p95 dropped from 55.1 ms to 51.8 ms, with unfiltered searches unchanged.
- `vera stats`, call-graph queries, and update checks open the index read-only, so reading never creates or changes an index.
- Filtered `vera overview` statistics are computed in the database instead of loading every row.
- Vector counts in `vera stats` come from stored chunks, and deleted vectors no longer inflate them.

### Models and API

- API embedding providers support document prefixes through `EMBEDDING_DOCUMENT_PREFIX`, detected from the model ID when unset.
- The document prefix used to build an index is stored with it. If the active prefix changes, search falls back to keyword-only results and `vera update` asks for a re-index, rather than mixing vectors built from different inputs.
- ROCm GPU detection handles both `rocm-smi` output layouts and treats near-zero free VRAM conservatively.

### Other fixes

- Markdown chunking respects code fences, and Python class inheritance is extracted more accurately.
- Files with names like `snapshot_writer.rs` are no longer misclassified as archived content and hidden from source-scoped searches.
- `vera doctor` no longer treats intentionally blank environment variables as missing, `vera uninstall` reports partial failures accurately, and `vera agent install` refuses to prompt without an interactive terminal.
- Update checks compare full version numbers, and helper output honors `max_output_chars` in JSON and raw modes.
- MCP file watchers are scoped per working directory, merge repeated updates, and ignore index staging writes. `vera serve` limits the size of incoming requests.

### Building from source

- Building from source needs Rust 1.88 or later. The previously stated minimum, 1.86, could not build the code.

### Compatibility

v1.1.0 indexes still open. If an index was built without the API document prefix that is now active, Vera turns off its vector search until you rebuild it, rather than mixing embeddings. No breaking CLI changes.

## v1.1.0

### Search quality and default model

- The default local embedding model is `minishlab/potion-code-16M-v2`, pinned to revision `e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b`. It runs on the CPU without ONNX Runtime or a GPU.
- With the same model, harness, and scoring for both tools, Vera scored 0.8441 nDCG@10 against Semble's 0.8514 on the full 1,251-task suite, 0.8540 against 0.8494 on the 320-task subset, and 0.7644 against 0.7655 on the independent 180-task set.
- New ranking signals: file coherence, keyword-path matching, content coverage, definitions, and a concept pool. Each was measured with and without on both the 320-task subset and the independent set, and signals that did not clear the bar were dropped.
- Cross-encoder reranking stays opt-in. No tested cross-encoder beat search without a reranker. If you want local reranking anyway, `mxbai-rerank-xsmall-v1` is the recommended model.

### Query speed

- Vector search scans memory-mapped vectors directly with SIMD instructions, with sqlite-vec kept for writes and as a fallback. Median query time on the Semble comparison dropped from 15.1 ms to 9.9 ms.
- On the largest tested repository, zig (about 336,000 chunks), the vector step dropped from about 460 ms to 21 ms per query.
- Search stores are reused across queries, the reranker loads only when needed, and local rerank batches are sorted by length to reduce wasted work.

### Model download integrity

- Local model downloads can be pinned to fixed Hugging Face revisions. Built-in models carry pinned revisions and SHA-256 checks compiled into the binary.
- Custom downloads record their checksums, ONNX Runtime archives are verified before extraction, and ONNX models with weights split across several files are supported.
- The local reranker can be replaced with `LOCAL_RERANKER_*` environment variables.
- Repository files are opened relative to the repository directory, which reduces the risk of reading files outside it through crafted paths.

### Indexing, parsing, and MCP

- RST include chains are bounded and cached, so large include graphs cannot blow up indexing.
- Jina v5 uses last-token pooling and its query and document prefixes. Symbol extraction improved for Lua function assignments, Bash case arms, JSX call sites, nested Rust modules, and members of containers.
- `vera update` can be cancelled safely, runs in parallel where safe, and records freshness only after the stores are rebuilt.
- MCP multi-query results are merged by rank instead of concatenated, MCP results include stale-index warnings, and the watcher reuses its model between updates.
- `vera serve` keeps one loaded model per slot and finishes a model load even if the client that requested it disconnects.

### Platform

- Upgraded to Tantivy 0.26.1.
- Docker images are published for CPU, CUDA, ROCm, and OpenVINO, and contain the same Linux binary as the GitHub release.

## v1.0.1

- `vera index` gained `--no-progress`, and progress output appears only when stderr is a terminal.
- Flag-driven setup works without a terminal and rejects blank or incomplete API credentials.
- Agent configuration sync runs quietly and keeps your edits outside Vera's marked sections.
- `vera upgrade` checks the version of the installed binary instead of trusting the installer's exit code.
- CoreML reranking uses the CPU path, avoiding a problem with fp16.
- sqlite-vec searches are clamped to that backend's limit without turning off vector search.

## v1.0

Vera 1.0 brought together the hybrid search pipeline, the code-intelligence commands, agent integrations, and local inference backends. Many fixes along the way came from community pull requests and user reports.

### Search quality

The v1.0.0 release candidate was measured on the full 1,251-task Semble v0.5.5 suite across 63 repositories, using hybrid keyword and vector search with a local ONNX cross-encoder reranker on a CUDA GPU. The scores use Vera's graded scoring, which differs from Semble's published metric; see [benchmark methodology](benchmarks.md#provenance).

| Metric | v1.0.0-rc |
|--------|-----------|
| nDCG@10 | 0.7327 |
| Recall@1 | 0.5476 |
| Recall@5 | 0.8144 |
| Mean search latency | 2421 ms |

The reranker accounts for most of that latency. Keyword-only search with path filters ran at about 54 ms p95 at the time, so the two figures describe different setups.

The gains came from a reworked default pipeline: stemming and identifier-aware tokenization for keyword search, stronger ranking of definitions, matching concepts to file names, adaptive weighting when merging keyword and vector results, a penalty for many results from one file, and running keyword and vector search in parallel. A bug that starved filtered keyword searches of candidates was also fixed. Three further changes were measured before release:

| Change | Decision |
|--------|----------|
| Skip reranking when there are no extra candidates beyond the requested result count | Shipped. nDCG@10 changed by -0.0002 and mean latency dropped 2.4 ms. |
| Rerank searches that use `--path` filters | Shipped. Full-suite nDCG@10 rose by 0.0113. |
| Add callers and implementations to the rerank pool (structural graph augmentation) | Released as an experimental opt-in (`VERA_GRAPH_AUGMENT=1`). It gained 0.0047 nDCG@10 but added about 83% to mean latency, and Recall@5 did not move. Removed in v2.0.0. |

### Agent benchmark

Ten cross-file questions about Flask were each answered by a fresh agent, once with Vera installed and once with the Vera command blocked. A judge model graded the answers blind against a verified answer key.

| Model | With Vera | Without Vera | Result |
|-------|-----------|--------------|--------|
| `claude-opus-5` | 10.0/10 | 10.0/10 | Same quality and same efficiency |
| `kimi-k3` | 10.0/10 | 9.9/10 | 17% fewer input tokens with Vera (230.6k vs 278.0k) |

This is a small-sample result, not a general claim. Questions, reproduction steps, raw data, and limits are in the [agent benchmark README](../benchmarks/agent-bench/README.md).

### Code intelligence and agent workflows

- `vera structural` answers structural questions: `definitions`, `env`, `routes`, `sql`, and `impls`. Implementations, conformances, inheritance, and Rust trait implementations are stored in the index, so `impls` answers from the index rather than by matching text.
- Git-aware scopes limit a query to changed files: `--changed` for working-tree changes, `--since <rev>` for files changed since a revision, and `--base <rev>` for files changed since the merge base with a revision. They work with `vera search`, `vera grep`, `vera overview`, and `vera references`.
- `vera explain-path <path>` explains why a file is or is not indexed.
- Vera warns when the index no longer matches the working tree, and `vera stats` reports parser health, so a damaged or partial index is visible instead of quietly returning worse results.
- `--path` can be repeated to match any of several patterns. Other filters still narrow the result: `--lang`, `--type`, and `--scope` apply on top of the combined path match.
- `--type function` and `--type method` are interchangeable.

### MCP server

The MCP server went from four tools to seven: `structural_search`, `find_references`, and `explain_path` were added, and the search tools gained the CLI's path filters and git scopes.

### Models and local backends

- `vera setup --potion-code` selects the `minishlab/potion-code-16M-v2` static embedding model, which runs on any supported machine's CPU with no GPU or ONNX Runtime.
- Loaded embedding and reranker models are reused across searches, multi-query and deep searches, and MCP calls.
- Voyage AI rerank endpoints are supported, including the `rerank-2` format.
- `VERA_EMBEDDING_MODEL_ALIASES` lets different deployment names for the same model share an index (after the usual dimension check). Separate groups with semicolons and names within a group with commas.
- Local mode checks ONNX model files, and `vera doctor` and `vera repair` can recover damaged or incomplete downloads.
- CoreML embedding batches scale with the available unified memory on Apple Silicon.
- When an OpenAI-compatible embedding provider reports that an input is too long, Vera truncates it and retries.

### HTTP inference server

`vera serve` runs a local HTTP server with OpenAI-compatible embeddings (`POST /v1/embeddings`), Cohere- and Jina-compatible reranking (`POST /v1/rerank`), and a health endpoint. It supports bearer-token authentication, `--host`, `--port`, and `--idle-timeout` for how long models stay loaded. Requests are cancelled when the client disconnects, an empty key cannot bypass authentication, and internal errors are not exposed in responses.

### Agent configuration sync

`vera agent install` writes the skill files for supported agent clients. During normal use, Vera keeps its own marked section in files such as `AGENTS.md` and `CLAUDE.md` up to date and leaves your edits outside that section alone.

### Indexing and updates

- `vera index` and `vera update` show progress for discovery, parsing, and embedding. Use `--no-progress` or `--json` for quiet or machine-readable output (`vera update` had `--no-progress` in v1.0.0; `vera index` gained it in v1.0.1).
- `vera update . --max-files <N>` limits how many added or modified files one run processes and reports the rest for a later run.
- Indexing and updates can be interrupted: in-flight embedding work stops, and time limits on remote calls keep an abandoned run from using up API quota.
- A failed update keeps the previously indexed data for affected files instead of dropping it.

### Fixes

- Directory patterns such as `--path src/app` match files below that directory instead of returning nothing.
- `--intent` no longer leaks into keyword queries, which had caused errors for intent searches.
- Files with invalid UTF-8 are read with replacement characters instead of being skipped.
- Unicode file paths work with path globs and with `vera grep` match positions.
- Cached embeddings are keyed by model, so vectors from one model are never reused for another.
- Indexing of TypeScript and JavaScript class methods, Java enum methods, TypeScript interface methods, and generic type relations was corrected, which improves `references` and structural results.
- `crossbeam-epoch` was updated for a RustSec security advisory.

### Upgrade notes

No breaking CLI changes: search, indexing, update, agent, and MCP commands keep their names and flags.

Existing indexes open and search as before, but they keep pre-1.0 search quality until you rebuild them. Run `vera index .` to pick up the new keyword fields, caller and type-relation data, and index-health metrics.

Building from source now requires fetching four grammar sources that are not tracked in git. If they are missing, the build stops early and prints the command to run:

```bash
bash scripts/bootstrap-vendored-grammars.sh
cargo build --release
```

Prebuilt binaries and package installs do not need this step. Full source-build instructions are in the [installation guide](installation.md).

### Distribution

Vera is available as prebuilt binaries on GitHub Releases, the `@vera-ai/cli` npm package, the `vera-ai` PyPI package, and Docker images at `ghcr.io/veratools/vera` (`cpu`, `cuda`, `rocm`, and, since v1.0.1, `openvino`). See the [installation guide](installation.md) and the [Docker guide](docker.md).
