---
name: vera
description: Code search over the current repository. In a repository with a Vera index, your first search goes through Vera, not grep, rg, or file reads. Run `vera overview` in an unfamiliar repository, `vera search "<what the code does>"` for how or where something works, `vera structural definitions <symbol>` or `vera references <symbol>` when the question names a symbol, and `vera grep "pattern"` for exact text or regex. Fall back to grep after a Vera call has missed, or use your usual search tools when no index exists.
---

# Vera

Ranked code search over an indexed repository. Results are markdown codeblocks: `path:line_start-line_end symbol_type:symbol_name` (split symbols render as `name (part N)` in that position, the bare name plus the part suffix), then the code. The returned order is authoritative. When a response includes `score`, it is a pipeline-specific ranking value, not a probability or a value to compare across queries.

## First search

Use the following in indexed repositories. Without an index, follow [Recovery](#recovery); without a shell, use your usual search tools.

1. Unfamiliar repository: `vera overview` (languages, entry points, hotspots) instead of `ls` and README skimming.
2. Then the question's first lookup goes through the table below. Do not "check with grep first".

## Pick the tool

| Question shape | Do this |
|----------------|---------|
| How or where does something work | `vera search "env file loading decision"` |
| The question names a function, class, or config key | `vera structural definitions make_config`, then `vera references make_config` for callers |
| Who calls or what is called by a symbol | `vera references make_config` / `vera references make_config --callees` |
| Exact text or regex | `vera grep "before_request"` (line numbers always shown; `--limit <N>` caps results) |
| Enumerate every route, env read, implementation | `vera structural routes` / `vera structural env` / `vera structural impls <symbol>` |
| Documentation on a topic | `vera search "deploying behind proxy" --scope docs` |
| Files changed in this branch | add `--changed`, `--since <rev>`, or `--base <rev>` to any of the above |
| Edit the same pattern in many files mechanically | `rg` |
| Read a file you already know the path and lines of | Read it directly |

`vera references` resolves split-symbol call sites, `vera structural definitions` finds split symbols by bare name and deduplicates to the earliest declaration, and `vera dead-code` deduplicates split parts by (symbol, file).

Narrow callers or callees with `vera references make_config --path 'src/**/*.rs'`; repeat `--path` to include several patterns, just as with search.

## Search well

- Search behavior, not nouns: `"JWT expiry handling"`, not `"auth"` or `"utils"`.
- Pass several angles in one call: `vera search "OAuth token refresh" "JWT expiry" "auth middleware"`.
- Start broad with `--compact` (signatures only, fewer tokens), then narrow with `--lang`, `--path`, `--type`, `--limit`.
- `--path` is relative to the repository root: `--path src/flask`, `--path "tests/**/*.py"`. Absolute paths inside the repository are accepted and rewritten; paths outside it match nothing.
- Add `--intent "<goal>"` when the query is vague but the goal is clear.
- `--deep` expands the query and merges rankings; use it only after normal search misses.

## Recover from a miss

- Top hit is a usage site, not the definition: `vera structural definitions <symbol>` with the name from the hit. Do not try more phrasings.
- Hit shows a call but not the caller chain: `vera references <symbol>`.
- Two searches returned the same region: stop searching and read that code.
- The code is in a dependency (site-packages, uv or pip caches, vendored trees outside the index): use `rg` there, then return to Vera for repository code.
- For an indexed dependency tree, use `vera -C <dir> search "query"` (put `-C <dir>` before the subcommand); each tree keeps its own `.vera/`. To search the project and that tree together, use `vera search "query" --also <dir>`: in text output, hits from the other tree show absolute paths; `--json` keeps relative `file_path` values and adds an absolute `repo` field.

## Treat hits as leads

- A hit is a lead, not evidence. Verify behavior against the hit's code; open the file only for lines the hit did not include.
- Cite `path:line` from code you actually read.
- A stale-index warning does not invalidate hits. After editing files, run `vera update .` from any directory; it refreshes the repository root's index.

## Recovery

If you will work in a code repository without an index, run `vera index .` from its root. A first index can take several minutes: use a long shell timeout or run it in the background, and use rg until it finishes. An interrupted API-mode run reuses its finished embeddings. Otherwise, or without shell access, use your usual search tools.

| Symptom | Fix |
|---------|-----|
| `no index found` | Follow the indexing and fallback guidance above |
| `no indexed file matches <path>` | Use a root-relative `--path`; stderr suggests a wildcard directory form when one applies |
| Stale results after edits | `vera update .` (or `vera watch .`); works from any subdirectory |
| A file is missing from results | `vera explain-path path/to/file` |
| Local model or ONNX error | `vera doctor --probe`, then `references/troubleshooting.md` |
| Missing local assets | `vera repair` |
| Install, API keys, backends | `references/install.md` |
| MCP server | `references/mcp.md` |

## References

- [Install and setup](references/install.md): install, setup, API and local config, `.veraignore` rules
- [Query patterns](references/query-patterns.md): more query examples and rg guidance
- [Troubleshooting](references/troubleshooting.md): common errors and fixes
- [MCP](references/mcp.md): optional MCP server usage
