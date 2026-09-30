<div align="center">

<img width="1584" height="539" alt="vera" src="https://github.com/user-attachments/assets/c866fc70-b1e6-400b-aaf7-fa68721a4955" />

# Vera

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/VeraTools/Vera/blob/master/LICENSE)
[![CI](https://github.com/VeraTools/Vera/actions/workflows/ci.yml/badge.svg)](https://github.com/VeraTools/Vera/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@vera-ai/cli)](https://www.npmjs.com/package/@vera-ai/cli)
[![PyPI](https://img.shields.io/pypi/v/vera-ai)](https://pypi.org/project/vera-ai/)
[![GitHub release](https://img.shields.io/github/v/release/VeraTools/Vera?include_prereleases&sort=semver)](https://github.com/VeraTools/Vera/releases)
[![Languages](https://img.shields.io/badge/languages-65%2B-green.svg)](docs/supported-languages.md)

[Docs](docs/README.md)
·
[Install Guide](docs/installation.md)
·
[Features](docs/features.md)
·
[Query Guide](docs/query-guide.md)
·
[Benchmarks](docs/benchmarks.md)
·
[How It Works](docs/how-it-works.md)
·
[Models](docs/models.md)
·
[Supported Languages](docs/supported-languages.md)

**Local, symbol-aware code search for developers and AI agents.**

Describe the behavior you need, then get ranked code chunks with file paths, line ranges, and symbols. Vera combines keyword and vector search across 65 languages, with optional reranking. The index stays on your machine.

<sub>**V**ector **E**nhanced **R**eranking **A**gent</sub>

</div>

![vera search demo](docs/assets/vera-demo.gif)

## Quick Start

**1. Install**

```bash
bunx @vera-ai/cli install   # or: npx -y @vera-ai/cli install / uvx vera-ai install
```

**2. Set up and index**

Use the default local CPU model. Setup downloads its assets, then indexes the current project:

```bash
vera setup --potion-code --index .
```

Or run `vera setup` for the full wizard: choose a backend, optionally install agent skills, and optionally index. Local CPU is first and selected by default. Indexing defaults to Yes for local backends and No for API mode.

For API mode, run `vera setup --api --index .`. Qwen/OpenRouter is the recommended API preset and uses one shared key for embeddings and reranking (paid usage). See [Models](docs/models.md) for the measured tradeoffs and other API or ONNX choices.

**3. Search**

```bash
vera search "authentication logic"
```

Add `.vera/` to `.gitignore`; the project index can be large. An interactive search offers to create a missing index. JSON and non-interactive searches return an error so scripts can handle it.

## What Sets Vera Apart

| Capability | What it gives you |
|---|---|
| **Search by behavior or identifier** | Hybrid keyword and vector retrieval finds both exact names and conceptual matches. |
| **Relevant code chunks** | Symbol-bounded chunks with paths and line ranges let agents read relevant code without loading every matching file. |
| **Local CPU default** | Potion Code runs on CPU. After the model download, local indexing and search work offline. |
| **Incremental indexes** | `vera update .` and watch mode reuse the index and re-embed changed files. |
| **Code navigation** | Structural queries, heuristic references and dead-code candidates, and project overviews use the same index. |

Vera started as a fork of Pampax and was rebuilt around measured retrieval choices. The [ADRs](docs/adr/000-decision-summary.md) record those decisions; the [feature guide](docs/features.md) covers the command surface.

## Choosing a Backend

Start with local Potion Code on CPU. API mode is the second option when you want a remote embedding or reranking service; Qwen/OpenRouter is recommended within the API presets. Jina ONNX backends support optional local GPU inference.

Indexes live in `.vera/` per project. Configuration and downloaded assets use the Vera data directory. See [Installation](docs/installation.md) for supported platforms and setup, and [Models](docs/models.md) for backend choices and dependencies.

## Privacy

Local inference keeps code and query text on your machine. Initial model downloads need network access. API mode sends chunk text and queries to the configured model endpoints. The update check contacts GitHub once a day; disable it with `VERA_NO_UPDATE_CHECK=1`.

Vera complements ripgrep: use `rg` when you know the exact string, and Vera when you know what the code does but not what it is called.

## Use with AI Agents

The preferred agent integration is the CLI plus the Vera skill: `vera agent install` installs it for supported coding agents and can add a short usage snippet to your project's `AGENTS.md`, `CLAUDE.md`, `COPILOT.md`, or editor rules file.

```bash
vera agent install
vera agent install --client all
```

If you use the [skills CLI](https://github.com/vercel-labs/skills), you can install Vera there too:

```bash
npx skills add VeraTools/Vera
```

<details>
<summary><strong>Optional: MCP server</strong> (for MCP-first clients or teams standardizing on MCP)</summary>

Vera also ships an MCP server: `vera mcp`. Setup for each client and the full tool list: [MCP integration](docs/mcp.md).

```bash
claude mcp add vera -- vera mcp      # Claude Code
```

Cursor, Windsurf, and generic MCP clients:

```json
{"mcpServers":{"vera":{"command":"vera","args":["mcp"]}}}
```

Vera exposes `search_code`, `get_stats`, `get_overview`, `regex_search`, `structural_search`, `find_references`, and `explain_path`. Client-specific setup and tool details: [MCP integration](docs/mcp.md).

</details>

## Usage

### Search Patterns

```bash
vera search "error handling" --lang rust
vera search "routes" --path "src/**/*.ts" --path "tests/**/*.ts"
vera search "OAuth token refresh" "JWT expiry handling" "auth middleware"
vera search "config" --intent "find where database connection strings are loaded"
vera search "token validation" --changed
vera structural definitions parse_config
vera references parse_config
vera update .
```

Repeat `--path` to match any of several patterns. See the [query guide](docs/query-guide.md) for filters, deep search, and structural queries; `vera --help` lists commands.

### Output

Defaults to Markdown codeblocks:

````
```src/auth/login.rs:42-68 function:authenticate
pub fn authenticate(credentials: &Credentials) -> Result<Token> { ... }
```
````

Use `--json` for compact JSON. `--raw` works with `vera search`, `vera grep`, and `vera references`; `--timing` works with `vera search` and `vera grep`. You can place them before or after the subcommand (for example, `vera --timing search "auth"` or `vera references parse_config --raw`).

### Excluding Files

Vera respects `.gitignore` by default. Create a `.veraignore` file (gitignore syntax) for more control, or use `--exclude` flags. Details: [docs/features.md](docs/features.md#flexible-exclusions).

If a file is missing from the index and you need the exact reason, run:

```bash
vera explain-path path/to/file
```

## Benchmarks

The [benchmark report](docs/benchmarks.md#current-results) compares Vera and Semble on the full 1,251-task suite, with dated hardware, model, and scoring details. The [benchmark history](docs/benchmarks-history.md#agent-level-benchmark) includes the agent-context experiment and its small-sample limits. Token savings depend on the query, repository, and agent workflow.

## Status and Community

V2 removes rejected experiment controls while preserving ordinary default indexes and search-result fields. See [migration guidance](docs/migration-v2.md) and [What's New](docs/whats-new.md). Bug reports and feature requests go to [Issues](https://github.com/VeraTools/Vera/issues).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).
