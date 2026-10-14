# Manual install

Use this when Vera cannot download models or ONNX Runtime itself, for example on a network that only allows downloads in a browser.

## 1. Download the files

For the default Potion Code backend, you only need the model. It runs on CPU and does not use ONNX Runtime.

- [`minishlab/potion-code-16M-v2`](https://huggingface.co/minishlab/potion-code-16M-v2): `tokenizer.json`, `model.safetensors`, `config.json`

For the opt-in Jina ONNX backends, download:

- ONNX Runtime for your platform and execution provider, from the [ONNX Runtime releases](https://github.com/microsoft/onnxruntime/releases)
- [`jinaai/jina-embeddings-v5-text-nano-retrieval`](https://huggingface.co/jinaai/jina-embeddings-v5-text-nano-retrieval): `tokenizer.json`, plus `onnx/model_quantized.onnx` and `onnx/model_quantized.onnx_data` for `--onnx-jina-cpu`, or `onnx/model_fp16.onnx` and `onnx/model_fp16.onnx_data` for every GPU backend
- [`jinaai/jina-reranker-v2-base-multilingual`](https://huggingface.co/jinaai/jina-reranker-v2-base-multilingual) if you enable reranking: `onnx/model_quantized.onnx`, `tokenizer.json`

For the CodeRankEmbed preset, download [`Zenabius/CodeRankEmbed-onnx`](https://huggingface.co/Zenabius/CodeRankEmbed-onnx) (`onnx/model_quantized.onnx`, `tokenizer.json`) instead of the Jina embedding model.

Vera pins each curated model to a specific Hugging Face commit, so download the files from that commit (see step 2 for the commit IDs and URL format).

## 2. Place them in Vera's data directory

The data directory is, by default:

| Platform | Path |
|---|---|
| Linux | `~/.local/share/vera/` (or `$XDG_DATA_HOME/vera/`) |
| macOS | `~/Library/Application Support/vera/` |
| Windows | `%APPDATA%\vera\` |

If `~/.vera/` already holds a Vera installation, Vera keeps using it. Set `VERA_HOME` to choose another location. `vera doctor` prints the path of the config file, which sits in the data directory.

Put each file at the same relative path it has in the Hugging Face repository, under these directories:

| Asset | Directory |
|---|---|
| Potion Code | `<data>/models/minishlab/potion-code-16M-v2/revisions/e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b/` |
| Jina embeddings | `<data>/models/jinaai/jina-embeddings-v5-text-nano-retrieval/revisions/ac5d898c8d382b17167c33e5c8af644a3519b47d/` |
| CodeRankEmbed | `<data>/models/Zenabius/CodeRankEmbed-onnx/revisions/e6a6893986a9aaf09a6aa177f42ebc73bb623cca/` |
| Jina reranker | `<data>/models/jinaai/jina-reranker-v2-base-multilingual/revisions/9cfeff2df7d40d1b78e75e5e9cebec92a99813c9/` |

The last path segment is the pinned commit. To download from it in a browser, replace `main` with the commit in the file URL, for example `https://huggingface.co/minishlab/potion-code-16M-v2/resolve/e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b/model.safetensors`.

Keep ONNX Runtime outside the data directory and point Vera at it with `ORT_DYLIB_PATH`. For CPU, CUDA, and CoreML, Vera's `lib/` folder only accepts a runtime it downloaded and checksummed itself, so a hand-placed copy there is downloaded again, and CUDA setup replaces that folder on every run. Extract the archive anywhere, keep its provider libraries next to the main library, and set the variable in your shell profile, because Vera reads it on every run:

```bash
export ORT_DYLIB_PATH=/opt/onnxruntime/lib/libonnxruntime.so   # Linux
# macOS: .../libonnxruntime.dylib   Windows: ...\onnxruntime.dll
```

With `ORT_DYLIB_PATH` set, setup and repair never download or replace ONNX Runtime.

A custom embedding model can stay wherever it is. Point setup at it with `vera setup --onnx-jina-cpu --embedding-dir /path/to/model-dir`; see [Models](models.md#custom-local-embedding-models).

## 3. Run setup and check

Run setup for your backend. It uses the files that are already in place:

```bash
vera setup --potion-code
# or
vera setup --onnx-jina-cuda
# or
vera setup --onnx-jina-cuda --code-rank-embed
```

`vera repair --<backend>` checks the files again and fetches anything still missing or damaged.

Then verify:

```bash
vera doctor
vera doctor --probe
```

## CUDA runtime version

For `--onnx-jina-cuda`, Vera picks the CUDA 12 or CUDA 13 build of ONNX Runtime from the toolkit installed on your machine. It checks `CUDA_PATH`, CUDA's `version.json` or `version.txt`, `nvcc --version`, and on Linux the CUDA libraries visible through `LD_LIBRARY_PATH`, `ldconfig`, and the standard library directories. Download the matching ONNX Runtime 1.24.4 build: `onnxruntime-linux-x64-gpu-1.24.4.tgz` for CUDA 12 or `onnxruntime-linux-x64-gpu_cuda13-1.24.4.tgz` for CUDA 13 (the Windows archives use the same suffixes). If you switch CUDA versions later, point `ORT_DYLIB_PATH` at the matching build.
