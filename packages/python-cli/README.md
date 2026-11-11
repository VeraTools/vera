# vera-ai

Vera is code retrieval and structural navigation for AI coding agents and developers, built to spend less context finding the right code. Describe what a piece of code does, look up where a symbol is defined, or trace who calls it, and Vera returns ranked functions, classes, and call sites with file paths and line ranges.

Each project's index lives in `.vera/` with no background daemon or database server. Parsing, storage, BM25 keyword search, and code-aware ranking always run on your machine; embeddings run either locally on any CPU (`potion-code-16M-v2`, offline after one download) or through an OpenAI-compatible API such as Qwen3 embeddings plus reranking.

This package installs the native `vera` binary and runs it.

## Install and setup

### 1. One-time machine setup

```bash
uvx vera-ai install
# or
pip install vera-ai && vera-ai install
```

Requires Python 3.9 or later.

`install` downloads the native Vera binary matching this package's version, adds a `vera` command to your user bin directory (for example `~/.local/bin`), and opens the agent skill picker if you are in a terminal. Without a terminal (as in CI), it installs the binary and skips the skill picker. `vera-ai <command>` (or `uvx vera-ai <command>`) also forwards any command to the binary.

Next, pick your model backend once for your machine (and install the skill globally if you skipped it):

```bash
vera setup
vera agent install
```

### 2. In each repository

```bash
vera index .
vera search "where expired sessions are cleaned up"
```

Inside a Git repository, Vera automatically adds `.vera/` and its `.vera.*` working files to `.git/info/exclude`, so you do not need to edit `.gitignore`. After editing code, `vera update .` or `vera watch .` refreshes only changed files.

## Designed for context efficiency

- **Hybrid retrieval aimed at definitions.** On an embedded index, each query runs BM25 keyword search and embedding similarity together, fuses the rankings, optionally reranks top candidates, and prioritizes symbol definitions and source files over passing mentions, tests, docs, and generated code (a `--full-text-only` index uses BM25 alone).
- **Whole symbols and compact output.** Tree-sitter extracts functions, classes, methods, and structs across most of Vera's 65 supported languages. Results come back as plain Markdown code blocks labeled with `path:start-end kind:name` (`--compact` returns signatures without bodies; `--json` outputs JSON).
- **Direct structural lookup.** The same index answers definitions (`vera structural definitions`), callers and callees (`vera references`), HTTP routes (`vera structural routes`), environment variable reads (`vera structural env`), SQL sites, explicit implementations (`vera structural impls`), and dead-code candidates (`vera dead-code`).
- **Fast on CPU, including dependency trees.** On an AMD Ryzen 7 9800X3D CPU, Vera indexes its own repository (447 files, ~8,100 chunks) in 0.9 seconds with Potion Code and answers a CLI search in ~0.13 seconds including process startup. Index dependency sources without embeddings via `vera index ~/.cargo/registry/src --full-text-only`, then search them alongside your repo with `vera search "retry backoff" --also ~/.cargo/registry/src`.
- **Agent integration.** For shell-capable agents (Claude Code, OpenCode, Codex, Factory Droid, Cursor, Gemini CLI, Copilot, and other Agent Skills clients), `vera agent install` installs an on-demand skill that teaches when to use `vera search`, `vera structural`, `vera references`, `rg`, or direct file reads. An MCP server (`vera mcp`) is included for MCP-centric clients; see the [MCP guide](https://github.com/VeraTools/Vera/blob/master/docs/mcp.md).
- **Measured retrieval and agent efficiency.** In a 4-arm Flask experiment (10 cross-file questions, 40 sessions, all 10/10 blind-judged), Vera with Qwen3 API embeddings + reranking cut total prompt tokens by 48% vs grep/file reads (statistically significant at `p = 0.037`), while local Vera observed `-27%` vs Semble CLI's `-12%` (within noise at `N = 10`). On Semble v0.5.5's 1,251-task benchmark with `potion-code-16M-v2`, Vera 2.0.1 scored `0.8436` nDCG@10 vs Semble 0.5.5's `0.8514` (`0.9199` vs `0.9177` Recall@5) in `4.7 GB` vs `32 GB` of disk space. Details: [Benchmarks](https://github.com/VeraTools/Vera/blob/master/docs/benchmarks.md).

## Common commands

| Task | Command |
|---|---|
| Guided backend and skill setup | `vera setup` |
| Orient in an unfamiliar repository | `vera overview` |
| Search by behavior | `vera search "retry failed uploads with backoff"` |
| Scan signatures only | `vera search "request validation" --compact --limit 8` |
| Narrow results | `vera search "error handling" --lang rust --path "src/**" --type function` |
| Scope to uncommitted or branch changes | `vera search "token validation" --changed` (or `--base main`) |
| Search another indexed tree alongside yours | `vera search "retry backoff" --also ~/.cargo/registry/src` |
| Find a definition | `vera structural definitions parse_config` |
| Find callers or callees | `vera references parse_config` / `vera references parse_config --callees` |
| Regex search over indexed files | `vera grep "TODO\|FIXME"` |
| Refresh changed files | `vera update .` or `vera watch .` |
| Diagnose why a file was skipped | `vera explain-path path/to/file` |
| Install or update agent skills | `vera agent install` |
| Check your setup | `vera doctor` |
| Check for a newer release | `vera upgrade` |

## Models and privacy

Two starting points fit most workflows:

- **Local CPU (`vera setup --potion-code`, default):** runs MinishLab's `potion-code-16M-v2` on any CPU with no GPU, ONNX Runtime, or API key, and works offline after a one-time ~32 MB download.
- **API mode (`vera setup --api`):** pairs `qwen/qwen3-embedding-8b` and `qwen/qwen3-reranker-8b` via the OpenRouter preset (one paid key) or any OpenAI-compatible embedding endpoint plus optional generic/Voyage reranking (including separate hosts or a local `llama.cpp` server) for maximum retrieval quality.

Local GPU ONNX backends (`vera setup --onnx-jina-cuda` and others) and keyword-only indexes (`--full-text-only`) are also supported; see [Models and backends](https://github.com/VeraTools/Vera/blob/master/docs/models.md).

With a local backend and no external reranker or completion endpoint configured, your code and queries stay on your machine. Data goes elsewhere only to endpoints you configure: API mode sends chunk text and queries to your embedding host, an API reranker receives queries and top candidate chunks, and `--deep` with a completion endpoint sends the query plus local symbol names and file paths. Vera checks GitHub for a new release once a day; set `VERA_NO_UPDATE_CHECK=1` to disable it.

## How this package installs Vera

- Downloads the release archive matching this package's exact version (`VERA_MANIFEST_URL` overrides the manifest URL) and verifies archive size and SHA-256 before extracting.
- Caches the verified binary in Vera's data directory (`~/.local/share/vera` on Linux) for offline reuse.
- Covers Linux (`x86_64`, `aarch64`, plus static `musl` on `x86_64` systems without glibc), macOS (Intel and Apple Silicon), and Windows (`x86_64`). Set `VERA_TARGET` to override target detection, for example `VERA_TARGET=x86_64-unknown-linux-musl uvx vera-ai install`.
- `vera uninstall` removes the binary, models, configuration, agent skills, and the `vera` shim while leaving per-project `.vera/` indexes untouched.

## Documentation

- [Installation guide](https://github.com/VeraTools/Vera/blob/master/docs/installation.md): backends, agent skills, prebuilt binaries, Docker
- [Models and backends](https://github.com/VeraTools/Vera/blob/master/docs/models.md): local CPU, Qwen API, GPU ONNX, and custom models
- [Query guide](https://github.com/VeraTools/Vera/blob/master/docs/query-guide.md): choosing commands and phrasing queries
- [Features](https://github.com/VeraTools/Vera/blob/master/docs/features.md): every command and flag
- [Benchmarks](https://github.com/VeraTools/Vera/blob/master/docs/benchmarks.md): agent context-efficiency study, retrieval snapshots, and methodology
- [GitHub repository](https://github.com/VeraTools/Vera): source, issues, and release notes

Vera is MIT licensed.
