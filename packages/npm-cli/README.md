# @vera-ai/cli

Code search for AI agents. Vera indexes your codebase using tree-sitter parsing and hybrid search (BM25 + vector similarity + optional cross-encoder reranking), then returns ranked code snippets as Markdown codeblocks by default, or JSON with `--json`.

This package downloads and wraps the native Vera binary for your platform. It downloads the exact package version, checks the release archive size and SHA-256, and stores a completed binary cache. Later runs use that cache without network access. Older caches are downloaded and verified once before they can be reused offline. On Linux without glibc (such as Alpine or NixOS), the wrapper selects the musl binary. Other native targets use their platform system libraries; optional ONNX backends have additional runtime requirements. Set `VERA_TARGET` to override target detection (e.g., `VERA_TARGET=x86_64-unknown-linux-musl npm install -g @vera-ai/cli`).

The default local embedding model, `minishlab/potion-code-16M-v2`, runs on CPU and works offline after its first download. API mode is also available; Qwen/OpenRouter is recommended among API presets (paid usage). The [benchmark report](https://github.com/VeraTools/Vera/blob/master/docs/benchmarks.md) separates the full 1,251-task suite from smaller screening subsets, and the [agent benchmark](https://github.com/VeraTools/Vera/blob/master/docs/benchmarks-history.md#agent-level-benchmark) records context measurements and their sample limits.

## Install

```bash
npm install -g @vera-ai/cli
```

## Quick Start

```bash
vera setup --potion-code --index .
vera search "authentication logic"
```

`vera setup` runs the full wizard: configure a backend, optionally install agent skills, and optionally index. Local CPU is first and selected by default; API mode is second. Indexing defaults to Yes for local backends and No for API mode. `vera setup --api` uses the shorter API configuration flow, with Qwen/OpenRouter first and recommended (one shared key). See the [models guide](https://github.com/VeraTools/Vera/blob/master/docs/models.md#api-mode) for other providers and non-interactive setup.

The preferred agent integration is the CLI plus the Vera skill: `vera agent install` installs it for supported coding agents and can add a short usage snippet to your project's `AGENTS.md`, `CLAUDE.md`, `COPILOT.md`, or editor rules file. The client selector uses Space to toggle and Enter to continue. Empty selection changes nothing; unselected installations remain intact. Vera also ships an optional MCP server (`vera mcp`); see the [MCP guide](https://github.com/VeraTools/Vera/blob/master/docs/mcp.md) if your client is MCP-first.

## Common Tasks

| Task | Command |
|------|---------|
| Use the interactive setup wizard | `vera setup` |
| Use the default local model | `vera setup --potion-code` |
| Configure API mode | `vera setup --api` |
| Use a local NVIDIA backend | `vera setup --onnx-jina-cuda` |
| Search semantically | `vera search "authentication middleware"` |
| Search only changed files | `vera search "authentication middleware" --changed` |
| Common structural tasks | `vera structural routes` / `vera structural env DATABASE_URL` / `vera structural impls Loader` |
| Find callers or callees | `vera references foo` / `vera references foo --callees` |
| Explain why a file is missing | `vera explain-path path/to/file` |
| Inspect index health | `vera stats --json` |
| Keep the index up to date | `vera update .` |
| Watch for file changes | `vera watch .` |
| Run local HTTP inference server | `vera serve` |
| Diagnose setup issues | `vera doctor` |
| Run the deeper local probe | `vera doctor --probe` |
| Repair missing local assets | `vera repair` |
| Inspect binary upgrades | `vera upgrade` |
| Install agent skills | `vera agent install` |

For the full backend matrix, model options, Docker setup, and troubleshooting, see the main [README](https://github.com/VeraTools/Vera) and [Installation Guide](https://github.com/VeraTools/Vera/blob/master/docs/installation.md).

## What you get

- **65 languages** (61 with tree-sitter AST parsing)
- **Hybrid search**: BM25 keyword + vector similarity, fused with Reciprocal Rank Fusion
- **Opt-in cross-encoder reranking** for precision, disabled by default
- **Git-aware scopes and index debugging**: `--changed` / `--since` / `--base`, `explain-path`, and index health in `vera stats`
- **Markdown codeblock output** by default with file paths, line ranges, and optional symbol info (use `--json` for compact JSON; `--raw` works with `vera search`, `vera grep`, and `vera references`; `--timing` works with `vera search` and `vera grep`, before or after the subcommand)

For full documentation, including local model options and manual install steps, see the [GitHub repo](https://github.com/VeraTools/Vera).
