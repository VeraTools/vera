# Query guide

Vera works best when you describe what the code does. This page covers how to phrase queries, how to narrow results, and which command fits which question. The full list of flags is in [features](features.md).

## Pick the right command

| You want to know | Use |
|---|---|
| How or where something works | `vera search "how expired sessions are cleaned up"` |
| Where a named symbol is defined | `vera structural definitions SessionStore` |
| Who calls a function, or what it calls | `vera references cleanup_sessions` / `--callees` |
| Every route, env var read, or implementation | `vera structural routes` / `env` / `impls <symbol>` |
| An exact string or regex | `vera grep "SESSION_TTL"` |
| What a codebase looks like overall | `vera overview` |
| Something in the docs | `vera search "deploying behind a proxy" --scope docs` |

## Write queries that describe behavior

Good queries say what the code does:

```bash
vera search "JWT token validation"
vera search "retry failed requests with backoff"
vera search "database connection pooling"
vera search "where uploaded files are written to disk"
```

Single generic words match too much to rank well:

```bash
vera search "auth"     # which part of auth?
vera search "utils"
vera search "handler"
```

If you know the name, search for it. Keyword search handles exact identifiers, file names, and config keys directly:

```bash
vera search "AuthMiddleware"
vera search "max_connections"
```

For a definition, `vera structural definitions AuthMiddleware` is more direct: it returns the declaration instead of every place the name appears.

## When the first search misses

Try these in order:

1. **Rephrase in two or three ways at once.** Vera merges the results, so one call covers several angles:

   ```bash
   vera search "OAuth token refresh" "JWT expiry handling" "auth middleware"
   ```

2. **Add your goal with `--intent`** when the query is short or ambiguous:

   ```bash
   vera search "config" --intent "find where database connection strings are loaded from environment variables"
   ```

3. **Narrow with filters** (next section).
4. **Use `--deep`** for questions whose answer spans several files. See [deep search](features.md#deep-search).

If the top hit is a place that uses the symbol rather than defines it, take the name from that hit and run `vera structural definitions <name>` instead of trying more phrasings.

## Narrow the results

Add filters one at a time:

```bash
vera search "error handling" --lang rust
vera search "error handling" --lang rust --type function
vera search "routes" --path "src/**/*.ts" --path "tests/**/*.ts"
vera search "mod loader" --scope runtime --include-generated
vera search "token validation" --changed
```

- `--path` is relative to the repository root. A bare directory such as `--path src/api` matches everything below it.
- `--type function` also matches methods, and the reverse.
- `--scope docs` searches documentation; `--scope runtime` searches extracted or bundled app code; `--scope all` lets every content class except generated files match, as no `--scope` does, and source code still ranks first.
- `--changed` limits the search to modified, staged, and untracked files; `--base <rev>` to files changed since the merge base with `<rev>` (a branch diff); `--since <rev>` to files changed since `<rev>`.

Vera ranks source code above docs and tests; use `--scope` to search only one kind of file. Generated files such as minified bundles are left out of results unless you pass `--include-generated`. Directories such as `dist`, `build`, and `node_modules` are not indexed at all by default; see [Configuration](configuration.md) to change the exclusions.

## Callers and same-named symbols

`vera references` matches calls by name. If two classes both define `get`, their callers come back together. When the calls use at least two different receiver names, the output lists them:

```bash
vera references get                  # all calls to anything named get, with receiver counts
vera references get --receiver app   # only calls written as app.get(...)
```

Calls with no receiver, such as a plain `get()`, do not match any `--receiver` value.

## When to use `vera grep` vs `rg`

Use `vera grep` for exact text or regex in indexed files. It shares `vera search`'s filters and git scopes:

```bash
vera grep "EMBEDDING_MODEL_BASE_URL"
vera grep "TODO\(" -i
vera grep "queryClient|invalidateQueries" --path "frontend/src/**"
```

It uses Rust regex syntax: `|` is alternation and `\|` matches a literal pipe. If none of your `--path` patterns match an indexed file, Vera suggests a pattern that would.

Use `rg` when you need to:

- search files that are not indexed (dependencies, ignored or generated trees)
- count occurrences
- find files by name
- prepare a find-and-replace

## Keep results fresh

If Vera warns that the index is stale, or results miss code you just wrote:

```bash
vera update .      # once
vera watch .       # continuously while you work
```

If a file never shows up, `vera explain-path path/to/file` tells you why it is not indexed. More fixes are in [troubleshooting](troubleshooting.md).

## Reading results

Each result is labeled with its path, line range, symbol type, and name:

````text
```src/flask/cli.py:698-763 function:load_dotenv
````

Results are in ranked order; trust the order. When JSON output includes a `score`, it only ranks results within that query and cannot be compared across queries. Large symbols are split into parts and labeled `name (part N)`; in JSON, `symbol_name` holds the bare name and `part_index` the part.
