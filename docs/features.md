# Features

A tour of what Vera can do, grouped by task. For writing good queries, see the [query guide](query-guide.md). For every setting, see [configuration](configuration.md).

## Search

### Search by behavior or by name

```bash
vera search "retry failed uploads with exponential backoff"
vera search "parse_config"
```

Each query runs two searches at once: keyword search (BM25), which is good at exact identifiers, paths, and config keys, and embedding search, which finds code that matches the meaning of the query even when the words differ. Vera merges the two result lists, then applies code-aware ranking. [How Vera works](how-it-works.md) explains each step.

Ranking prefers source files over docs and tests, leaves generated code out unless you pass `--include-generated`, and prefers the definition of a symbol over places that only mention it. Results are returned five at a time by default; change that with `--limit` or `retrieval.default_limit`.

### Filters

| Filter | Example | Effect |
|---|---|---|
| `--lang` | `--lang rust` | One language |
| `--path` | `--path "src/**/*.ts"` | Files matching a glob, relative to the repository root. Repeat it to match any of several patterns. |
| `--type` | `--type function` | One symbol type (`function` and `method` are interchangeable) |
| `--scope` | `--scope docs` | Only `source` (code, tests, and config), `docs`, `runtime` (extracted or bundled app code), or `all`. Without it, every indexed file except generated ones can match, and source ranks first. Generated files need `--include-generated` with any scope. |
| `--include-generated` | | Include minified and other generated files, which results leave out by default |
| `--limit` | `--limit 10` | Number of results |

Filters combine with AND. Multiple `--path` patterns combine with OR.

### Several phrasings at once

```bash
vera search "OAuth token refresh" "JWT expiry handling" "auth middleware"
```

Vera runs each query and merges the results into one ranked list. This is cheaper than three separate calls and catches code that one phrasing would miss.

### Intent

```bash
vera search "config" --intent "find where database connection strings are loaded"
```

`--intent` adds your goal to a short or ambiguous query so ranking can aim at what you need.

### Deep search

```bash
vera search "how a request becomes a response" --deep
```

`--deep` is for questions whose answer is spread across several places. With a chat completion endpoint configured (`VERA_COMPLETION_BASE_URL` and `VERA_COMPLETION_MODEL_ID`, any OpenAI-compatible server including a local one), Vera first collects real symbol and file names from the index, asks the model to break the question into targeted sub-queries, runs them in parallel, and merges the results with the original query weighted highest. Without a completion endpoint, `--deep` follows the symbols named in the top results and searches for those instead.

### Compact results

```bash
vera search "request validation" --compact
```

`--compact` returns signatures (name, parameters, return type) without bodies, so more results fit in fewer tokens. It is useful for a broad first look before reading specific functions. It also works with `grep`, `references`, and `structural`.

### Search what changed

```bash
vera search "token validation" --changed       # modified, staged, and untracked files
vera search "token validation" --since HEAD~5  # files changed since a revision
vera search "token validation" --base main     # files changed on this branch
```

The same flags work with `grep`, `structural`, `references`, and `overview`. In a monorepo, the scope is limited to files inside the indexed directory.

### Optional reranking

A reranker (cross-encoder) rereads the query together with each top candidate and reorders them. It is off by default, and `vera setup --api` turns it on when you configure an API reranker. In our screening, no tested local reranker improved on Vera's built-in ranking, while the Qwen API reranker paired with Qwen embeddings scored highest overall. Turn reranking on with `vera config set retrieval.reranking_enabled true`; see [models](models.md#reranking) for options.

## Navigate code

### Structural queries

```bash
vera structural definitions parse_config   # where a symbol is defined
vera structural env DATABASE_URL           # where an environment variable is read (omit the name for all)
vera structural routes                     # HTTP route registrations
vera structural sql                        # SQL execution sites
vera structural impls Loader               # types that implement, extend, or conform to Loader
```

These answer common questions directly from the index, without writing tree-sitter queries. `impls` reports explicit declarations (`implements`, `extends`, `impl Trait for Type`, and similar); it does not infer implicit interface satisfaction. `routes` and `sql` take no query term; narrow them with `--path`, `--lang`, or `--scope`.

### Callers and callees

```bash
vera references parse_config             # who calls it
vera references parse_config --callees   # what it calls
vera references get --receiver app       # only calls written as app.get(...)
```

Call sites are extracted from the syntax tree at index time and matched by name, not resolved by a type checker. When several definitions share a name, the output lists the receivers the calls went through, and `--receiver` keeps just one.

### Dead code candidates

```bash
vera dead-code
```

Lists functions and methods with no indexed callers, skipping common entry points such as `main`, `new`, and `default`. Treat the list as candidates: dynamic dispatch, reflection, generated code, and callers outside the index are invisible to name matching, so confirm before deleting.

### Project overview

```bash
vera overview
vera overview --base main
```

Summarizes the project: languages, directory layout, entry points, symbol counts, complexity hotspots, and detected frameworks and conventions. It is a quick orientation for a person or an agent new to the codebase.

### Regex search

```bash
vera grep "TODO|FIXME" --path "src/**"
vera grep "queryClient" -i --context 3
```

Regex search over indexed files, with the same filters and git scopes as `vera search`. It uses Rust regex syntax: `|` is alternation and `\|` matches a literal pipe. Use `rg` instead for files outside the index, counting matches, or find-and-replace work.

## Indexing

### Symbol-aware chunks

Vera parses files with tree-sitter and indexes functions, classes, methods, structs, traits, interfaces, and `impl` blocks as separate chunks, each labeled with its path, line range, and symbol name. Symbols longer than 200 lines are split at natural boundaries and keep their name with a part number. Config files are indexed whole, Markdown and reStructuredText by section, and files in unrecognized formats by line windows. See [supported languages](supported-languages.md).

Any chunk larger than `indexing.max_chunk_bytes` (24 KB by default) is split at line boundaries. That budget is separate from the embedding model's input window: local models truncate longer input to their maximum length, and an API provider with a smaller token limit can reject a chunk that fits the budget. If that happens, lower `indexing.max_chunk_bytes` and run `vera index .` again.

### Updates and watch mode

```bash
vera update .                    # re-index only changed files
vera update . --max-files 250    # cap the work in one run
vera watch .                     # update automatically as files change
```

`vera update` compares content hashes and re-parses and re-embeds only the files that changed. `vera watch` does the same in the background while you work, two seconds after the last change. `update`, `search`, `grep`, and the other query commands find the nearest `.vera/` index above the current directory, so they work from any subdirectory.

Search warns when the index is behind the working tree and tells you how many files changed.

### Choosing what gets indexed

Vera skips anything in `.gitignore`, plus common build and dependency directories (`node_modules`, `target`, `dist`, `.venv`, and others). For more control, add a `.veraignore` file in gitignore syntax; put `#include .gitignore` on its first line to keep the gitignore rules and add your own on top. One-off options: `--exclude <glob>`, `--no-ignore`, and `--no-default-excludes`.

If a file you expect is missing, ask Vera why:

```bash
vera explain-path src/generated/schema.ts
```

It reports the deciding rule: a default exclude, an ignore file, the size limit, binary detection, and so on.

### Index health

`vera stats` shows file, chunk, and language counts, the index size, and parse health: files where tree-sitter found syntax errors, files that fell back to line chunking, and files that failed to parse. Problems show up here instead of quietly lowering search quality. Add `--json` for scripts.

### Indexing with an API backend

API indexing sends requests in parallel, retries timeouts and rate limits with backoff, and honors `Retry-After`. If a run fails partway, the embeddings already received are saved in `.vera.resume/`, and the next `vera index` reuses them instead of paying for them again. The existing index stays usable until a new build finishes.

Indexing shows a progress display in a terminal. In CI or agent runs, it prints a short progress line to stderr every 10 seconds on slow runs, and `--json` prints a summary with file, chunk, request, retry, and timing counts.

## Output

| Flag | Output |
|---|---|
| (default) | Markdown code blocks labeled with path, line range, and symbol |
| `--compact` | Signatures only |
| `--json` | Compact JSON |
| `--raw` | Verbose, human-readable output with every field |
| `--timing` | Timings on stderr: per stage for `search`, total time for `grep` and `structural` |

`--json`, `--raw`, and `--timing` can go before or after the subcommand. `vera search --json --rerank-status` also reports whether results were reranked and, if not, why.

Search output has no length limit by default. Set `retrieval.max_output_chars` (or `VERA_MAX_OUTPUT_CHARS`) to cap the total characters; lower-ranked results are trimmed first, at a line boundary.

## Agent integration

- **Skill files.** `vera agent install` installs a Vera skill for more than 30 agent clients, globally or per project. It tells the agent which Vera command fits which question and when to use `grep` instead. See [installation](installation.md#set-up-agent-skills).
- **Project instructions.** The installer can add a short Vera section to `AGENTS.md`, `CLAUDE.md`, and similar files, kept current between `<!-- vera:begin -->` and `<!-- vera:end -->` markers.
- **Keeping skills current.** `vera agent sync` refreshes outdated skill copies and project sections. Upgrades do this on their own.
- **MCP server.** `vera mcp` exposes search, regex, structural, references, overview, explain-path, and stats tools to MCP clients, indexing on first use and keeping the index current. See [MCP](mcp.md).

## Models and backends

- **Default.** Potion Code runs on CPU and needs only a one-time 32 MB download.
- **Local GPU.** Jina ONNX backends for CUDA, ROCm, CoreML, DirectML, and OpenVINO. Batch sizes adapt to sequence length and available memory, and `--low-vram` forces conservative settings on small GPUs.
- **API.** Any OpenAI-compatible embedding endpoint, with optional reranking through generic or Voyage-style rerank APIs.
- **Switching.** `vera backend` changes the backend without rerunning the full setup.

Details and measurements: [models and backends](models.md).

### HTTP inference server

```bash
vera serve --port 3000
```

`vera serve` runs Vera's local models behind an HTTP API so other tools, or other machines, can use them: `POST /v1/embeddings` (OpenAI format), `POST /v1/rerank` (Cohere and Jina format), and `GET /v1/health`. It listens on `127.0.0.1` by default. Use `--host` to bind elsewhere and `--api-key` (or `VERA_SERVE_KEY`) to require a bearer token. Models unload after 300 seconds without requests; change that with `--idle-timeout`, or pass a negative value to keep them loaded.

## Maintenance

| Command | What it does |
|---|---|
| `vera doctor` | Checks configuration, model files, the index, and whether a newer release exists. `--probe` also loads the backend to test it. Exits 1 on failure. |
| `vera repair` | Re-downloads missing or damaged model files and runtimes. |
| `vera upgrade` | Shows the update plan; `--apply` runs it. |
| `vera config` | Shows settings; `vera config get` and `vera config set` read and change them. |
| `vera uninstall` | Removes Vera's binary, models, config, and agent skills. Project indexes stay. |

## Platforms

Native binaries for Linux (x86_64, aarch64), macOS (x86_64, Apple Silicon), and Windows (x86_64), plus a static musl build for Linux systems without glibc. Install through npm, Bun, uv, pip, a prebuilt archive, Docker, or from source. See [installation](installation.md).
