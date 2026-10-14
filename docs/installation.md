# Installation

The short version is in the [README quick start](../README.md#quick-start). This page covers every install method, backend setup, agent integration, updates, and removal.

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

**Build from source** (Rust 1.88 or newer):

```bash
git clone https://github.com/VeraTools/Vera.git && cd Vera
bash scripts/bootstrap-vendored-grammars.sh   # fetches four grammars not stored in git
cargo build --locked --release
cp target/release/vera ~/.local/bin/
```

**Docker.** Images for CPU, CUDA, ROCm, and OpenVINO run the MCP server by default. See [Docker](docker.md).

**Restricted networks.** If Vera cannot download models or ONNX Runtime directly, follow [manual install](manual-install.md).

## Set up a backend

Vera always indexes, stores, and searches on your machine. The backend only decides where the embedding model (and the optional reranker) runs.

Run the guided setup:

```bash
vera setup
```

It asks for a backend, offers to install the agent skill, and offers to index the current directory. Local CPU is the default choice. To skip the wizard, pass a backend flag, and add `--index .` to index in the same step. In a terminal, setup then asks one confirmation; add `--yes` to skip it:

```bash
vera setup --potion-code --index .
```

### Local CPU (default)

```bash
vera setup --potion-code
```

This downloads [`potion-code-16M-v2`](https://huggingface.co/minishlab/potion-code-16M-v2), a 32 MB static embedding model for code. It runs on any supported machine, needs no GPU or ONNX Runtime, and works offline after the download. Use this unless you have a specific reason to pick something else.

### Local GPU

Jina ONNX backends run the [Jina v5 nano](https://huggingface.co/jinaai/jina-embeddings-v5-text-nano-retrieval) embedding model on your GPU. Vera downloads the model and the matching ONNX Runtime build.

| Hardware | Command |
|---|---|
| NVIDIA (CUDA 12 or newer) | `vera setup --onnx-jina-cuda` |
| AMD on Linux (ROCm) | `vera setup --onnx-jina-rocm` |
| Apple Silicon (CoreML) | `vera setup --onnx-jina-coreml` |
| Intel GPU or iGPU on Linux (OpenVINO) | `vera setup --onnx-jina-openvino` |
| Any DirectX 12 GPU on Windows (DirectML) | `vera setup --onnx-jina-directml` |

The ROCm and OpenVINO runtimes are installed with pip into a Python environment that Vera manages. A GPU is optional: in model screening, the default CPU model scored within about 0.01 nDCG@10 of Jina and indexed faster. See [models](models.md) for the comparison and for custom ONNX models.

### API mode

```bash
vera setup --api
```

API mode sends embedding (and optionally reranking) requests to any OpenAI-compatible endpoint. The setup menu offers presets for Qwen through OpenRouter (recommended: one key covers embeddings and reranking, paid usage), OpenAI, Jina, and Voyage, plus a custom endpoint for anything else, including a local [llama.cpp](llama-cpp-setup.md) server. If you give setup a reranker, it turns reranking on.

Provider details and non-interactive setup are in [models: API mode](models.md#api-mode).

## Index and search

If you did not pass `--index .` during setup:

```bash
vera index .
vera search "authentication logic"
```

Add the index directory to `.gitignore`. A failed API-mode index run also leaves a resume directory, so add both:

```bash
printf '.vera/\n.vera.resume/\n' >> .gitignore
```

If you search in a project without an index, an interactive `vera search` offers to create one. Scripts and `--json` searches get an error instead, so they can handle it.

## Check your setup

```bash
vera doctor           # config, models, index, and version checks
vera doctor --probe   # also loads the local backend to test it
```

`vera doctor` exits with status 1 if a check fails, so `vera doctor && vera index .` stops early on a broken setup. If model files are missing or damaged, it tells you which `vera repair` command to run.

## Set up agent skills

The Vera skill teaches coding agents when to use Vera, how to phrase queries, and when plain `grep` is the better tool. It supports more than 30 agent clients, including Claude Code, Codex, Cursor, OpenCode, Gemini CLI, GitHub Copilot, Windsurf, Cline, Roo Code, Junie, and Factory Droid.

```bash
vera agent install                                 # interactive: pick agents and scope
vera agent install --client all                    # all agents, global scope
vera agent install --client claude --scope project # one agent, this project only
```

The interactive picker preselects agents you already have installed. Press Space to toggle and Enter to continue. It only adds or updates the agents you select and never removes others. Use `vera agent status` to see what is installed, `vera agent remove` to uninstall, and `vera agent sync` to refresh outdated copies.

In a terminal, the installer (with or without `--client`) also offers to add a short Vera section to your project's `AGENTS.md`, `CLAUDE.md`, `COPILOT.md`, `.cursorrules`, `.clinerules`, or `.windsurfrules`. Vera keeps that section up to date between its `<!-- vera:begin -->` and `<!-- vera:end -->` markers and leaves the rest of the file alone.

<details>
Add the agent instructions by hand

```markdown
## Code Search

<!-- vera:begin -->

Use Vera before opening many files or running broad text search when you need to find where logic lives or how a feature works.

- `vera search "query"` for semantic code search. Describe behavior: "JWT validation", not "auth". If one phrasing misses, try 2-3 varied queries or add `--intent "goal"`.
- `vera search ... --changed`, `--since <rev>`, or `--base <rev>` when the task is limited to modified files or a PR diff
- `vera grep "pattern"` for exact text or regex in indexed files
- `vera structural definitions <symbol>`, `vera structural env <NAME>`, `vera structural routes`, or `vera structural impls <symbol>` for common structural tasks and explicit type relationships
- `vera explain-path path/to/file` to explain why a file is or is not indexed
- `vera references <symbol>` for callers and `vera references <symbol> --callees` for callees
- `vera overview` for a project summary (languages, entry points, hotspots). Add `--changed`, `--since <rev>`, or `--base <rev>` to scope it to modified files.
- `vera stats --json` for index health, including tree-sitter error, parse-failure, and Tier 0 fallback counts
- `vera search --deep "query"` for RAG-fusion query expansion + merged ranking
- Narrow `vera search` or `vera grep` with `--lang`, `--path`, `--type`, or `--scope docs`
- `vera watch .` to auto-update the index, or `vera update .` after edits (`vera index .` if `.vera/` is missing)
- For detailed usage, query patterns, and troubleshooting, read the Vera skill file installed by `vera agent install`
<!-- vera:end -->
```

</details>

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
