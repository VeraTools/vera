# Manual Install

Use this when Vera cannot download ONNX Runtime or model files directly, for example on corporate networks that only allow downloads in a browser.

## 1. Download The Files In A Browser

For the default Potion Code model, download the static embedding model that runs locally on CPU on any supported machine; no GPU or ONNX Runtime needed:

- [`minishlab/potion-code-16M-v2`](https://huggingface.co/minishlab/potion-code-16M-v2)

For the optional Jina ONNX stack, download:

- ONNX Runtime for your backend from the [Microsoft ONNX Runtime releases](https://github.com/microsoft/onnxruntime/releases)
- [`jinaai/jina-embeddings-v5-text-nano-retrieval`](https://huggingface.co/jinaai/jina-embeddings-v5-text-nano-retrieval)
- [`jinaai/jina-reranker-v2-base-multilingual`](https://huggingface.co/jinaai/jina-reranker-v2-base-multilingual)

If you want the optional CodeRankEmbed preset instead of Jina embeddings, download:

- [`Zenabius/CodeRankEmbed-onnx`](https://huggingface.co/Zenabius/CodeRankEmbed-onnx)
- [`jinaai/jina-reranker-v2-base-multilingual`](https://huggingface.co/jinaai/jina-reranker-v2-base-multilingual)

## 2. Place Them In Vera's Data Directory

Vera stores models and libraries in `$XDG_DATA_HOME/vera/` (defaults to `~/.local/share/vera/` on Linux, `~/Library/Application Support/vera/` on macOS). Existing installs using `~/.vera/` continue to work. Set `VERA_HOME` to override.

Default paths:

| Asset | Destination |
| --- | --- |
| ONNX Runtime CPU | `<vera-data>/lib/` |
| ONNX Runtime GPU backend | `<vera-data>/lib/<backend>/` such as `<vera-data>/lib/cuda/` |
| Potion Code | `<vera-data>/models/minishlab/potion-code-16M-v2/` |
| Jina embeddings | `<vera-data>/models/jinaai/jina-embeddings-v5-text-nano-retrieval/` |
| CodeRankEmbed | `<vera-data>/models/Zenabius/CodeRankEmbed-onnx/` |
| Local reranker | `<vera-data>/models/jinaai/jina-reranker-v2-base-multilingual/` |

Expected filenames for the curated presets:

| Model | Files |
| --- | --- |
| Potion Code | `tokenizer.json`, `model.safetensors`, `config.json` |
| Jina embeddings | `onnx/model_quantized.onnx`, `onnx/model_quantized.onnx_data`, `tokenizer.json` (`vera setup --onnx-jina-coreml` and `vera repair --onnx-jina-coreml` use `onnx/model_fp16.onnx` and `onnx/model_fp16.onnx_data` instead) |
| CodeRankEmbed | `onnx/model_quantized.onnx`, `tokenizer.json` |
| Jina reranker | `onnx/model_quantized.onnx`, `tokenizer.json` |

If you want to keep a custom embedding model somewhere else, skip copying it into the models directory and point Vera at it directly with `vera setup --onnx-jina-cpu --embedding-dir /path/to/model-dir`.

## 3. Re-run Setup Or Repair

```bash
vera setup --potion-code
# or
vera setup --onnx-jina-cuda
# or
vera setup --onnx-jina-cuda --code-rank-embed
# or
vera repair --onnx-jina-cuda
```

Then verify:

```bash
vera doctor
vera doctor --probe
```

## Notes

- Set `VERA_HOME` to override the data directory location. Run `vera config` to see the active path.
- Set `ORT_DYLIB_PATH` if you installed ONNX Runtime somewhere else and want Vera to use that exact shared library.
- On Windows CUDA 13, the ONNX Runtime archive name and the folder inside the archive do not match. Current Vera releases handle that layout correctly.
- On CUDA backends, `vera setup --onnx-jina-cuda` and `vera repair --onnx-jina-cuda` refresh the downloaded runtime using the detected toolkit/runtime major version. Vera checks `CUDA_PATH`, CUDA's `version.json` or `version.txt`, `nvcc --version`, and on Linux also CUDA runtime libraries (`LD_LIBRARY_PATH`, `ldconfig`, standard library dirs). If you switch between CUDA 12 and CUDA 13, rerun one of those commands.
