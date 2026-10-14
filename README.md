<div align="center">

<img width="1584" height="539" alt="vera" src="https://github.com/user-attachments/assets/c866fc70-b1e6-400b-aaf7-fa68721a4955" />

# Vera

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/VeraTools/Vera/blob/master/LICENSE)
[![CI](https://github.com/VeraTools/Vera/actions/workflows/ci.yml/badge.svg)](https://github.com/VeraTools/Vera/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@vera-ai/cli)](https://www.npmjs.com/package/@vera-ai/cli)
[![PyPI](https://img.shields.io/pypi/v/vera-ai)](https://pypi.org/project/vera-ai/)
[![GitHub release](https://img.shields.io/github/v/release/VeraTools/Vera?include_prereleases&sort=semver)](https://github.com/VeraTools/Vera/releases)
[![Languages](https://img.shields.io/badge/languages-65-green.svg)](docs/supported-languages.md)

**Local code search for AI coding agents and the developers who use them.**

[Install](docs/installation.md) · [Features](docs/features.md) · [Query guide](docs/query-guide.md) · [Benchmarks](docs/benchmarks.md) · [How it works](docs/how-it-works.md) · [All docs](docs/README.md)

<sub>**V**ector **E**nhanced **R**eranking **A**gent</sub>

</div>

Describe what the code does, and Vera finds it. Ask for "where expired sessions are cleaned up" and you get back the functions and classes that do it, ranked, with file paths and line ranges. Exact names work too.

Vera is a single native binary. The index lives in your project, search runs on your machine, and the default model runs on any CPU with no GPU, API key, or server to manage.

![vera search demo](docs/assets/vera-demo.gif)

## Quick start

```bash
# 1. Install (pick one: bunx, npx, or uvx)
bunx @vera-ai/cli install        # or: npx -y @vera-ai/cli install / uvx vera-ai install

# 2. Download the default local model and index the current project
vera setup --potion-code --index .

# 3. Search
vera search "where expired sessions are cleaned up"
```

Run `vera setup` with no flags in a terminal for a guided setup that can also install the agent skill. Add `.vera/` to your `.gitignore`. The [installation guide](docs/installation.md) covers GPU backends, API mode, prebuilt binaries, and Docker.

## What Vera does well

**It finds code by behavior and by name.** Every query runs keyword search (BM25) and embedding search side by side, merges the two lists, then applies code-aware ranking that prefers definitions over passing mentions and source files over tests and docs. Searching `parse_config` finds that function; searching "load settings from environment variables" finds it too.

**Results are whole functions, not line fragments.** Vera parses files with tree-sitter and indexes functions, classes, methods, structs, and other symbols as separate chunks. 54 of the [65 supported languages](docs/supported-languages.md) get symbol-level chunks; config files, docs, and the rest are split by section, whole file, or line windows. An agent reads the relevant symbol instead of opening the whole file.

**It is fast on an ordinary CPU.** On a desktop CPU (Ryzen 7 9800X3D), Vera indexes its own repository (438 files, about 7,900 chunks) in under a second, and a search from the command line returns in about 0.13 seconds, process startup included. `vera update .` and `vera watch .` re-index only the files that changed.

**Its search quality holds up against the best local tools.** On Semble's 1,251-task code search benchmark, run with the same embedding model, Vera scores within 1% of Semble on nDCG@10 and slightly ahead on Recall@5, with an index about a seventh the size. Details are [below](#benchmarks).

**It does more than search.** The same index answers structural questions: where a symbol is defined, who calls it, which routes or environment variables exist, which types implement an interface, and which functions have no callers. You can scope any search to files changed in your working tree or branch.

**It is built for agents.** `vera agent install` adds a Vera skill to more than 30 agent clients, including Claude Code, Codex, Cursor, OpenCode, Gemini CLI, and Copilot. Results come back as compact Markdown code blocks with paths and line ranges, which agents read well. An MCP server is included for clients that prefer it.

## Using it with an AI agent

The recommended setup is the CLI plus the Vera skill. The skill tells the agent when to reach for `vera search`, `vera references`, or plain `grep`, and how to write queries that work.

```bash
vera agent install                  # interactive: pick agents and global or project scope
vera agent install --client all     # every supported agent, global scope
```

The interactive installer can also add a short Vera section to your project's `AGENTS.md`, `CLAUDE.md`, or editor rules file. If you use the [skills CLI](https://github.com/vercel-labs/skills), `npx skills add VeraTools/Vera` works too.

<details>
<summary>MCP server</summary>

`vera mcp` runs an MCP server over stdio with seven tools: `search_code`, `get_overview`, `regex_search`, `structural_search`, `find_references`, `explain_path`, and `get_stats`. It indexes the project on first use and keeps the index current while it runs.

```bash
claude mcp add vera -- vera mcp                               # Claude Code
```

```json
{"mcpServers": {"vera": {"command": "vera", "args": ["mcp"]}}}
```

Setup for Codex, VS Code, and other clients is in the [MCP guide](docs/mcp.md).

</details>

## Common commands

| Task | Command |
|---|---|
| Search by behavior | `vera search "retry failed uploads with backoff"` |
| Try several phrasings at once | `vera search "OAuth token refresh" "JWT expiry" "auth middleware"` |
| Narrow the results | `vera search "error handling" --lang rust --path "src/**" --type function` |
| Search only files you changed | `vera search "token validation" --changed` (or `--base main`) |
| Find a definition | `vera structural definitions parse_config` |
| Find callers, or what a function calls | `vera references parse_config` / `vera references parse_config --callees` |
| List routes, env reads, or implementations | `vera structural routes` / `vera structural env` / `vera structural impls Loader` |
| Exact text or regex in indexed files | `vera grep "TODO\|FIXME"` |
| Get a project overview | `vera overview` |
| Keep the index current | `vera update .` or `vera watch .` |
| Find out why a file is not indexed | `vera explain-path path/to/file` |
| Check your setup | `vera doctor` |

Results come back as Markdown code blocks, one per result, labeled with the path, line range, and symbol. Here is a search over the Flask repository with `--compact`, which keeps signatures and drops bodies:

````text
$ vera search "load config from environment variables" --compact --limit 2
```src/flask/config.py:50-247 class:Config (part 1)
class Config(dict):  # type: ignore[type-arg] ...
```

```src/flask/cli.py:698-763 function:load_dotenv
def load_dotenv(
    path: str | os.PathLike[str] | None = None, load_defaults: bool = True
) -> bool: ...
```
````

Without `--compact` you get the full code of each result. Use `--json` for machine-readable output. The [query guide](docs/query-guide.md) explains how to write queries that land, and the [feature guide](docs/features.md) covers every command.

## Choosing a model backend

Indexing, storage, and search always run locally. The backend only decides where the embedding model runs.

| Backend | Setup | When to use it |
|---|---|---|
| Potion Code on CPU (default) | `vera setup --potion-code` | Almost everyone. Small, fast, works offline after a one-time download. |
| Jina ONNX on GPU | `vera setup --onnx-jina-cuda` (also ROCm, CoreML, DirectML, OpenVINO) | You have a supported GPU and want a transformer embedding model locally. |
| API | `vera setup --api` | You want hosted models. Qwen3 embeddings plus reranking through OpenRouter scored highest in our model screening. Any OpenAI-compatible endpoint works, including a local llama.cpp server. |

See [models and backends](docs/models.md) for measurements and setup details.

## Privacy

With a local backend, your code and queries stay on your machine; only the first model download needs the network. Code or queries go elsewhere only to endpoints you configure: API mode sends chunk text and queries to the embedding endpoint, an API reranker receives queries and candidate chunks (even with a local backend), and `--deep` with a completion endpoint sends the query to it. Vera checks GitHub for a new release once a day; set `VERA_NO_UPDATE_CHECK=1` to turn that off.

## Benchmarks

The main benchmark is [Semble](https://github.com/MinishLab/semble)'s task set: 1,251 code search queries across 63 open-source repositories, each with known correct answers. Semble is a popular local code search tool from MinishLab, the team behind the Potion embedding model. Both tools below used that same `potion-code-16M-v2` model and the same tasks and nDCG@10 formula; see the [benchmark page](docs/benchmarks.md#scoring) for how answers were matched.

| Tool | nDCG@10 | Recall@1 | Recall@5 | Recall@10 | Index size |
|---|---:|---:|---:|---:|---:|
| Vera 2.0.1 | 0.844 | 0.671 | **0.920** | 0.950 | **4.7 GB** |
| Semble 0.5.5 | **0.851** | **0.675** | 0.918 | **0.966** | 32 GB |

Vera also scored slightly higher than Semble on the 320-task tuning subset (0.854 vs 0.849) and on an independent set of 180 tasks from 10 repositories outside Semble's benchmark (0.767 vs 0.766). That second set exists to check that neither tool, nor the shared embedding model, was tuned to Semble's own tasks.

In a small agent experiment (10 code-tracing questions about Flask, each answered by a fresh coding agent), every setup answered every question correctly. Agents with Vera used 27% less context than agents limited to grep and file reads, and 48% less with the Qwen API backend; agents with Semble used 12% less. With only 10 questions, the 48% result is the only one that is statistically significant.

Methodology, hardware, latency numbers, and the full history are on the [benchmarks page](docs/benchmarks.md).

## How it works

```text
query ─┬─ BM25 keyword search (Tantivy) ─┐
       └─ vector search (embeddings) ────┴─ rank fusion ─ optional reranker ─ code-aware ranking ─ results
```

Files are parsed with tree-sitter into symbol-sized chunks. Each chunk is indexed twice: as text for keyword search and as an embedding for semantic search. Everything is stored in `.vera/` inside your project (SQLite, a Tantivy index, and a flat vector file), so there is no database or background service to run. [How Vera works](docs/how-it-works.md) has the details, and the [architecture decisions](docs/adr/000-decision-summary.md) record why each piece was chosen.

## Background

Before Vera, I maintained Pampax, my fork of an existing code search tool called PAMPA. I fixed and improved a lot of it, but kept running into limits in its foundation, so I started over. Vera is written from scratch in Rust, and its design choices (storage, chunking, ranking, default model) were each picked by measuring the alternatives. The experiments that did not pan out are documented alongside the ones that did.

## Community and support

- Questions, bug reports, and feature requests: [GitHub Issues](https://github.com/VeraTools/Vera/issues)
- Chat and help: [Discord](https://discord.gg/rXNQXCTWDt)
- Release notes: [What's new](docs/whats-new.md). Upgrading from 1.x: [v2 migration](docs/migration-v2.md)
- Contributing: [CONTRIBUTING.md](CONTRIBUTING.md)

Vera is MIT licensed.
