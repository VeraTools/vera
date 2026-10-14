# Migrating to Vera v2

Upgrade the binary or package wrapper the usual way. Default indexes from v1 keep working, and the JSON fields in search results are unchanged. The sections below cover what does change.

## Retired settings

V2 removes four ranking and chunking experiments that did not improve results:

| Experiment | Removed configuration keys | Removed environment variables |
|---|---|---|
| Multiplicative path penalty | `retrieval.ranking_multiplicative_path_penalty`, `retrieval.ranking_path_penalty`, `retrieval.ranking_multiplicative_penalty` | `VERA_RANKING_MULTIPLICATIVE_PATH_PENALTY`, `VERA_RANKING_PATH_PENALTY`, `VERA_RANKING_MULTIPLICATIVE_PENALTY` |
| Uniform candidate-pool multiplier | `retrieval.ranking_candidate_pool_multiplier`, `retrieval.ranking_candidate_pool_size_multiplier`, `retrieval.ranking_pool_multiplier` | `VERA_RANKING_CANDIDATE_POOL_MULTIPLIER`, `VERA_RANKING_CANDIDATE_POOL_SIZE_MULTIPLIER`, `VERA_RANKING_POOL_MULTIPLIER`, `VERA_RANKING_CANDIDATE_POOL_MULT` |
| Character-cap chunking | `indexing.chunk_max_chars`, `indexing.max_chunk_chars`, `indexing.max_chunk_characters` | `VERA_INDEXING_CHUNK_MAX_CHARS`, `VERA_INDEXING_MAX_CHUNK_CHARS`, `VERA_MAX_CHUNK_CHARS`, `VERA_CHUNK_MAX_CHARS` |
| Structural graph augmentation | None | `VERA_GRAPH_AUGMENT` |

Config files that contain these keys still load. The keys are ignored and dropped the next time the config is saved, and `vera config get` and `vera config set` report them as unknown. The environment variables no longer have any effect. Query-dependent recall-pool expansion and `vera references` are unaffected.

If you built an index with a nonzero character cap, search and `vera update` refuse it until you rebuild:

```bash
vera index /path/to/repository
```

`vera update` cannot convert such an index, because unchanged files would keep their old chunk boundaries. Indexes with no character-cap setting, or a cap of zero, keep working.

## Embedding request defaults

API indexing now sends two 128-input requests at a time (`embedding.max_in_flight_inputs` is 256, up from 16) with a 120-second timeout (up from 60).

Earlier versions wrote every default into `config.json` whenever it was saved. So when v2 loads such a file, it treats a saved `max_in_flight_inputs` of 16 or `timeout_secs` of 60 as the old defaults and uses the new ones. The next config save writes the new values and adds a `config_format` marker. To keep the old values, set them again:

```bash
vera config set embedding.max_in_flight_inputs 16
vera config set embedding.timeout_secs 60
```

## Indexing and search behavior

- Saved `embedding.query_prefix` and `embedding.document_prefix` values now reach API requests; earlier versions ignored them. If you saved a prefix, rebuild with `vera index` so stored vectors match new queries.
- Saved `indexing.no_ignore`, `indexing.no_default_excludes`, and `indexing.extra_excludes` now apply when the matching flags are not given; earlier versions reset them on every run. `--exclude` now adds to saved globs instead of replacing them.
- A failed API `vera index` leaves `<repo>/.vera.resume/` so the next run can reuse finished embeddings. Add it to `.gitignore` next to `.vera/`.
- An index left half-written by an interrupted `vera update` is refused until `vera update` or `vera index` repairs it.
- Index and update summaries gain request, retry, timeout, and phase-time fields; existing fields are unchanged. Slow runs print progress to stderr even when stderr is not a terminal. `--no-progress` turns this off.
- When `retrieval.reranking_enabled` is true but no reranker is configured, or the reranker cannot start, search prints `reranker unavailable` on stderr and returns unreranked results.
- Error messages from embedding, reranker, and completion APIs no longer include endpoint URLs.

## Docker images

Each image now sets `VERA_BACKEND`: Potion Code for `cpu`, and the matching Jina ONNX backend for `cuda`, `rocm`, and `openvino`. Earlier images ran Jina ONNX on the CPU in every variant.

GPU images use the same model as before, so their indexes still work. Indexes built with an older `cpu` image used Jina, so either rebuild them or keep using Jina:

```bash
# Rebuild with Potion Code
docker run --rm -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:cpu index .

# Or keep Jina on CPU
docker run --rm -i -e VERA_BACKEND=onnx-jina-cpu -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:cpu
```

The image's `VERA_BACKEND` takes priority over a backend saved with `vera setup`. Pass `-e VERA_BACKEND=<backend>` to override it.

## Search scores

See [Search scores](how-it-works.md#search-scores) for how to read the returned ranking values.

## Setup and agent skills

Bare `vera setup` runs the full wizard for local and API backends. Local CPU comes first and is selected by default; API mode is second, with Qwen through OpenRouter recommended among the API presets. The final indexing step defaults to Yes for local backends and No for API mode. Passing an explicit backend flag skips most of the wizard.

The agent selector preselects clients that are already installed. Press Space to toggle a client and Enter to continue. Selected clients are installed or updated; deselecting a client does not remove it, and an empty selection changes nothing. Use `vera agent remove` to uninstall; shared skill directories appear as grouped choices. See [Installation](installation.md#set-up-agent-skills).
