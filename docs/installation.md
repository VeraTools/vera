# Installation Guide

For the short path, use the [Quick Start](../README.md#quick-start) in the README.

## Install the Binary

Pick whichever package manager you have:

```bash
bunx @vera-ai/cli install               # Bun
npx -y @vera-ai/cli install             # npm
uvx vera-ai install                     # Python (uv)
pip install vera-ai && vera-ai install  # Python (pip)
```

The installer downloads the `vera` binary for your platform, writes a shim to a user bin directory, and delegates to `vera agent install`, which launches an interactive scope and client selector to install skill files. After that, `vera` is a standalone command.

Without a terminal on stdin and stderr, a bare `install` skips the agent step and succeeds. Run `vera agent install` later in a terminal, or `vera agent install --client all --scope global` without one. Extra arguments to the installer are passed to `vera agent install`.

<details>
<summary>Other install methods</summary>

**Prebuilt binaries:**
Download from [GitHub Releases](https://github.com/VeraTools/Vera/releases) for Linux (x86_64, aarch64), macOS (x86_64, aarch64), or Windows (x86_64). For Alpine, NixOS, or minimal containers without glibc, use the `x86_64-unknown-linux-musl` archive (static binary for the default CPU backend). GNU Linux archives require glibc 2.28 or newer; macOS and Windows binaries use system libraries. Optional ONNX backends also need ONNX Runtime and provider dependencies. The npm/pip wrappers select musl when glibc is unavailable; to force a specific target, set `VERA_TARGET=x86_64-unknown-linux-musl` before running the install command.

**Build from source** (Rust 1.88+):
```bash
git clone https://github.com/VeraTools/Vera.git && cd Vera
bash scripts/bootstrap-vendored-grammars.sh   # downloads the four grammars that are not tracked in git
cargo build --locked --release
cp target/release/vera ~/.local/bin/
vera setup
```

**Docker** (MCP server):
```bash
docker run --rm -i -v "$(pwd):/workspace" ghcr.io/veratools/vera:cpu
```
CPU, CUDA, ROCm, and OpenVINO images available. See [docker.md](docker.md).

**Manual install:** [manual-install.md](manual-install.md)

</details>

## Set Up a Backend

Vera stores indexes and retrieves candidates locally. The "backend" only controls where embedding and reranking models run.

Run `vera setup` for the full wizard: configure a backend, optionally install agent skills, and optionally index the current project. Potion Code CPU is first and selected by default; API mode is second. Indexing defaults to Yes for local backends and No for API mode. Explicit backend flags run a shorter configuration flow.

The default embedding model is `minishlab/potion-code-16M-v2`. It runs on CPU and works offline after its assets have downloaded. Jina ONNX and CodeRankEmbed are opt-in alternatives.

### CPU Local Mode

The default `minishlab/potion-code-16M-v2` model runs locally on CPU on any supported machine.

```bash
vera setup --potion-code
```

Use this when you want the default local model. It also runs on CPU-only machines, and the interactive `vera setup` wizard selects it as the default local backend.

### API Mode

Use an OpenAI-compatible endpoint for embedding and optional reranking calls:

```bash
vera setup --api
```

Qwen/OpenRouter is first and recommended in the API selector (paid usage, one shared key). OpenAI, Jina, Voyage, and custom endpoints are also available. Setup saves the endpoints, credentials, and reranker protocol settings together. `vera backend --api` uses the same configuration prompts.

See [Models: API mode](models.md#api-mode) for provider links, non-interactive environment examples, and protocol overrides. Provider pricing and quotas can change; check the provider before indexing a large project.

### GPU Local Mode

Jina ONNX is an opt-in local backend. Vera downloads the Jina embedding model and local reranker, then uses your GPU provider. No API key is needed, and the setup works offline after the download.

**Pick the right command for your hardware:**

| You have | Command | What happens |
|----------|---------|-------------|
| Not sure | `vera setup` | Full wizard, with local CPU selected by default |
| CPU only | `vera setup --potion-code` | Uses the default `minishlab/potion-code-16M-v2` model |
| Apple Silicon (M1/M2/M3/M4) | `vera setup --onnx-jina-coreml` | Uses CoreML GPU acceleration |
| NVIDIA GPU | `vera setup --onnx-jina-cuda` | Uses CUDA |
| AMD GPU (Linux) | `vera setup --onnx-jina-rocm` | Uses ROCm |
| Intel GPU (Linux) | `vera setup --onnx-jina-openvino` | Uses OpenVINO |
| DirectX 12 GPU (Windows) | `vera setup --onnx-jina-directml` | Uses DirectML |

For custom ONNX models, GPU-specific tuning, and inference speed comparisons, see [models.md](models.md).

## Verify Your Setup

```bash
vera doctor          # checks config, models, and connectivity
vera doctor --probe  # deeper local backend diagnostics
```

## Index and Search

Add `--index .` to an explicit setup command to configure the backend and index the current project in one step:

```bash
vera setup --potion-code --index .
vera search "authentication logic"
```

The bare wizard includes this indexing step; explicit setup flags need `--index .` to include it. If you skip indexing, an interactive `vera search` offers to create the missing index. JSON and non-interactive searches return the existing missing-index error instead of prompting.

See the [query guide](query-guide.md) for tips on writing effective queries.

## Set Up Agent Skills

Vera can install skill files so your AI coding agents know how to use it:

```bash
vera agent install              # interactive: choose scope + agents
vera agent install --client all # non-interactive: all agents, global
```

The selector preselects installed clients. Press Space to toggle and Enter to continue; an empty selection makes no changes. Installation adds or updates selected clients and leaves unselected installations in place. Shared skill directories are written once. Use `vera agent remove` for removal; its interactive choices group clients that share a directory.

The interactive flow can also update your project's `AGENTS.md`, `CLAUDE.md`, `COPILOT.md`, `.cursorrules`, `.clinerules`, or `.windsurfrules` file with a short Vera usage snippet.

<details>
<summary>Add the instructions manually</summary>

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

`vera structural impls <symbol>` only finds explicit declarations such as `implements`, `extends`, `with`, `:`, or `impl Trait for Type`. It does not infer implicit interface satisfaction.

</details>

<details>
<summary>Use the Vercel skills CLI instead</summary>

```bash
npx skills add VeraTools/Vera
```

</details>

## Updating

Upgrading from v1? Read the [v2 migration notes](migration-v2.md) for retired controls and experimental indexes that need rebuilding.

Vera checks for new releases daily and prints a hint when one is available.

```bash
vera upgrade              # dry run: shows what would happen
vera upgrade --apply      # applies the update
```

After an upgrade, Vera automatically syncs stale agent skill installs. Set `VERA_NO_UPDATE_CHECK=1` to disable the automatic check.

`--apply` runs the installer for the new version without a terminal, so it skips the agent selector. It does not change a global npm or Bun package; update that yourself if you use its `vera-ai` command.

If you are having trouble updating, reinstall with the package manager you originally used:

```bash
# Bun
bunx @vera-ai/cli@latest install
# npm
npx -y @vera-ai/cli@latest install
# uv
uvx vera-ai@latest install
# pip
pip install --upgrade vera-ai && vera-ai install
```

## Uninstalling

```bash
vera uninstall   # removes Vera data, agent skills, and its PATH shim
```

A data directory containing files Vera did not create is left in place and reported.

## Troubleshooting

- Run `vera doctor` to diagnose issues.
- Run `vera doctor --probe` for deeper local backend diagnostics.
- Wrong backend? Run `vera setup` again with a different flag.
- Slow opt-in Jina indexing on CPU? Switch to `--potion-code`, `--api`, or a GPU backend.
- See [troubleshooting.md](troubleshooting.md) for more.
