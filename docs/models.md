# Models and backends

Vera needs an embedding model for semantic search. It can run one locally or call a hosted API. For keyword search only, skip the model with a [keyword-only index](features.md#keyword-only-index).

## Which backend to use

| Your situation | Backend | Setup command |
|---|---|---|
| Most users, any machine | Potion Code on CPU (the default) | `vera setup --potion-code` |
| You have a supported GPU and want a neural local model | Jina ONNX on GPU | `vera setup --onnx-jina-cuda` (or another `--onnx-jina-*` flag) |
| You want the highest measured quality, or a hosted model | API mode | `vera setup --api` |

Potion Code ([`minishlab/potion-code-16M-v2`](https://huggingface.co/minishlab/potion-code-16M-v2)) is a static embedding model. It runs on the CPU of any supported machine, needs no GPU, no ONNX Runtime, and no API account, and works offline after the first download.

API mode works with any OpenAI-compatible endpoint. The recommended preset is Qwen through OpenRouter (`qwen/qwen3-embedding-8b` for embeddings and `qwen/qwen3-reranker-8b` for reranking, one key, paid usage). On the 320-task Semble subset, this pair with reranking scored 0.8647 nDCG@10 against 0.8538 for the local default, and Recall@1 rose by three percentage points. That was measured on the subset only; see [current results](benchmarks.md#current-results). API mode adds network latency and provider costs, and sends code to the provider (see [API mode](#api-mode)).

Local backends download their assets into Vera's data directory (see [Installation](installation.md)). Jina ONNX backends also download the matching ONNX Runtime library into `lib/`, and some GPU providers need drivers or Python packages; see [Troubleshooting](troubleshooting.md#onnx-runtime-or-gpu-backend-fails).

## Local embedding models

| Model | Setup command | Notes |
|---|---|---|
| Potion Code | `vera setup --potion-code` | Default. CPU only, static embeddings. |
| Jina embeddings v5 nano retrieval | `vera setup --onnx-jina-cuda` or another `--onnx-jina-*` flag | Opt-in, intended for GPUs. Vera prefixes queries with `Query:` and indexed text with `Document:`, as the model expects. |
| CodeRankEmbed | `vera setup --onnx-jina-cuda --code-rank-embed` | Optional preset on the Jina ONNX backend. |

The `--onnx-jina-*` flags select the execution provider: `cpu`, `cuda` (NVIDIA), `rocm` (AMD, Linux), `directml` (Windows), `coreml` (Apple Silicon), and `openvino` (Intel, Linux).

### Embedding screening

Measured 2026-08-21 and 2026-08-22 with reranking off, on an older Vera version than the current one, so absolute scores are lower than today's. The 320-task set is a Semble subset; the independent set has 180 tasks from 10 repositories outside Semble.

| Model | nDCG@10 (320 / independent) | Index time (320 / independent) |
|---|---:|---:|
| `potion-code-16M-v2` | 0.7930 / 0.7019 | 56 s / 26 s |
| `jina-embeddings-v5` nano | 0.7956 / 0.7149 | 147 s / 61 s |
| `CodeRankEmbed` | 0.7949 / 0.7069 | 375 s / 153 s |

The three models land within 0.013 nDCG@10 of each other. Potion Code indexes fastest and runs on every machine, which is why it is the default. Details are in the [benchmark history](benchmarks-history.md).

### Indexing speed

Reference points, each on a different corpus:

| Backend | Hardware | Corpus | Index time |
|---|---|---|---|
| Potion Code | AMD Ryzen 7 9800X3D, CPU | Flask, 227 files, 1,873 chunks | 0.3 s |
| Potion Code | AMD Ryzen 7 9800X3D, CPU | Vera repository, 447 files, 8,109 chunks | 0.9 s |
| Potion Code | AMD Ryzen 7 9800X3D, CPU | Semble suite, 63 repositories | 89 s |
| Jina ONNX, CUDA | NVIDIA RTX 4080 | commons-lang, 11,429 chunks | about 16 s |
| Jina ONNX, CPU | AMD Ryzen 5 7600X3D, 6 cores | Vera repository at the time, 239 files, about 3,100 chunks | about 6 min |

The two small Potion Code rows were measured with v2.1.3. Jina ONNX on CPU works but is slow; on a machine without a GPU, use Potion Code. After the first index, `vera update .` re-embeds only changed files on every backend.

On Apple Silicon, the CoreML backend treats half of system RAM as available GPU memory. Like the other GPU backends, it embeds at most 16 inputs per batch.

## Reranking

A reranker rescores the top candidates after retrieval. It is off by default for local backends, and API setup turns it on when you configure an API reranker. To turn it on yourself:

```bash
vera config set retrieval.reranking_enabled true
```

With reranking on, Vera uses the API reranker if one is configured. Otherwise local backends download and run [`jinaai/jina-reranker-v2-base-multilingual`](https://huggingface.co/jinaai/jina-reranker-v2-base-multilingual) through ONNX Runtime (on CPU for Potion Code).

In local mode you can leave it off. In the 2026-08-23 screening, no tested local cross-encoder beat the default ranking without a reranker (nDCG@10, 320-task subset / independent set):

| Reranker | nDCG@10 |
|---|---:|
| None (default) | 0.8540 / 0.7644 |
| `mxbai-rerank-xsmall-v1` | 0.8497 / 0.7564 |
| `jina-reranker-v2-base-multilingual` | 0.8473 / 0.7557 |
| `gte-reranker-modernbert-base` | 0.8472 / 0.7525 |

The Qwen API reranker was measured separately and is part of the 0.8647 result above.

If you want a local reranker anyway, `mxbai-rerank-xsmall-v1` came closest to the baseline. Select it with these environment variables:

```bash
export LOCAL_RERANKER_REPO=mixedbread-ai/mxbai-rerank-xsmall-v1
export LOCAL_RERANKER_REVISION=b5c6e9da73abc3711f593f705371cdbe9e0fe422
export LOCAL_RERANKER_ONNX_FILE=onnx/model_quantized.onnx
export LOCAL_RERANKER_TOKENIZER_FILE=tokenizer.json
```

Any ONNX cross-encoder on Hugging Face works the same way. If you leave `LOCAL_RERANKER_REVISION` unset, Vera uses the `main` branch. A pinned revision is stored under `models/<repo>/revisions/<revision>/`, so different revisions do not overwrite each other.

## API mode

Run the interactive setup:

```bash
vera setup --api
```

`vera backend --api` does the same without the agent and indexing steps. The menu offers these presets:

| Preset | Embedding model | Reranker | Provider |
|---|---|---|---|
| Qwen (recommended) | `qwen/qwen3-embedding-8b` | `qwen/qwen3-reranker-8b` | [OpenRouter](https://openrouter.ai/) |
| OpenAI | `text-embedding-3-small` | none built in | [OpenAI](https://platform.openai.com/) |
| Jina AI | `jina-embeddings-v3` | `jina-reranker-v2-base-multilingual` | [Jina AI](https://jina.ai/) |
| Voyage AI | `voyage-code-3` | `rerank-2` | [Voyage AI](https://www.voyageai.com/) |
| Custom | any OpenAI-compatible endpoint | optional | your provider |

Check pricing, quotas, and model availability with the provider.

For a reranker, setup also asks for the wire protocol (`auto`, `generic`, or `voyage`), an optional endpoint path, and an optional task instruction. The Qwen preset sets the `generic` protocol for you and skips these questions. Switching to Qwen clears endpoint and task overrides left over from another provider.

When you configure a reranker, setup turns reranking on; setup without a reranker turns it off. Switching to a local backend later drops the saved API reranker. If one was saved, reranking turns off with it, unless your shell sets all three `RERANKER_MODEL_*` variables; then reranking stays on and uses that reranker. If you turned reranking on for a local backend and no API reranker was saved, it stays on.

### Non-interactive setup

Export the endpoint values, then run setup with `--yes`. The three reranker variables must be all set or all unset.

```bash
export EMBEDDING_MODEL_BASE_URL=https://your-embedding-api/v1
export EMBEDDING_MODEL_ID=your-embedding-model
export EMBEDDING_MODEL_API_KEY=your-api-key

# Optional reranker
export RERANKER_MODEL_BASE_URL=https://your-reranker-api/v1
export RERANKER_MODEL_ID=your-reranker-model
export RERANKER_MODEL_API_KEY=your-api-key

vera setup --api --yes

# Optional reranker protocol settings
vera config set retrieval.reranker_protocol generic
vera config set retrieval.reranker_endpoint_path "/rerank"
vera config set retrieval.reranker_task_instruction "Given a query, retrieve relevant code"
```

For Qwen through OpenRouter:

```bash
export EMBEDDING_MODEL_BASE_URL=https://openrouter.ai/api/v1
export EMBEDDING_MODEL_ID=qwen/qwen3-embedding-8b
export EMBEDDING_MODEL_API_KEY=your-openrouter-key
export RERANKER_MODEL_BASE_URL=https://openrouter.ai/api/v1
export RERANKER_MODEL_ID=qwen/qwen3-reranker-8b
export RERANKER_MODEL_API_KEY=your-openrouter-key

vera setup --api --yes
vera config set retrieval.reranker_protocol generic
```

To run your own OpenAI-compatible endpoints locally, see [llama.cpp setup](llama-cpp-setup.md).

### What leaves your machine

API mode sends queries and chunk text to the configured endpoints. The index, BM25 search, and code-aware ranking stay local. If an API reranker is configured, it receives the query and the text of the top candidates. The daily update check is described in [Installation](installation.md).

### Query and document prefixes

Some models expect a prefix on queries or documents. In API mode, Vera picks a prefix from the model ID for known models (Qwen3 embeddings, for example). Override it with `EMBEDDING_QUERY_PREFIX` and `EMBEDDING_DOCUMENT_PREFIX`, or the `embedding.query_prefix` and `embedding.document_prefix` config keys; see [Configuration](configuration.md).

## Custom local embedding models

The Jina ONNX backend can load another ONNX embedding model. These flags do not affect Potion Code or API mode.

From a Hugging Face repository (a full `https://huggingface.co/...` URL also works):

```bash
vera setup --onnx-jina-cuda \
  --embedding-repo Zenabius/CodeRankEmbed-onnx \
  --embedding-pooling cls \
  --embedding-no-onnx-data \
  --embedding-query-prefix "Represent this query for searching relevant code:"
```

From a directory you downloaded or exported yourself:

```bash
vera setup --onnx-jina-cuda \
  --embedding-dir /path/to/model-dir \
  --embedding-onnx-file onnx/model_quantized.onnx \
  --embedding-tokenizer-file tokenizer.json \
  --embedding-dim 768
```

| Flag | Meaning |
|---|---|
| `--code-rank-embed` | Use the built-in CodeRankEmbed preset |
| `--embedding-repo <repo-or-url>` | Download the model from Hugging Face |
| `--embedding-dir <dir>` | Load the model from a local directory |
| `--embedding-onnx-file <path>` | ONNX file inside the repository or directory (default `onnx/model_quantized.onnx`) |
| `--embedding-onnx-data-file <path>` | ONNX external data file (default `onnx/model_quantized.onnx_data`) |
| `--embedding-no-onnx-data` | The model has no external data file |
| `--embedding-tokenizer-file <path>` | Tokenizer file (default `tokenizer.json`) |
| `--embedding-dim <n>` | Embedding dimension the model returns |
| `--embedding-pooling <mode>` | `mean`, `cls`, or `last-token` |
| `--embedding-max-length <n>` | Tokenizer truncation length |
| `--embedding-query-prefix <text>` | Prefix added to queries |
| `--embedding-document-prefix <text>` | Prefix added to indexed text |

## Changing models

The index records which embedding model built it, including every `--embedding-*` setting, so changing the model, pooling, or a prefix requires a rebuild with `vera index .`.

If two model names produce identical embeddings (the same model behind two endpoints, for example), declare them as aliases to skip the rebuild. Set `embedding.model_aliases` or export `VERA_EMBEDDING_MODEL_ALIASES`: groups separated by semicolons, names within a group separated by commas, at least two names per group.

```bash
export VERA_EMBEDDING_MODEL_ALIASES="jina-embeddings-v3,jina-v3;text-embedding-3-small,openai-small"
```

If your network blocks downloads from the command line, see [Manual install](manual-install.md).
