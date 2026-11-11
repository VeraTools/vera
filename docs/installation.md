# Installation

The short version is in the [README quick start](../README.md#quick-start). Setup splits into a one-time setup on your machine (binary, model backend, and global agent skill) and a lightweight per-repository step (`vera index .`). This page covers every install method, backend option, agent integration, updates, and removal.

## Install the binary

Use whichever package runner you already have:

```bash
bunx @vera-ai/cli install               # Bun
npx -y @vera-ai/cli install             # npm
uvx vera-ai install                     # uv
pip install vera-ai && vera-ai install  # pip
```

The installer downloads the native `vera` binary for your platform, puts a `vera` command in your user bin directory, and then opens the agent skill installer (see [Set up agent skills](#set-up-agent-skills)). After that, `vera` is a standalone command and does not need Node or Python.

In CI or any other non-interactive shell, the installer skips the agent step. Extra arguments are passed to `vera agent install`, so `npx -y @vera-ai/cli install --client all --scope global` installs the skill without prompts.

Supported platforms: Linux (x86_64, aarch64), macOS (x86_64, Apple Silicon), and Windows (x86_64). Linux builds need glibc 2.28 or newer (Ubuntu 20.04, Debian 10, RHEL 8, and later). On x86_64 Alpine and other systems without glibc, the installer picks the static musl build automatically. There is no musl build for aarch64, so Linux aarch64 needs glibc.

### Other install methods

**Prebuilt binaries.** Download an archive from [GitHub Releases](https://github.com/VeraTools/Vera/releases) and put `vera` on your `PATH`. Use the `x86_64-unknown-linux-musl` archive on systems without glibc. To force a target with the package installers, set `VERA_TARGET`, for example `VERA_TARGET=x86_64-unknown-linux-musl npx -y @vera-ai/cli install`.

**Build from source** (Rust 1.92 or newer):

```bash
git clone https://github.com/VeraTools/Vera.git && cd Vera
bash scripts/bootstrap-vendored-grammars.sh   # fetches four grammars not stored in git
cargo build --locked --release
cp target/release/vera ~/.local/bin/
```

**Docker.** Images for CPU, CUDA, ROCm, and OpenVINO run the MCP server by default. See [Docker](docker.md).

**Restricted networks.** If Vera cannot download models or ONNX Runtime directly, follow [manual install](manual-install.md).

## Set up a backend

Parsing, SQLite + Tantivy + vector storage, BM25 keyword search, and code-aware ranking always run on your machine. Your backend choice only decides where embeddings (and optional reranking) run. Most setups start with either **local CPU (`--potion-code`)** for offline zero-cost inference or **API mode (`--api`)** with Qwen3 embeddings plus reranking for maximum retrieval quality and agent context savings.

Run the guided wizard once on your machine:

```bash
vera setup
```

It asks for a backend, offers to install the agent skill, and offers to index the current directory (defaulting to Yes for local backends and No for API mode). To skip the wizard, pass a backend flag directly, and add `--index .` if you want to index the current project in the same command (`--yes` skips the confirmation prompt):

```bash
vera setup --potion-code --index .
```

### Local CPU (default)

```bash
vera setup --potion-code
```

Downloads [`potion-code-16M-v2`](https://huggingface.co/minishlab/potion-code-16M-v2), a ~32 MB static code embedding model from MinishLab. It runs on any supported CPU, needs no GPU, ONNX Runtime, or API account, indexes hundreds of files per second, and works offline after the initial download. Pick this when you want zero ongoing cost, offline operation, or local-only privacy.

### API mode

```bash
vera setup --api
```

Sends embedding (and optional reranking) requests to any OpenAI-compatible endpoint while keeping indexing, storage, BM25 search, and code-aware ranking local. The first and recommended preset is Qwen (`qwen/qwen3-embedding-8b` + `qwen/qwen3-reranker-8b`) through OpenRouter, where one paid key covers both models; this pair achieved the highest retrieval score in our subset benchmark and a 48% prompt-token reduction in our agent benchmark. Presets for OpenAI, Jina, and Voyage are included, and custom endpoints let you mix providers (for example, OpenRouter for embeddings and SiliconFlow for lower-cost reranking) or point at a self-hosted [llama.cpp](llama-cpp-setup.md) server. Configuring a reranker enables reranking automatically.

Provider details, privacy notes, and non-interactive setup are in [models: API mode](models.md#api-mode).

### Local GPU

Jina ONNX backends run the [Jina v5 nano](https://huggingface.co/jinaai/jina-embeddings-v5-text-nano-retrieval) embedding model (or a custom ONNX model) on your GPU. Vera downloads the model and the matching ONNX Runtime build.

| Hardware | Command |
|---|---|
| NVIDIA (CUDA 12 or newer) | `vera setup --onnx-jina-cuda` |
| AMD on Linux (ROCm) | `vera setup --onnx-jina-rocm` |
| Apple Silicon (CoreML) | `vera setup --onnx-jina-coreml` |
| Intel GPU or iGPU on Linux (OpenVINO) | `vera setup --onnx-jina-openvino` |
| Any DirectX 12 GPU on Windows (DirectML) | `vera setup --onnx-jina-directml` |

The ROCm and OpenVINO runtimes are installed with pip into a Python environment that Vera manages. A GPU is optional: in model screening, the default CPU model scored within about 0.01 nDCG@10 of Jina and indexed faster. See [models](models.md) for the comparison and for custom ONNX models.

## Index and search

Once your backend and global skill are set up, adding Vera to a new repository only requires indexing its root:

```bash
vera index .
vera search "authentication logic"
```

Inside a git repository, Vera automatically writes `.vera/` and its `.vera.*` working paths to the repository's local exclude file (the path printed by `git rev-parse --git-path info/exclude`, normally `.git/info/exclude`, including linked worktrees). You do not need to edit `.gitignore` unless that exclude file cannot be written or the directory is outside Git.

If you run `vera search` interactively in a directory with no index, Vera offers to create one. Non-interactive agent runs, scripts, and `--json` calls fail immediately with setup guidance instead of blocking on a prompt. Linked git worktrees without an index automatically seed one from an indexed main checkout and update changed files before answering.

## Check your setup

```bash
vera doctor           # config, models, index, and version checks
vera doctor --probe   # also loads the local backend to test it
```

`vera doctor` exits with status 1 if a check fails, so `vera doctor && vera index .` stops early on a broken setup. If model files are missing or damaged, it tells you which `vera repair` command to run.

## Set up agent skills

For coding agents that already have shell access (including Claude Code, OpenCode, Codex, Factory Droid, Cursor, Gemini CLI, GitHub Copilot, Windsurf, Cline, and Roo Code), the CLI paired with the Vera skill is the recommended integration. A skill exposes a short discovery description and loads detailed command instructions on demand, which can be lighter than exposing seven MCP tool schemas in clients that load MCP tool definitions upfront. It also teaches the agent how to choose between `vera search`, `vera structural`, `vera references`, `rg`, and direct file reads.

```bash
vera agent install                                 # interactive: pick agents and global/project scope
vera agent install --client all                    # install globally across supported skill directories
vera agent install --client claude --scope project # one client, this repository only
```

Installing in `global` scope covers future repositories automatically; `vera agent install` writes to the shared `.agents/skills/` convention and client-specific skill directories without building an index. The interactive picker preselects clients where the Vera skill is already installed (Space toggles, Enter confirms) and never removes unselected ones. Use `vera agent status` to inspect installed locations, `vera agent remove` to uninstall, and `vera agent sync` to refresh outdated copies.

In a terminal, the installer (with or without `--client`) also offers to add a short Vera section to your project's `AGENTS.md`, `CLAUDE.md`, `COPILOT.md`, `.cursorrules`, `.clinerules`, or `.windsurfrules`. Vera keeps that section up to date between its `<!-- vera:begin -->` and `<!-- vera:end -->` markers and leaves the rest of the file alone.

To add that section later, run `vera agent install --client <agent>` from the project root in a terminal and accept the prompt. Without a terminal the prompt is skipped, but an existing managed section in a Markdown config file is still refreshed.

If you use the [skills CLI](https://github.com/vercel-labs/skills), `npx skills add VeraTools/Vera` installs the same skill.

For MCP clients, see [MCP](mcp.md).

## Update

Vera checks for a new release once a day and prints a note when one is out. To update:

```bash
vera upgrade           # show what would run
vera upgrade --apply   # run it
```

After an upgrade, Vera refreshes outdated agent skill copies on its own. Set `VERA_NO_UPDATE_CHECK=1` to turn off the daily check.

`vera upgrade --apply` updates the binary installed by the `install` command. It does not touch a global npm or Bun package; update that with your package manager if you use its `vera-ai` command. If an upgrade fails, rerun the installer you used originally:

```bash
bunx @vera-ai/cli@latest install
npx -y @vera-ai/cli@latest install
uvx vera-ai@latest install
pip install --upgrade vera-ai && vera-ai install
```

Coming from 1.x? Read the [v2 migration notes](migration-v2.md) first.

## Uninstall

```bash
vera uninstall
```

This removes the Vera binary, downloaded models and runtimes, configuration and saved credentials, installed agent skills, and the `vera` command shim. If the data directory contains files Vera did not create, it is left in place and reported. Project indexes (`.vera/` in each project) are not touched; delete them yourself if you want them gone.

## Where Vera keeps files

| What | Where |
|---|---|
| Project index | `.vera/` in the indexed directory |
| Models, runtimes, and config | `~/.local/share/vera/` (or `$XDG_DATA_HOME/vera/`) on Linux, `~/Library/Application Support/vera/` on macOS, `%APPDATA%\vera\` on Windows, or `~/.vera/` for installs that already use it |

Set `VERA_HOME` to use a different data directory.

If something goes wrong, see [troubleshooting](troubleshooting.md).
