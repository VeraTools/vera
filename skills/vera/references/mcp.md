# Optional MCP Usage

Vera supports MCP, but MCP is not the preferred install path for agent users. The primary path is:

- install Vera
- install the Vera skill
- use the Vera CLI directly

Use MCP only when:

- the client explicitly requires MCP
- the user asks to integrate Vera through MCP
- the environment already has an MCP-centric workflow

## Start The Server

Wrapper-based commands:

```sh
npx -y @vera-ai/cli mcp
bunx @vera-ai/cli mcp
uvx vera-ai mcp
```

If Vera is already installed on `PATH`:

```sh
vera mcp
```

The server exposes:

- `search_code` (supports `queries` array for multi-query search, `intent` for reranking, `path` argument as a string or an array of strings with OR semantics, and `changed` / `since` / `base` git scopes; auto-indexes and starts watcher on first use)
- `get_stats`
- `get_overview` (includes detected project conventions and supports git-scoped summaries)
- `regex_search` (regex, context, scope, generated-file toggle, compact output, and git-scoped file selection)
- `structural_search` (definitions, env reads, routes, SQL, and explicit implementation lookups; git scopes supported; auto-indexes and starts watcher on first use)
- `find_references` (indexed caller or callee matches, with git-scoped filtering; auto-indexes and starts watcher on first use)
- `explain_path` (explains why a file is or is not indexed)

## Guidance

- Keep the Vera skill CLI-centered.
- Keep the MCP surface small. Reach for the CLI skill when you need the full set of filters and commands.
- Only mention MCP when the task or client explicitly depends on it.
- Do not assume MCP is installed if the user only asked for Vera search capability.
