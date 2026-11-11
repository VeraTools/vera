<div align="center">

<img width="1584" height="539" alt="vera" src="https://github.com/user-attachments/assets/c866fc70-b1e6-400b-aaf7-fa68721a4955" />

# Vera

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/VeraTools/Vera/blob/master/LICENSE)
[![CI](https://github.com/VeraTools/Vera/actions/workflows/ci.yml/badge.svg)](https://github.com/VeraTools/Vera/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@vera-ai/cli)](https://www.npmjs.com/package/@vera-ai/cli)
[![PyPI](https://img.shields.io/pypi/v/vera-ai)](https://pypi.org/project/vera-ai/)
[![GitHub release](https://img.shields.io/github/v/release/VeraTools/Vera?include_prereleases&sort=semver)](https://github.com/VeraTools/Vera/releases)
[![Languages](https://img.shields.io/badge/languages-65-green.svg)](docs/supported-languages.md)

**Code retrieval and structural navigation for AI coding agents, built to spend less context finding the right code.**

[Install](docs/installation.md) · [Features](docs/features.md) · [Models](docs/models.md) · [Query guide](docs/query-guide.md) · [Benchmarks](docs/benchmarks.md) · [How it works](docs/how-it-works.md) · [All docs](docs/README.md)

<sub>**V**ector **E**nhanced **R**eranking **A**gent</sub>

</div>

Coding agents burn context fast in unfamiliar repositories: running broad text searches, opening half-related files, and chasing call chains one file at a time. Vera gives an agent (or a developer in a terminal) direct paths to the relevant symbols. Ask what a piece of code does, where a type is defined, or who calls a function, and Vera returns ranked functions, classes, and call sites with file paths and line ranges.

Vera ships as a single native binary and keeps each project's index in `.vera/` with no background daemon or database server. Indexing, storage, keyword search, and code-aware ranking always run on your machine; embeddings run either locally on any CPU (`potion-code-16M-v2`, offline after one download) or through an OpenAI-compatible API such as Qwen3 embeddings plus reranking.

![vera search demo](docs/assets/vera-demo.gif)

## Quick start

Setup has two parts: configuring Vera once on your machine, then indexing the repositories you work in.

### 1. One-time machine setup

```bash
# Install the native binary (pick npx, bunx, or uvx)
npx -y @vera-ai/cli install        # or: bunx @vera-ai/cli install / uvx vera-ai install

# Choose your model backend (local CPU or API, plus optional skill setup)
vera setup

# Install or update the agent skill (if you skipped it during install or setup)
vera agent install
```

`install` downloads the platform binary, places a `vera` command in your user bin directory, and (in an interactive terminal) opens the agent skill picker. `vera setup` saves your backend choice once for your machine; pass `vera setup --potion-code` or `vera setup --api` if you want to skip the full wizard. Installing the skill in `global` scope makes Vera available to your agent across future repositories without repeating skill setup.

### 2. In each repository

```bash
vera index .
vera search "where expired sessions are cleaned up"
```

Run `vera index .` once from the repository root (or pass `--index .` during `vera setup`). Inside a Git repository, Vera automatically adds `.vera/` and its `.vera.*` working files to `.git/info/exclude` (including linked worktrees), so you do not need to edit `.gitignore`. If you want Vera's short routing section in a repository's `AGENTS.md`, `CLAUDE.md`, or editor rules file, run `vera agent install` in a terminal from that repository and accept the prompt.

After editing code, `vera update .` or `vera watch .` re-indexes only changed files. Linked Git worktrees without an index automatically seed one from an indexed main checkout and apply worktree changes before answering. See the [installation guide](docs/installation.md) for prebuilt binaries, GPU runtimes, Docker, and non-interactive CI flags.

## Designed for context efficiency

Most token waste during coding tasks happens before an agent writes a line of code: broad text searches, opening tangential files, and tracing call chains one file at a time. Vera shapes retrieval, chunking, output, and agent guidance around cutting down that exploratory overhead (design mechanisms below; measured agent results in [Evaluations and benchmarks](#evaluations-and-benchmarks)):

- **Hybrid retrieval tuned for implementations.** On an embedded index, each query runs BM25 keyword search and vector similarity side by side, fuses the rankings, optionally runs a cross-encoder reranker, and applies code-aware priors that favor symbol definitions over incidental mentions and source code over tests, docs, and generated files (a `--full-text-only` index runs BM25 alone). Searching `parse_config` and searching `"load settings from environment variables"` use the same path.
- **Symbol-sized AST chunks and lean output.** Tree-sitter extracts whole functions, classes, methods, structs, and traits across 54 of the [65 supported languages](docs/supported-languages.md), falling back to section, whole-file, or line-window chunks for config and docs. Output is plain Markdown code blocks labeled with `path:start-end kind:name` and no generated commentary. `--compact` returns signatures without bodies so an agent can scan candidates before reading full implementations.
- **Structural lookup instead of search-term guessing.** When an agent already knows a symbol name or wants an exhaustive list, the same index answers `vera structural definitions`, `vera references` (callers or `--callees`), `vera structural routes`, `vera structural env`, `vera structural sql`, and `vera structural impls` in one call.
- **Explicit restraint when Vera is not the right tool.** The shipped skill routes known exact strings, match counts, file names, find-and-replace tasks, and unindexed files to `rg` or direct file reads, and tells agents to skip search tools altogether on non-code questions.
- **Low latency across project and dependency trees.** On an AMD Ryzen 7 9800X3D CPU, Vera indexes its own repository (447 files, ~8,100 chunks) in 0.9 seconds with Potion Code and answers a CLI search in ~0.13 seconds including process startup. You can also build a zero-model keyword index over dependency sources (`vera index ~/.cargo/registry/src --full-text-only`) and search it alongside your repo (`vera search "retry backoff" --also ~/.cargo/registry/src`, ~0.2 seconds across 51,000 files).

## Using Vera with coding agents

### CLI + skill (recommended for shell-capable agents)

For agents with shell access, including Claude Code, OpenCode, Codex, Factory Droid, Cursor, Gemini CLI, and GitHub Copilot, the CLI paired with the Vera skill is the primary integration:

```bash
vera agent install                  # interactive: choose global/project scope and clients
vera agent install --client all     # install globally for all supported skill directories
```

Why prefer CLI + skill when the agent already has a shell:

1. **On-demand prompt footprint.** Agent skills expose a short discovery description up front and load the full command reference only when the task calls for code discovery, which can be lighter than exposing seven MCP tool schemas in clients that load MCP tool definitions into every prompt.
2. **Guided tool selection.** Beyond listing flags, the skill teaches the agent how to choose between `vera overview`, `vera search`, `vera structural`, `vera references`, `rg`, and direct file reads based on the question.

`vera agent install` writes the skill to the cross-agent `.agents/skills/` layout and client-specific skill directories, and in a terminal can maintain a `<!-- vera:begin --> ... <!-- vera:end -->` section in your project's instruction file. `npx skills add VeraTools/Vera` installs the same skill via the [skills CLI](https://github.com/vercel-labs/skills).

### MCP server

Use `vera mcp` when your client is built around MCP tools, runs without general shell access, or shares a team server configuration. It runs over stdio with seven tools (`search_code`, `get_overview`, `regex_search`, `structural_search`, `find_references`, `explain_path`, and `get_stats`), builds an index automatically on first search if needed, watches for file changes while running, and supports multi-repo queries via `repo` and `repos`.

```bash
claude mcp add vera -- vera mcp     # Claude Code
```

```json
{"mcpServers": {"vera": {"command": "vera", "args": ["mcp"]}}}
```

Setup for Codex, Cursor, Windsurf, VS Code, and Docker is in the [MCP guide](docs/mcp.md).

## Agent and CLI workflows

| Workflow | Command |
|---|---|
| Orient in an unfamiliar repository | `vera overview` |
| Trace behavior across files | `vera search "retry failed uploads with backoff"` |
| Try several phrasings in one call | `vera search "OAuth token refresh" "JWT expiry" "auth middleware"` |
| Scan signatures before reading full bodies | `vera search "request validation" --compact --limit 8` |
| Narrow by language, path, or symbol kind | `vera search "error handling" --lang rust --path "src/**" --type function` |
| Scope to uncommitted or branch changes | `vera search "token validation" --changed` (or `--base main`) |
| Search your repo plus indexed dependencies | `vera search "retry backoff" --also ~/.cargo/registry/src` |
| Jump to a symbol's definition | `vera structural definitions parse_config` |
| Inspect callers or callees before a refactor | `vera references parse_config` / `vera references parse_config --callees` |
| Enumerate routes, env reads, or implementations | `vera structural routes` / `vera structural env` / `vera structural impls Loader` |
| Regex search over indexed files | `vera grep "TODO\|FIXME" --path "src/**"` |
| Refresh changed files or watch continuously | `vera update .` / `vera watch .` |
| Diagnose why a file was skipped | `vera explain-path path/to/file` |
| Verify config, models, and index health | `vera doctor` |

Each hit is a Markdown code block labeled with file path, line range, and symbol identity. Here is a `--compact` search over Flask:

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

Omit `--compact` to get full symbol bodies, or add `--json` for structured output. See the [query guide](docs/query-guide.md) and [feature guide](docs/features.md) for every flag.

## Choosing a model backend

Parsing, SQLite + Tantivy + memory-mapped vector storage, BM25 keyword search, and code-aware ranking always run locally. Your backend choice only decides where embeddings (and optional reranking) execute. Most users should start with one of two setups:

| Recommended starting point | Setup | Strengths and tradeoffs |
|---|---|---|
| **Local CPU: Potion Code** (default) | `vera setup --potion-code` | Runs [`minishlab/potion-code-16M-v2`](https://huggingface.co/minishlab/potion-code-16M-v2) on any supported CPU. Needs no GPU, ONNX Runtime, or API key; indexes hundreds of files per second; works offline after a one-time ~32 MB download; zero ongoing cost and full local privacy. |
| **API: Qwen3 Embedding 8B + Reranker 8B** | `vera setup --api` | Pairs `qwen/qwen3-embedding-8b` with `qwen/qwen3-reranker-8b`. Highest measured retrieval quality (`0.8647` vs `0.8538` nDCG@10 on our 320-task slice, +3 points Recall@1) and the largest context reduction in our agent benchmark (`-48%` prompt tokens). OpenRouter's preset covers both with one key, or you can pair any OpenAI-compatible embedding endpoint with a generic or Voyage-style `/rerank` endpoint (for example, OpenRouter for embeddings and SiliconFlow for lower-cost reranking, or a local [llama.cpp](docs/llama-cpp-setup.md) server). Requires network access, provider billing, and sending chunk text and queries to your configured endpoints. |

Other supported modes:

- **Local GPU / custom ONNX (`vera setup --onnx-jina-cuda`, ROCm, CoreML, DirectML, OpenVINO):** runs Jina embeddings v5 nano, CodeRankEmbed, or a custom Hugging Face / local ONNX model on hardware accelerators.
- **Keyword-only (`vera index . --full-text-only`):** BM25, regex, structural queries, and references with no model at all, useful for large dependency caches or air-gapped environments.

Full measurements, reranker details, and non-interactive env configuration are in [models and backends](docs/models.md).

## Privacy

With a local backend and no external reranker or completion endpoint configured, your code and queries never leave your machine; only the initial model download uses the network. Data is sent externally only to endpoints you configure:

- **API embeddings:** chunk text during `vera index` / `vera update`, and query text during `vera search`.
- **API reranker** (even if paired with local embeddings): query text and up to `retrieval.rerank_candidates` candidate chunks (default 20, capped at 2,000 chars each).
- **`--deep` with a completion endpoint:** query text plus symbol names and file paths gathered from an initial local keyword pass.

Vera checks GitHub Releases once a day to notify you of updates; set `VERA_NO_UPDATE_CHECK=1` to disable that check.

## Evaluations and benchmarks

Vera is measured on coding-agent context use, agent tool-routing choices, and standalone retrieval quality. Full methodology, fairness controls, reproduction commands, and rejected experiments are in [benchmarks](docs/benchmarks.md) and [benchmark history](docs/benchmarks-history.md).

### Agent context efficiency and skill routing

On 10 cross-file tracing questions about Flask answered by fresh Factory Droid sessions (`GLM-5.3` at `high` effort; 40 sessions total, ~65.8M prompt tokens and ~29.8 hours of wall time), all four arms scored `10.0/10` under blind grading:

| Arm | Blind score | Tool calls | Prompt tokens (input + cache read) |
|---|---:|---:|---:|
| Control (`rg`/grep and file reads only) | 10.0/10 | 598 | 21.00M (baseline) |
| Semble CLI (`potion-code-16M-v2`) | 10.0/10 | 566 | 18.49M (-12%) |
| Vera, local Potion Code | 10.0/10 | 521 | 15.39M (-27%) |
| **Vera, Qwen3 Embedding 8B + Reranker 8B (API)** | **10.0/10** | **453** | **10.91M (-48%)** |

Only the Qwen3 API arm's `-48%` prompt-token reduction was statistically significant across the 10 paired questions (paired `t = -3.6`, Wilcoxon `p = 0.037`, bootstrap 95% CI `-61.8%` to `-12.9%`). Local Vera observed `-27%` versus Semble CLI's `-12%`, though neither local arm's confidence interval excluded zero at `N = 10`. Scope is 1 repo, 10 questions, 1 run per cell, and 1 agent model; two earlier two-arm runs on an older skill-only setup (`claude-opus-5` and `kimi-k3`, medium effort) did not reproduce the same savings direction. Separately, a small [`skill-eval`](benchmarks/skill-eval/README.md) check (8 Flask probes x 3 reps, including `git log` and `LICENSE` restraint checks) saw trigger-and-restraint skill wording raise tool-choice appropriateness from 38% (no skill) and 50% (older skill) to 75% (`21.5k` to `16.5k` mean tokens/probe).

### Retrieval quality and credit to Semble / MinishLab

Vera's primary retrieval suite and default local model come from [MinishLab](https://github.com/MinishLab): [`potion-code-16M-v2`](https://huggingface.co/minishlab/potion-code-16M-v2) and the 1,251-task, 63-repository benchmark published with [Semble v0.5.5](https://github.com/MinishLab/semble/tree/v0.5.5) (`9218491`). In a versioned historical comparison using `potion-code-16M-v2` on both tools, `Vera 2.0.1` scored `0.8436` nDCG@10 vs `Semble 0.5.5`'s `0.8514` (`0.9199` vs `0.9177` Recall@5) while storing all 63 indexes in `4.7 GB` vs `32 GB`; on a 180-task independent check across 10 external repositories, Vera scored `0.7674` vs `0.7655` (treat both as snapshots at those versions rather than live claims on newer releases). On the 320-task slice, switching Vera to `qwen/qwen3-embedding-8b` + `qwen/qwen3-reranker-8b` raised nDCG@10 from `0.8538` to `0.8647` (+3 points Recall@1). Ranking changes must clear +0.5% on the full 1,251-task suite to ship; rejected hypotheses stay recorded in [benchmark history](docs/benchmarks-history.md#rejected-ranking-and-chunking-hypotheses-2026-09-01).

## How it works

```text
query ─┬─ BM25 keyword search (Tantivy) ─┐
       └─ vector search (embeddings) ────┴─ rank fusion ─ optional reranker ─ code-aware ranking ─ results
```

Files are parsed with tree-sitter into symbol-sized chunks. Each chunk is indexed both as text (Tantivy BM25) and as an embedding vector, stored under `.vera/` inside your project (SQLite metadata, Tantivy inverted index, and a memory-mapped vector file) so a fresh CLI invocation searches in milliseconds without loading vectors into a resident daemon. [How Vera works](docs/how-it-works.md) walks through the pipeline, and the [architecture decisions](docs/adr/000-decision-summary.md) explain why each component was chosen.

## Background

Before Vera, I maintained Pampax, my fork of an earlier code search tool called PAMPA. After hitting architectural limits in its storage, chunking, and runtime dependencies, I started over from scratch in Rust. Every major piece of Vera (storage layout, AST chunking, hybrid ranking priors, default model, reranker budget, and skill instructions) was chosen by measuring alternatives and dropping ideas that did not clear the bar.

## Community and support

- Questions, bug reports, and feature requests: [GitHub Issues](https://github.com/VeraTools/Vera/issues)
- Chat and help: [Discord](https://discord.gg/rXNQXCTWDt)
- Release notes: [What's new](docs/whats-new.md) · Upgrading from 1.x: [v2 migration](docs/migration-v2.md)
- Contributing: [CONTRIBUTING.md](CONTRIBUTING.md)

Vera is MIT licensed.
