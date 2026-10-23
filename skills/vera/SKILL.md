---
name: vera
description: Code search over indexed repositories using the Vera MCP server. Prefer Vera MCP for its reusable, cached indexes; load or select the index for the target repository, then use search_code for behavior, structural_search for definitions and implementations, find_references for callers, and regex_search for exact text.
---

# Vera

Vera MCP searches indexed repositories and can keep multiple indexes loaded in one server process. Prefer it over one-off local searches: the existing index can be reused and cached rather than rebuilding search context for each request.

## Select the repository index

The typical index directory is `.vera` at the repository root. An index can also be stored elsewhere, such as a shared index directory. Use `list_indexes` to see indexes registered with the MCP server and their repository roots. Use `load_index` with an existing index directory when the needed index is not registered.

When more than one index is loaded, route searches to the intended repository with `scope_path`, using its absolute repository path. `search_code` also accepts `index_name` as an alternative selector. `scope_path` selects the index scope; it does not itself filter results to a subdirectory. Use `path` to narrow results within that repository.

Use `get_overview` for an unfamiliar indexed repository and `get_stats` to inspect its size and language breakdown. If no loaded index covers the target repository, first check `list_indexes`, then load the appropriate existing index.

## Choose the MCP search tool

| Question shape | Do this |
|----------------|---------|
| How or where something works | `search_code` with a natural-language description of the behavior |
| Definition or explicit implementation of a named symbol | `structural_search` with `kind: "definitions"` or `"implementations"` |
| Callers or callees of a symbol | `find_references` |
| Exact text, identifier, or regex | `regex_search` |
| Routes, environment reads, or SQL query sites | `structural_search` with `kind: "route_handlers"`, `"env_reads"`, or `"sql_queries"` |
| Documentation on a topic | `search_code` with `scope: "docs"` |
| Files changed in this branch | Use the MCP tool's `changed`, `since`, or `base` filter |

`search_code` performs hybrid BM25 (keyword) and vector (semantic) retrieval when the index has embeddings. Some indexes are full-text-only and have no embeddings; searches against these rely on lexical/BM25 retrieval, so try a few concrete terms and synonyms rather than only abstract natural-language descriptions. Use `regex_search` for exact strings or patterns regardless of embedding availability. Check `get_stats` or the index metadata from `list_indexes` when you need to understand an index's characteristics.

Results include paths, line numbers, and code snippets. Treat hits as leads and verify relevant behavior in the returned code. Ranked order is authoritative; any `score` is pipeline-specific, not a probability or comparable across queries.

## Search well

- Search behavior, not just nouns: describe the goal or behavior you want to locate.
- Pass 2-3 varied queries in one `search_code` call when useful.
- Start with `compact` for signatures only, then narrow with `lang`, `path`, `symbol_type`, or `limit`.
- `path` is relative to the selected repository root; it accepts a string or an array of patterns.
- Add `intent` when the goal is clear but the query is ambiguous.
- For an exact symbol, use structural search or references rather than trying many conceptual phrasings.

## When a result is missing

- If the top hit is a usage site rather than a definition, use `structural_search` for the definition.
- If a hit shows a call but not its caller chain, use `find_references`.
- If an expected file is absent, use `explain_path` to learn why it is not indexed.
- A stale-index warning means changed or new files may not be represented. Do not treat missing results from a stale index as proof that code is absent.
- If no suitable index exists or MCP cannot access the repository, report the limitation rather than silently treating an incomplete search as exhaustive.

| Symptom | Fix |
|---------|-----|
| No loaded index covers the repository | Use `list_indexes` and load the existing index with `load_index` |
| Results come from the wrong repository | Set the correct `scope_path` or `index_name` |
| No results for a broad semantic query | Check whether the index has embeddings; for full-text-only indexes, try concrete keywords or `regex_search` |
| A file is missing from results | Use `explain_path` on the repository-relative or absolute path |
