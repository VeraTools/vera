# Install And First Run

## Quick Start

```sh
npx -y @vera-ai/cli install   # or: bunx @vera-ai/cli install / uvx vera-ai install
vera setup --potion-code --index . # configure local CPU and index once
vera search "your query"
```

`vera setup` runs the full wizard: local CPU first and selected by default, API mode second, optional agent skills, and optional indexing. Indexing defaults to Yes for local backends and No for API mode. Explicit backend flags run a shorter configuration flow; add `--index .` to include indexing.

## GPU Backends

```sh
vera setup --onnx-jina-cuda       # NVIDIA GPU (CUDA 12+)
vera setup --onnx-jina-rocm       # AMD GPU (Linux, ROCm)
vera setup --onnx-jina-directml   # DirectX 12 GPU (Windows)
vera setup --onnx-jina-coreml     # Apple Silicon (macOS, M1/M2/M3/M4)
vera setup --onnx-jina-openvino  # Intel GPU/iGPU (Linux only)
```

GPU flags download the matching ONNX Runtime build automatically.

## API Mode

Qwen/OpenRouter is first and recommended among API presets (paid usage, one shared key for embeddings and reranking). OpenAI, Jina, Voyage, and custom OpenAI-compatible endpoints follow:

```sh
vera setup --api
```

For non-interactive setup, export the endpoint values first and add `--yes`:

```sh
export EMBEDDING_MODEL_BASE_URL=https://your-embedding-api/v1
export EMBEDDING_MODEL_ID=your-model
export EMBEDDING_MODEL_API_KEY=your-key
vera setup --api --yes
```

Optional reranker: enter it during interactive setup, or set `RERANKER_MODEL_BASE_URL`, `RERANKER_MODEL_ID`, and `RERANKER_MODEL_API_KEY` before running `vera setup --api --yes`.

## Skill Management

Running `vera agent install` with no flags opens an interactive prompt where you choose the install scope (global, project, or both) and select agents in a paged checklist. Installed clients are preselected. Space toggles a choice; Enter continues. Empty selection changes nothing, and unselected installations remain intact. Use the explicit removal command to uninstall.

```sh
vera agent install                              # interactive: choose scope + agents
vera agent install --client claude              # non-interactive: Claude Code, global
vera agent install --client all --scope project # all agents, project only
vera agent status                               # check install status everywhere
vera agent remove                               # interactive: pick installs to remove
vera agent remove --client codex                # non-interactive: remove Codex global
```

Supported clients: `agents` (universal), `amp`, `antigravity`, `augment`, `claude`, `cline`, `codebuff`, `codebuddy`, `codex`, `copilot`, `cortex`, `crush`, `cursor`, `droid`, `gemini`, `goose`, `iflow`, `junie`, `kilo`, `kimi`, `kiro`, `mux`, `opencode`, `openhands`, `pi`, `qwen`, `roo`, `trae`, `vibe`, `windsurf`, `zed`.

The `agents` client writes to the cross-agent `.agents/skills/` directory, which is the open Agent Skills spec supported by most modern coding agents.

## Upgrading

```sh
vera upgrade              # show update plan
vera upgrade --apply      # execute the binary upgrade
vera agent sync           # sync skill files and refresh Vera-owned project snippets (marked, or verbatim legacy ones)
```

## Uninstalling

```sh
vera uninstall            # removes data dir, agent skills, PATH shim
```

Per-project indexes (`.vera/` in each project) are left in place.

## Diagnostics

```sh
vera doctor   # check config, models, ORT, index health
vera config show # show persisted configuration
vera stats    # index statistics
```
