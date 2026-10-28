# MCP

`vera mcp` runs a Model Context Protocol server over standard input and output, using JSON-RPC.

For most coding agents, the CLI plus the skill installed by `vera agent install` works better than MCP (see [Installation](installation.md)). Use MCP when your client is built around MCP servers or your team shares one server config.

## Tools

| Tool | What it does |
|---|---|
| `search_code` | Hybrid keyword and semantic search, with filters, intent, multiple queries, and git scopes. |
| `get_stats` | File count, chunk count, index size, language breakdown, and index health. |
| `get_overview` | Languages, directories, entry points, symbol types, complexity hotspots, and project conventions. |
| `regex_search` | Regular-expression search over indexed files, with surrounding context. |
| `structural_search` | Definitions, environment-variable reads, routes, SQL sites, and explicit implementations. |
| `find_references` | Callers or callees from Vera's stored reference data. |
| `explain_path` | Why a path is or is not indexed: ignore rules, binary detection, size, or default exclusions. |

If the server's working directory has no index, the first call to `search_code`, `structural_search`, or `find_references` builds one and starts a file watcher. Search, references, and overview also accept git scopes that limit results to changed files.

A search result can include a second text block with warnings: the index is stale, semantic search was unavailable so results are keyword-only, or the reranker failed so results are not reranked. Each warning names its cause; see [Troubleshooting](troubleshooting.md#keyword-only-results).

### Querying another index

All seven tools accept an optional `repo` string: an indexed repository root or any directory inside one, absolute or relative to the server's working directory. Build its index first with `vera index <repo>`. For example, pass these arguments to `search_code`:

```json
{"repo":"/home/user/.cargo/registry/src","query":"retry backoff"}
```

A different repository is never auto-indexed or watched. Omitting `repo`, or selecting the server's working directory, keeps the normal auto-index and watcher behavior. Filters, relative file paths, and staleness checks use the selected repository. The existing project `path` argument on `get_stats` and `get_overview` remains supported; `repo` takes precedence.

## Client setup

The server uses the backend saved by `vera setup`, unless `VERA_BACKEND` is set (the Docker images set it). Configure a backend first.

By default, the server works on the project in its working directory. Most clients start MCP servers in the open project's root; if yours starts them elsewhere, set the client's working-directory option (often `cwd`) to the project root, or the first search will index the wrong directory. A server started inside an indexed repository uses that repository's index rather than creating a nested one. The server refuses to auto-index the filesystem root or your home directory; run `vera index <dir>` to index one deliberately.

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
