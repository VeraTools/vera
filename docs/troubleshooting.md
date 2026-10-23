# Troubleshooting

Start with `vera doctor`. It checks the saved and active backend, model files, ONNX Runtime, and config, and suggests a fix for most problems it finds. `vera doctor --probe` adds a deeper read-only test of the local backend.

```bash
vera doctor
vera doctor --probe
vera doctor --probe --json
```

Each of these commands exits with status 1 when a check fails, so `vera doctor && vera index .` stops on a broken setup. Warnings, such as a missing config file or an unindexed directory, leave the exit status at 0.

If your problem is not listed here, ask in [GitHub Issues](https://github.com/VeraTools/Vera/issues) or on [Discord](https://discord.gg/rXNQXCTWDt).

## Install and setup

### `vera` is not found after installing on Windows

The installer writes a shim to your user binary directory. Open a new shell and check:

```powershell
Get-Command vera
vera --version
```

If it is still not found, add the user bin directory that the installer printed to your user `PATH`, then open a new shell.

### Downloads are blocked on your network

If Vera cannot download models or ONNX Runtime, for example on a network that only allows browser downloads, follow [Manual install](manual-install.md).

### Building from source fails to link on macOS

On macOS 14 and later, C dependencies are compiled for your running macOS version while Cargo targets macOS 11.0 by default, so linking fails. Set the deployment target first:

```bash
export MACOSX_DEPLOYMENT_TARGET="$(sw_vers -productVersion)"
cargo build --release
```

## Indexing

### No index found

The repository has not been indexed, or you are outside any indexed directory. From the repository root:

```bash
vera index .
```

Search, grep, structural, references, overview, stats, and update look for the nearest `.vera/` in parent directories, so they also work from subdirectories.

### Index is incomplete

```text
the index at ... is incomplete because an update stopped before it finished writing
```

A `vera update` was interrupted. Vera refuses to read the partial index. Run `vera update .` to repair it or `vera index .` to rebuild.

### `.vera/` shows up in git

See [index-file exclusions](installation.md#index-and-search).

If the index was already tracked, run this from the indexed directory to remove it from git's tracking without deleting local files:

```bash
git rm -r --cached .vera
```

### Indexing a large monorepo is slow

- Add generated trees and vendored code to `.veraignore`.
- Index the project directory you work in rather than the monorepo root.
- Cap a single update run with `vera update . --max-files 250` (pick a value that suits the repository).

## Search results

### Results are stale

Code changed after the last index:

```bash
vera update .
```

This works from any subdirectory. To keep the index current while you edit, run `vera watch .`.

Vera prints the stale-index warning at most once every 10 minutes per index. Set `VERA_STALE_WARNING_ALWAYS=1` to see it on every run.

### A file is missing from results

`vera search` and `vera grep` read only indexed files. When the index is behind your working tree, Vera warns on stderr with counts of added, modified, and deleted files. Run `vera update .`.

If the file still does not appear, `vera explain-path <file>` tells you why it is excluded (ignore rules, binary detection, size limit, or a default exclusion).

### Results are irrelevant

- Describe the behavior you want rather than a single noun.
- Filter with `--lang rust`, `--path "src/**/*.ts"` (repeat for several patterns), or `--type function`.
- For exact strings or regex, use `vera grep`; for counting or files outside the index, use `rg`.

The [query guide](query-guide.md) has more examples.

### Keyword-only results

A stderr warning ending in `using BM25-only search` or `falling back to BM25-only results`, or `warning: semantic search unavailable (...)` in an MCP result, means vector search did not run and results come from keyword matching alone. The message gives the cause:

- The embedding endpoint failed, for example an expired key or a provider outage. See [API mode](#api-mode).
- The index was built with a different embedding model or document prefix than the active one, or with more dimensions than the active model returns. Rebuild with `vera index .`.
- The index has no stored vectors, for example because `.vera/vectors.db` was deleted. Run `vera update .` to re-embed the missing files.

### Reranker unavailable

`Warning: reranker unavailable (...)` on stderr means results were returned without reranking. The text in parentheses gives the cause. If reranking is on but no reranker is configured, configure one ([Models](models.md#reranking)) or turn reranking off:

```bash
vera config set retrieval.reranking_enabled false
```

To check reranking for a query, run `vera search "<query>" --json --rerank-status`. The output includes `reranked`, `reranker`, and `rerank_fallback_reason`.

## API mode

### Reranking does not run after API setup

Before v2.1.4, `vera setup --api` saved the reranker endpoint, including for the Qwen preset, but left reranking off. `vera doctor` warns about this. Run setup again, or turn reranking on directly:

```bash
vera config set retrieval.reranking_enabled true
```

### Setup or requests fail

To index without an embedding provider, use a [keyword-only index](features.md#keyword-only-index).

Re-run setup and enter the endpoint URL, model ID, and key:

```bash
vera setup --api
```

For `vera setup --api --yes`, all three of `EMBEDDING_MODEL_BASE_URL`, `EMBEDDING_MODEL_ID`, and `EMBEDDING_MODEL_API_KEY` must be set. The reranker variables (`RERANKER_MODEL_BASE_URL`, `RERANKER_MODEL_ID`, `RERANKER_MODEL_API_KEY`) must be all set or all unset.

Error messages never contain the endpoint URL or keys. They name the underlying cause, such as `tcp connect error: Connection refused`.

### Indexing stopped partway

Vera retries connection errors, 408, 429, and 5xx responses with backoff, honoring `Retry-After` up to 60 seconds. A 400 response containing `Unable to process`, which some providers send when overloaded, is retried the same way. Other 4xx responses, including authentication errors, fail immediately. When indexing fails, stderr ends with request, retry, and timeout counts followed by the error.

Run `vera index .` again. An API build resumes from the embeddings it already received.

### Rate limits and quota errors

A `429` or `quota exceeded` response is a provider limit. Lowering `embedding.max_concurrent_requests` helps with short burst limits but does not raise your quota; for an exhausted quota, wait for the reset or add billing.

For the reranker, a 429 causes a short retry and then unreranked results. For batch jobs, set `VERA_RERANK_RATE_LIMIT_WAIT_SECS` to a positive number of seconds (for example `65`) to wait for the provider's reported reset, capped at that value per retry. See [Configuration](configuration.md#retrievalranking).

### Batch size errors

If the provider rejects a batch (for example `at most 100 requests can be in one batch`), lower the batch size:

```bash
vera config set embedding.batch_size 100
```

Vera already caps known provider limits, such as 100 inputs per request for Gemini embeddings.

### The index uses a different embedding model

If the active model name differs from the one stored in the index, rebuild with `vera index .`. If the two names are the same model, declare them as aliases instead; see [Changing models](models.md#changing-models).

## Local GPU and ONNX backends

These apply to the opt-in `--onnx-jina-*` backends. The default Potion Code backend does not use ONNX Runtime or a GPU.

### ONNX Runtime or GPU backend fails

Run `vera doctor --probe` first. Common causes:

- **Model files missing or damaged.** Run `vera repair --<backend>`, for example `vera repair --onnx-jina-cuda`. `vera doctor` prints the matching command.
- **ONNX Runtime download failed.** Check the network, or set `ORT_DYLIB_PATH` to an ONNX Runtime library you installed yourself.
- **GPU drivers missing.** CUDA needs CUDA 12 or later, ROCm needs ROCm drivers, DirectML needs DirectX 12, CoreML needs macOS on Apple Silicon.
- **ROCm or OpenVINO runtime install failed.** Vera installs these through pip. If that fails, install `onnxruntime-rocm` or `onnxruntime-openvino` with pip yourself and set `ORT_DYLIB_PATH` to the `libonnxruntime.so` inside the package.
- **Windows DirectML download failed.** This runtime comes from NuGet. Set `ORT_DYLIB_PATH` to `onnxruntime.dll` from the [Microsoft.ML.OnnxRuntime.DirectML package](https://www.nuget.org/packages/Microsoft.ML.OnnxRuntime.DirectML). On Windows machines without a D3D12-capable GPU, setup picks Potion Code instead.

If a GPU session fails after these checks pass, Vera retries on CPU and logs a warning. If that is too slow, fix the GPU provider or switch to `vera setup --potion-code`.

### CUDA libraries are missing

`CUDA backend selected, but required libraries are missing` means ONNX Runtime downloaded, but the system linker cannot find the CUDA or cuDNN libraries it needs. Install them, refresh the linker cache if needed, and run `vera doctor --probe` again.

Vera picks the CUDA 12 or CUDA 13 runtime from your installed toolkit, not the driver. If you switch toolkits, run `vera repair --onnx-jina-cuda`.

### GPU out of memory

Vera sizes batches from available VRAM and retries a failed batch at a smaller size, but GPUs with 4 GB or less can still run out. Use:

```bash
vera index . --low-vram
```

This uses a batch size of 1 and caps the ONNX Runtime memory arena at 1 GB. To make it permanent, run `vera config set embedding.low_vram true`.

### Reranking is slow on Apple Silicon

With CoreML, the reranker runs on CPU because no reranker ONNX export works with CoreML; only embeddings use the GPU. `vera doctor --probe` reports this as `probe-reranker-coreml-cpu`. If reranking is too slow, turn it off with `vera config set retrieval.reranking_enabled false`.

## MCP

### The server does not appear in the client

- Make sure `vera` is on the client's `PATH`. If the client runs with a restricted environment, set `command` to the absolute path of the binary.
- Run `vera mcp` in a terminal to confirm it starts, then check the client's MCP logs.

See [MCP](mcp.md) for client configuration.

## Privacy

With a local backend, and no API reranker or completion endpoint configured, no code leaves your machine. API mode sends queries and chunk text to the configured embedding endpoint. An API reranker receives queries and candidate chunks, and `--deep` with a completion endpoint sends the query to it. The update check contacts GitHub once a day; disable it with `VERA_NO_UPDATE_CHECK=1`.
