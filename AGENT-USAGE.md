# Agent usage

Vera is a code search CLI for the current repository. It returns ranked functions, classes, and other symbols with file paths and line ranges. The full agent instructions are in [`skills/vera/SKILL.md`](skills/vera/SKILL.md); `vera agent install` installs them for your agent.

## Set up

```bash
bunx @vera-ai/cli install           # install the binary (or: npx -y @vera-ai/cli install, uvx vera-ai install)
vera setup --potion-code --index .  # default local model, then index the repo (add .vera/ to .gitignore)
vera update .                       # after editing files; works from any subdirectory
```

## Pick the command

| Question | Command |
|---|---|
| Unfamiliar repository | `vera overview` |
| How or where something works | `vera search "env file loading decision"` |
| The question names a function, class, or config key | `vera structural definitions make_config` |
| Who calls a symbol, or what it calls | `vera references make_config` / `vera references make_config --callees` |
| Exact text or regex in indexed files | `vera grep "before_request"` |
| Every route, env read, or implementation | `vera structural routes` / `vera structural env` / `vera structural impls <symbol>` |
| Only files you changed | add `--changed` (uncommitted and untracked files), `--base <rev>` (changes since the merge base with a branch), or `--since <rev>` (changes since a revision) |
| Mechanical edits across many files, or code in dependencies outside the index | `rg` |

Describe behavior in search queries ("JWT expiry handling", not "auth"). `--path` is relative to the repository root. Use `--deep` only after a normal search misses.

## Output

Results are Markdown code blocks, each labeled `path:start-end symbol_type:symbol_name`, followed by the code. Results are in ranked order. `--compact` keeps signatures and drops bodies. `--json` gives compact JSON, `--raw` gives verbose output with every field, and `--timing` prints stage timings to stderr.

## More

Query patterns, troubleshooting, install details, and MCP setup are in [`skills/vera/references/`](skills/vera/references/).
