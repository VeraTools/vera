# Migrating to Vera v2

Upgrade the binary or wrapper as usual. Ordinary default indexes remain compatible, and the search-result JSON fields are unchanged.

## Retired Experiments

V2 removes four rejected experiments:

| Experiment | Removed configuration keys | Removed environment variables |
|---|---|---|
| Multiplicative path penalty | `retrieval.ranking_multiplicative_path_penalty`, `retrieval.ranking_path_penalty`, `retrieval.ranking_multiplicative_penalty` | `VERA_RANKING_MULTIPLICATIVE_PATH_PENALTY`, `VERA_RANKING_PATH_PENALTY`, `VERA_RANKING_MULTIPLICATIVE_PENALTY` |
| Uniform candidate-pool multiplier | `retrieval.ranking_candidate_pool_multiplier`, `retrieval.ranking_candidate_pool_size_multiplier`, `retrieval.ranking_pool_multiplier` | `VERA_RANKING_CANDIDATE_POOL_MULTIPLIER`, `VERA_RANKING_CANDIDATE_POOL_SIZE_MULTIPLIER`, `VERA_RANKING_POOL_MULTIPLIER`, `VERA_RANKING_CANDIDATE_POOL_MULT` |
| Character-cap chunking | `indexing.chunk_max_chars`, `indexing.max_chunk_chars`, `indexing.max_chunk_characters` | `VERA_INDEXING_CHUNK_MAX_CHARS`, `VERA_INDEXING_MAX_CHUNK_CHARS`, `VERA_MAX_CHUNK_CHARS`, `VERA_CHUNK_MAX_CHARS` |
| Structural graph augmentation | None | `VERA_GRAPH_AUGMENT` |

Existing configuration files still load. Removed keys are ignored and disappear when configuration is saved; `vera config get` and `vera config set` report them as unknown keys. Removed environment variables no longer affect behavior. The supported query-dependent recall-pool expansion and explicit `references` queries remain available. See [ADR 007](adr/007-ranking-hypotheses.md) for the decision and evidence.

If an index was built with a nonzero character cap, search and incremental update require a full rebuild:

```bash
vera index /path/to/repository
```

Indexes with missing or zero character-cap metadata continue to work. Running `vera update` cannot convert an incompatible index because unchanged files would retain the old chunk boundaries.

## Embedding Request Defaults

API indexing now sends two 128-input requests at a time (`embedding.max_in_flight_inputs` 256, up from 16) with a 120-second timeout (up from 60). Earlier versions wrote every default into `config.json` on any save, so the first v2 load moves a saved `max_in_flight_inputs` of 16 or `timeout_secs` of 60 to the new defaults and marks the file with `config_format`. To keep the old values, set them again after upgrading:

```bash
vera config set embedding.max_in_flight_inputs 16
vera config set embedding.timeout_secs 60
```

## Indexing and Search Behavior

- Saved `embedding.query_prefix` and `embedding.document_prefix` values now reach API requests. Earlier versions ignored them. If you saved a prefix, rebuild with `vera index` so stored vectors match new queries.
- Saved `indexing.no_ignore`, `indexing.no_default_excludes`, and `indexing.extra_excludes` now apply when the matching flags are absent; earlier versions reset them on every run. `--exclude` adds to saved globs instead of replacing them.
- A failed API `vera index` leaves `<repo>/.vera.resume/` so the next run can reuse finished embeddings. Add it to `.gitignore` next to `.vera/`.
- An index left half-written by an interrupted `vera update` is refused until `vera update` or `vera index` repairs it.
- Index and update summaries gain request, retry, timeout, and phase-time fields. Existing fields are unchanged. Slow runs print progress lines to stderr even when it is not a terminal; `--no-progress` turns them off.
- When `retrieval.reranking_enabled` is true but no reranker is configured, or the reranker cannot be built, search prints `reranker unavailable` on stderr and returns unreranked results.
- Error messages from embedding, reranker, and completion APIs no longer include endpoint URLs.

## Docker Images

Each image now sets `VERA_BACKEND`: Potion Code for `cpu`, and the matching Jina ONNX backend for `cuda`, `rocm`, and `openvino`. Earlier images ran Jina ONNX on the CPU in every variant. GPU images keep the same model, so their indexes still work. Indexes built with an older `cpu` image used Jina, so rebuild them or keep Jina:

```bash
docker run --rm -v "$(pwd):/workspace" ghcr.io/veratools/vera:cpu index /workspace
docker run --rm -i -e VERA_BACKEND=onnx-jina-cpu -v "$(pwd):/workspace" ghcr.io/veratools/vera:cpu
```

The image setting wins over a backend saved with `vera setup`; pass `-e VERA_BACKEND=<backend>` to override it.

## Search Scores

See [Search scores](how-it-works.md#search-scores) for interpreting returned ranking values.

## Setup and Agent Skills

Bare `vera setup` runs the full wizard for local and API backends. Local CPU is first and selected by default; API mode is second, with Qwen/OpenRouter recommended among API presets. Indexing defaults to Yes for local backends and No for API mode. Explicit backend flags keep a shorter configuration flow.

The agent selector preselects installed clients and uses Space to toggle, Enter to continue. Installation adds or updates selected clients; deselecting a client leaves its installation in place. Empty selection makes no changes. Use `vera agent remove` for removal; shared directories appear as grouped choices. See [Installation](installation.md#set-up-agent-skills).
