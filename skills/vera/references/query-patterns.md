# Query Patterns

## Good Vera Queries

```sh
vera search "authentication middleware"
vera search "JWT token validation"
vera search "parse_config"
vera search "request rate limiting" --lang rust
vera search "routes" --path "src/**/*.ts" --path "tests/**/*.ts"
vera search "handler" --type function --limit 5
vera search "token validation" --changed
vera search "config loading" --base origin/main
```

## Weak Vera Queries

Single generic words return noise:

- `vera search "code"`
- `vera search "utils"`
- `vera search "file"`

Fix: describe what the code *does*, not what it *is*.

## When To Use `vera references` Instead

For structural queries about call relationships, use `references` or `dead-code` instead of `search`:

```sh
vera references parse_config            # who calls parse_config?
vera references parse_config --callees  # what does parse_config call?
vera dead-code                          # candidates with no indexed caller matches
```

These query name-based reference data extracted during indexing. Dynamic dispatch, reflection, generated calls, and external callers can be missed; same-name symbols can be confused. Review dead-code candidates before deletion.

## When To Use `vera grep` Instead of `rg`

`vera grep` searches only indexed files, so `.veraignore` and exclusion rules apply automatically. Use it when you want results scoped to the project's indexed corpus.

```sh
vera grep "fn\s+main"                      # regex over indexed files
vera grep "TODO|FIXME" -i                   # case-insensitive
vera grep "queryClient|invalidateQueries" --path "frontend/src/**"
vera grep "Authorization" --lang rust --type function
vera grep "handler" --scope docs             # scoped to documentation
vera grep "use std::collections" --context 0 # no surrounding context lines
vera grep "parse" --compact                  # signatures only
```

## When To Use `rg` Instead

- File name search: `rg --files | rg "docker"`
- Counting occurrences
- Bulk find-and-replace prep
- Files outside the Vera index

## Narrowing Results

Add one filter at a time:

1. `--lang rust`: restrict to a language
2. `--path "src/auth/**"`: restrict to a path glob; repeat it to OR multiple patterns
3. `--type function`: restrict to symbol type
4. `--limit 3`: fewer, higher-confidence results
5. `--scope source`: restrict to application source; other scopes are `docs`, `runtime`, and `all`

## Multi-Query Search

`vera search` accepts multiple quoted queries and merges the results with reciprocal rank fusion:

```sh
vera search "OAuth token refresh" "JWT expiry handling" "auth middleware"
```

Use this when one phrasing is too narrow but the task is still one coherent search.

## Git-Scoped Search

When the task is limited to modified files or a PR diff, scope the search first:

```sh
vera search "auth middleware" --changed
vera grep "TODO|FIXME" --changed
vera overview --base origin/main
```

Use:

- `--changed` for modified, staged, and untracked files
- `--since <rev>` for changes since a specific revision
- `--base <rev>` for changes since `merge-base(HEAD, <rev>)`

## Intent-Based Reranking

Add `--intent` when the raw query is short but you know the higher-level goal:

```sh
vera search "config" --intent "find where database connection strings are loaded from environment variables"
```

Use this when the raw query is too short or ambiguous to capture what you actually need.

## Structural Search

Use `vera structural` for the common structural tasks agents hit repeatedly:

```sh
vera structural definitions parse_config
vera structural env DATABASE_URL
vera structural routes --path "src/**"
vera structural sql
vera structural impls Loader
```

Use `vera structural impls <symbol>` for explicit inheritance or conformance declarations only. It does not infer implicit interface satisfaction.
