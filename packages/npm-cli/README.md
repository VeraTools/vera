# @vera-ai/cli

Vera is local code search for AI coding agents and the developers who use them. Describe what the code does, such as "where expired sessions are cleaned up", and Vera returns the functions and classes that do it, ranked, with file paths and line ranges. Exact names work too. The index lives in your project, search runs on your machine, and the default model runs on any CPU with no GPU, API key, or server to manage.

This package installs the native Vera binary and runs it.

## Install

```bash
npx -y @vera-ai/cli install
# or
bunx @vera-ai/cli install
```

Requires Node.js 18 or later (or Bun).

`install` downloads the Vera binary that matches this package's version, adds a `vera` command to your user bin directory (for example `~/.local/bin`), and then opens the agent skill installer if you are in a terminal. Without a terminal, as in CI, it installs the binary and skips the skill installer. If the bin directory is not on your PATH, the installer tells you which directory to add. `npx -y @vera-ai/cli <command>` also forwards any Vera command to the binary.

## Quick start

```bash
# Download the default local model and index the current project
vera setup --potion-code --index .

# Search
vera search "where expired sessions are cleaned up"
```

Run `vera setup` with no flags in a terminal for a guided setup that picks a backend, can install the agent skill, and can index the project.

## What you get

- Search by behavior and by name. Every query runs keyword search (BM25) and embedding search together, merges the results, and ranks definitions above passing mentions and source files above tests and docs.
- Whole functions, not line fragments. Vera indexes functions, classes, methods, and other symbols as separate results, with structural (tree-sitter) parsing for most of its 65 supported languages.
- Fast on an ordinary CPU. On a Ryzen 7 9800X3D desktop, Vera indexes its own repository (438 files, about 7,900 chunks) in under a second, and a command-line search returns in about 0.13 seconds including process startup. `vera update .` and `vera watch .` re-index only changed files.
- Measured search quality. On Semble's 1,251-task code search benchmark, with the same embedding model for both tools, Vera scores within 1% of Semble on nDCG@10 and slightly ahead on Recall@5, with an index about a seventh the size. See the [benchmarks](https://github.com/VeraTools/Vera/blob/master/docs/benchmarks.md).
- Structural questions from the same index: where a symbol is defined, who calls it, which routes or environment variables exist, which types implement an interface, and which functions have no callers. Any search can be limited to files changed in your working tree or branch.
- Agent integration. `vera agent install` adds a Vera skill to more than 30 agent clients, including Claude Code, Codex, Cursor, OpenCode, Gemini CLI, and Copilot. An MCP server (`vera mcp`) is included for clients that prefer it; see the [MCP guide](https://github.com/VeraTools/Vera/blob/master/docs/mcp.md).

Results come back as Markdown code blocks labeled with path, line range, and symbol. Add `--json` for machine-readable output.

## Common commands

| Task | Command |
|---|---|
| Guided setup | `vera setup` |
| Search by behavior | `vera search "retry failed uploads with backoff"` |
| Narrow the results | `vera search "error handling" --lang rust --path "src/**" --type function` |
| Search only files you changed | `vera search "token validation" --changed` (or `--base main`) |
| Find a definition | `vera structural definitions parse_config` |
| Find callers, or what a function calls | `vera references parse_config` / `vera references parse_config --callees` |
| Exact text or regex in indexed files | `vera grep "TODO\|FIXME"` |
| Keep the index current | `vera update .` or `vera watch .` |
| Find out why a file is not indexed | `vera explain-path path/to/file` |
| Install the agent skill | `vera agent install` |
| Check your setup | `vera doctor` |
| Check for a newer release | `vera upgrade` |

## Models and privacy

The default backend, Potion Code (`minishlab/potion-code-16M-v2`), runs on the CPU and works offline after a one-time download. Reranking is off by default for local backends. GPU backends (`vera setup --onnx-jina-cuda` and others) need extra runtime libraries; see the [installation guide](https://github.com/VeraTools/Vera/blob/master/docs/installation.md).

`vera setup --api` configures hosted models instead. Qwen3 embeddings plus reranking through OpenRouter (paid) scored highest in our model screening, and any OpenAI-compatible endpoint works. See [API mode](https://github.com/VeraTools/Vera/blob/master/docs/models.md#api-mode).

With a local backend, your code and queries stay on your machine. In API mode, or with an API reranker or `--deep` completion endpoint configured, Vera sends chunk text and queries to the endpoints you set. Vera checks GitHub for a new release once a day; set `VERA_NO_UPDATE_CHECK=1` to turn that off.

## How this package installs Vera

- By default it downloads the release of Vera that exactly matches the package version (`VERA_MANIFEST_URL` points it at a different release manifest), then checks the archive's size and SHA-256 against the release manifest before extracting the binary.
- The verified binary is cached in Vera's data directory (`~/.local/share/vera` on Linux), and later runs use it without network access. A cache written by an older wrapper is downloaded and verified once more before it is reused offline.
- Prebuilt binaries cover Linux x86_64 and arm64, macOS on Intel and Apple Silicon, and Windows x86_64. On x86_64 Linux without glibc (such as Alpine), it picks the musl build.
- Set `VERA_TARGET` to override platform detection, for example `VERA_TARGET=x86_64-unknown-linux-musl npx -y @vera-ai/cli install`.
- `vera uninstall` removes the binary, models, configuration, agent skills, and the `vera` command.

## Documentation

- [Installation guide](https://github.com/VeraTools/Vera/blob/master/docs/installation.md): GPU backends, API mode, prebuilt binaries, Docker
- [Query guide](https://github.com/VeraTools/Vera/blob/master/docs/query-guide.md): how to write queries that find what you want
- [Features](https://github.com/VeraTools/Vera/blob/master/docs/features.md): every command and flag
- [Benchmarks](https://github.com/VeraTools/Vera/blob/master/docs/benchmarks.md): methodology and results
- [GitHub repository](https://github.com/VeraTools/Vera): source, issues, and release notes

Vera is MIT licensed.
