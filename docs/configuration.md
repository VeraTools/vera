# Configuration

`vera config show` prints the saved configuration. Use `vera config get <key>` and `vera config set <key> <value>` for individual saved values. Runtime precedence is valid environment variable > saved config > built-in default. Environment values are never saved to `config.json`; invalid overrides keep the saved value or default. Aliases are listed together below.

## Backend/API

Backend selection adjusts effective embedding parameters: Potion Code uses a batch ceiling of 1024 and one worker; ONNX CPU uses 4 and one worker; GPU backends use batches of 16, one worker, and shrink the batch to 4 below 3 GB of free GPU memory and to 1 below 512 MB; `embedding.low_vram` always uses 1. Every backend is also capped by `embedding.max_in_flight_inputs`, so Potion Code embeds at most 256 inputs at a time by default. Potion Code stores at most 256 vector dimensions. Saved values shown by `vera config show` can differ from these runtime limits.

### `vera config` keys

| Name | Default | What it does |
|---|---:|---|
| `embedding.batch_size` | 128 API | Number of inputs in an embedding request. |
| `embedding.max_concurrent_requests` | 8 API | Maximum concurrent embedding requests. |
| `embedding.max_in_flight_inputs` | 256 | Bounds active embedding inputs across requests; with API defaults this sends two 128-input requests at a time. |
| `embedding.timeout_secs` | 120 | Embedding request timeout. |
| `embedding.max_retries` | 3 | Retries transient embedding errors. |
| `embedding.max_stored_dim` | 1024 | Truncates stored vectors above this dimension; `0` stores full vectors. |
| `embedding.gpu_mem_limit_mb` | 0 | ONNX CUDA memory limit in MB; `0` uses the runtime default. |
| `embedding.low_vram` | `false` | Uses conservative GPU settings. |
| `embedding.query_prefix` | `null` | Overrides the API model's query prefix; `""` disables it. A non-empty `EMBEDDING_QUERY_PREFIX` wins. |
| `embedding.document_prefix` | `null` | Overrides the API model's document prefix; `""` disables it. A non-empty `EMBEDDING_DOCUMENT_PREFIX` wins. |
| `embedding.model_aliases` | `[]` | Groups provider model names that have verified-compatible embeddings. |

### Environment variables

| Name | Default | What it does |
|---|---|---|
| `VERA_BACKEND` | saved backend, then API | Selects `potion-code-cpu`, `api`, or `onnx-jina-<provider>`; explicit CLI backend flags take precedence. |
| `VERA_LOCAL` | unset | Legacy `1` or `true` selects ONNX CPU when no explicit or saved backend applies. |
| `VERA_HOME` | platform default | Overrides Vera's data directory. |
| `EMBEDDING_MODEL_ID` | unset (required for API) | Selects the embedding model. |
| `EMBEDDING_MODEL_BASE_URL` | unset (required for API) | Sets the embedding API base URL. |
| `EMBEDDING_MODEL_API_KEY` | unset (required for API) | Credentials for the embedding API. |
| `EMBEDDING_QUERY_PREFIX` | unset | Prefixes embedding queries. |
| `EMBEDDING_DOCUMENT_PREFIX` | unset | Prefixes embedded documents. |
| `VERA_EMBEDDING_QUERY_PREFIX` | unset | Vera-specific query-prefix override. |
| `VERA_MAX_IN_FLIGHT_INPUTS` | 256 | Sets `embedding.max_in_flight_inputs`; values below 1 normalize to 1. |
| `VERA_EMBEDDING_MODEL_ALIASES` | unset | Defines semicolon-separated, comma-separated embedding alias groups. |
| `RERANKER_MODEL_ID` | unset | Selects the reranker model. |
| `RERANKER_MODEL_BASE_URL` | unset | Sets the reranker API base URL. |
| `RERANKER_MODEL_API_KEY` | unset | Credentials for the reranker API. |
| `VERA_COMPLETION_MODEL_ID` | unset | Selects the completion model used by completion features. |
| `VERA_COMPLETION_BASE_URL` | unset | Sets the completion API base URL. |
| `VERA_COMPLETION_API_KEY` | `none` | Credentials for the completion API. |
| `VERA_COMPLETION_MAX_TOKENS` | 16384 | Limits completion output tokens. |
| `VERA_COMPLETION_MAX_ALTERNATIVES` | 2 | Limits completion alternatives. |
| `VERA_COMPLETION_TIMEOUT_SECS` | 120 | Sets the completion request timeout. |
| `VERA_SERVE_KEY` | unset | Authenticates requests to `vera serve`. |

## Retrieval/ranking

### `vera config` keys

| Name | Default | What it does |
|---|---:|---|
| `retrieval.default_limit` | 5 | Number of results returned by default. |
| `retrieval.rrf_k` | 60.0 | Reciprocal Rank Fusion constant. |
| `retrieval.rerank_candidates` | 50 | Candidates passed to the reranker. |
| `retrieval.reranking_enabled` | `false` | Enables reranking. Uses the API reranker when `RERANKER_MODEL_*` is configured; otherwise local backends download and run the bundled local reranker, and the API backend warns that no reranker is configured. |
| `retrieval.max_output_chars` | 0 | Total search-output character budget; `0` is unlimited. |
| `retrieval.max_rerank_batch` | 20 | Documents per reranker request; `0` disables batching. |
| `retrieval.reranker_protocol` (`rerank_protocol`) | auto | Selects `generic` or `voyage` wire format. |
| `retrieval.reranker_endpoint_path` (`rerank_endpoint_path`, `endpoint_path`) | auto | Overrides the reranker endpoint path. |
| `retrieval.reranker_task_instruction` (`rerank_task_instruction`, `task_instruction`) | `null` | Adds reranker scoring guidance. |
| `retrieval.reranker_task_field` (`rerank_task_field`, `task_field`) | `null` | Names the wire field for the task instruction. |
| `retrieval.reranker_max_doc_chars` (`rerank_max_doc_chars`, `max_rerank_doc_chars`) | 4800 | Character budget per reranker document; `0` is unlimited. |
| `retrieval.reranker_timeout_secs` (`rerank_timeout_secs`, `timeout_secs`) | 30 | Reranker request timeout. |
| `retrieval.reranker_max_retries` (`rerank_max_retries`) | 2 | Retries transient reranker errors. |
| `retrieval.reranker_rate_limit_wait_secs` (`rerank_rate_limit_wait_secs`, `rate_limit_wait_secs`) | `null` | Positive values cap waits for a reported 429 quota reset; `null` or `0` uses short generic retries, then unreranked results. |
| `retrieval.reranker_return_documents` (`rerank_return_documents`, `return_documents`) | `false` | Controls whether reranker responses include document text. |
| `retrieval.ranking_filename_stem_boost` | `true` | Boosts files whose names match query keywords. |
| `retrieval.ranking_filename_stem_min_ratio` | 0.05 | Minimum filename-stem match ratio for that boost. |
| `retrieval.ranking_filename_stem_skip_symbol_queries` | `false` | Skips filename-stem boosting for symbol queries. |
| `retrieval.ranking_definition_boost` | `true` | Boosts matching definitions. |
| `retrieval.ranking_recall_pool_expansion` | `true` | Expands the recall pool before final ranking. |
| `retrieval.vector_filter_during_scan` | `true` | Applies eligible path and language filters during vector scanning. |

### Environment variables

| Name | Default | What it does |
|---|---:|---|
| `VERA_MAX_OUTPUT_CHARS` | 0 | Sets the search-output character budget. |
| `VERA_MAX_RERANK_BATCH` | 20 | Sets the reranker batch size. |
| `VERA_MAX_RERANK_DOC_CHARS` | 4800 | Sets the reranker document character budget. |
| `VERA_RERANK_TIMEOUT_SECS` | 30 | Sets the reranker timeout. |
| `VERA_RERANK_MAX_RETRIES` | 2 | Sets reranker retries. |
| `VERA_RERANK_RATE_LIMIT_WAIT_SECS` | unset | Positive seconds enable capped quota-reset waits; `0` keeps short generic retries. |
| `VERA_RANKING_FILENAME_STEM_BOOST` | `true` | Enables filename-stem boosting. |
| `VERA_RANKING_FILENAME_STEM_MIN_RATIO` | 0.05 | Sets the filename-stem match ratio. |
| `VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES` | `false` | Skips that boost for symbol queries. |
| `VERA_RANKING_DEFINITION_BOOST` | `true` | Enables definition-content boosting. |
| `VERA_RANKING_RECALL_POOL_EXPANSION` | `true` | Enables recall-pool expansion. |
| `VERA_VECTOR_FILTER_DURING_SCAN` | `true` | Enables filtering during flat vector scans. |
| `VERA_VECTOR_SCAN` | flat | Selects the flat SIMD scan or `vec0` fallback. |

## Indexing

### `vera config` keys

| Name | Default | What it does |
|---|---:|---|
| `indexing.max_chunk_lines` | 200 | Maximum lines in a chunk before splitting. |
| `indexing.max_file_size_bytes` | 1000000 | Skips files larger than this size. |
| `indexing.default_excludes` | built-in list | Adds default exclusions to `.gitignore` rules. |
| `indexing.extra_excludes` | `[]` | Exclusion globs; `--exclude` adds to them. |
| `indexing.no_ignore` | `false` | Disables `.gitignore` and `.veraignore` parsing. |
| `indexing.no_default_excludes` | `false` | Disables smart default exclusions. |
| `indexing.max_chunk_bytes` | 24576 | Splits oversized embedding chunks at line boundaries; `0` disables this cap. |

### Environment variables

| Name | Default | What it does |
|---|---:|---|
| `VERA_MAX_CHUNK_BYTES` | 24576 | Overrides the byte chunk cap. |

## Runtime/misc

| Name | Default | What it does |
|---|---:|---|
| `VERA_LOG` | info | Sets the logging filter. |
| `VERA_STALE_WARNING_ALWAYS` | unset | Prints the stale-index warning on every run instead of at most once per 10 minutes per index. |
| `VERA_NO_UPDATE_CHECK` | unset | Disables the daily GitHub update check when set to `1`. |
| `VERA_USER_BIN_DIR` | platform default | Selects the user binary directory used by installers. |
| `VERA_USER_HOME` | platform default | Overrides the user home used by the CLI. |
| `VERA_LOCAL_EMBEDDING_DIM` | model default | Sets local embedding dimensionality. |
| `VERA_LOCAL_EMBEDDING_DIR` | data directory | Sets the local embedding asset directory. |
| `VERA_LOCAL_EMBEDDING_MAX_LENGTH` | model default | Sets the local model input length. |
| `VERA_LOCAL_EMBEDDING_ONNX_DATA_FILE` | model default | Overrides the local ONNX data file. |
| `VERA_LOCAL_EMBEDDING_ONNX_FILE` | model default | Overrides the local ONNX model file. |
| `VERA_LOCAL_EMBEDDING_POOLING` | model default | Selects local embedding pooling. |
| `VERA_LOCAL_EMBEDDING_QUERY_PREFIX` | model default | Overrides the local query prefix. |
| `VERA_LOCAL_EMBEDDING_DOCUMENT_PREFIX` | model default | Overrides the local document prefix. |
| `VERA_LOCAL_EMBEDDING_REPO` | model default | Selects the local model repository. |
| `VERA_LOCAL_EMBEDDING_REVISION` | model default | Selects the local model revision. |
| `VERA_LOCAL_EMBEDDING_TOKENIZER_FILE` | model default | Overrides the local tokenizer file. |
| `LOCAL_RERANKER_REPO` | model default | Selects the local reranker repository. |
| `LOCAL_RERANKER_REVISION` | model default | Selects the local reranker revision. |
| `LOCAL_RERANKER_ONNX_FILE` | model default | Overrides the local reranker ONNX file. |
| `LOCAL_RERANKER_TOKENIZER_FILE` | model default | Overrides the local reranker tokenizer file. |
