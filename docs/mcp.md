# MCP

`vera mcp` runs a Model Context Protocol server over standard input and output, using JSON-RPC.

For most coding agents, the CLI plus the skill installed by `vera agent install` works better than MCP (see [Installation](installation.md)). Use MCP when your client is built around MCP servers or your team shares one server config.

## Tools

| Tool | What it does |
|---|---|
| `load_index` | Validate and register an index directory for the repository root recorded in its metadata. |
| `list_indexes` | List registered index names (repository folder names), directories, scopes, and metadata. |
| `search_code` | Hybrid keyword and semantic search, with filters, intent, multiple queries, git scopes, and index selection. |
| `get_stats` | File count, chunk count, index size, language breakdown, and index health. |
| `get_overview` | Languages, directories, entry points, symbol types, complexity hotspots, and project conventions. |
| `regex_search` | Regular-expression search over indexed files, with surrounding context. |
| `structural_search` | Definitions, environment-variable reads, routes, SQL sites, and explicit implementations. |
| `find_references` | Callers or callees from Vera's stored reference data. |
| `explain_path` | Why a path is or is not indexed: ignore rules, binary detection, size, or default exclusions. |

Without explicitly loaded indexes, the first call to `search_code`, `structural_search`, or `find_references` builds an index for the server's working directory and starts a file watcher. To use an existing index elsewhere, call `load_index` with its index directory. For search, regex, structural, and reference calls, an absolute `scope_path` selects the registered repository scope containing that path. `search_code` also accepts `index_name`, which selects by repository folder name as shown by `list_indexes`; the two selectors are mutually exclusive. Nested repository scopes select the most-specific match. Duplicate names are ambiguous and return an error, in which case use `scope_path`. With multiple indexes registered, choose a scope for stats and overview with the project `path`. Search, references, and overview also accept git scopes that limit results to changed files.

A search result can include a second text block with warnings: the index is stale, semantic search was unavailable so results are keyword-only, or the reranker failed so results are not reranked. Each warning names its cause; see [Troubleshooting](troubleshooting.md#keyword-only-results).

## Client setup

The server uses the backend saved by `vera setup`, unless `VERA_BACKEND` is set (the Docker images set it). Configure a backend first.

By default, the server works on the project in its working directory. Most clients start MCP servers in the open project's root; if yours starts them elsewhere, set the client's working-directory option (often `cwd`) to the project root, or the first search will index the wrong directory. Alternatively, register an existing index with `load_index`.

### Claude Code

```bash
claude mcp add vera -- vera mcp
```

### Cursor and Windsurf

Add this entry to the MCP configuration:

```json
{"mcpServers":{"vera":{"command":"vera","args":["mcp"]}}}
```

### Codex

```toml
[mcp_servers.vera]
command = "vera"
args = ["mcp"]
```

### VS Code (Copilot)

Add the server to `.vscode/mcp.json`. VS Code uses a `servers` key instead of `mcpServers`:

```json
{"servers":{"vera":{"type":"stdio","command":"vera","args":["mcp"]}}}
```

### Docker

The Docker images start `vera mcp` by default. Mount your project at `/workspace`; see the [Docker guide](docker.md) for image variants and client config.

## If the server does not appear

- Make sure `vera` is on the client's `PATH`. If the client runs with a restricted environment, put the absolute path to the binary in `command`.
- If you use API mode through environment variables, make sure the client passes them to the server process.
- Run `vera mcp` in a terminal to confirm it starts, then check the client's MCP logs.
