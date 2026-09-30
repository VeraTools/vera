# Migrating to Vera v2

Upgrade the binary or wrapper as usual. Ordinary default indexes remain compatible, and the search-result JSON fields are unchanged.

## Retired Experiments

V2 removes four rejected experiments:

| Experiment | Removed configuration keys | Removed environment variables |
|---|---|---|
| Multiplicative path penalty | `retrieval.ranking_multiplicative_path_penalty`, `retrieval.ranking_path_penalty`, `retrieval.ranking_multiplicative_penalty` | `VERA_RANKING_MULTIPLICATIVE_PATH_PENALTY`, `VERA_RANKING_PATH_PENALTY`, `VERA_RANKING_MULTIPLICATIVE_PENALTY` |
| Uniform candidate-pool multiplier | `retrieval.ranking_candidate_pool_multiplier`, `retrieval.ranking_candidate_pool_size_multiplier`, `retrieval.ranking_pool_multiplier` | `VERA_RANKING_CANDIDATE_POOL_MULTIPLIER`, `VERA_RANKING_CANDIDATE_POOL_SIZE_MULTIPLIER`, `VERA_RANKING_POOL_MULTIPLIER`, `VERA_RANKING_CANDIDATE_POOL_MULT` |
| Character-cap chunking | `indexing.chunk_max_chars`, `indexing.max_chunk_chars`, `indexing.max_chunk_characters` | `VERA_INDEXING_CHUNK_MAX_CHARS`, `VERA_INDEXING_MAX_CHUNK_CHARS`, `VERA_MAX_CHUNK_CHARS`, `VERA_CHUNK_MAX_CHARS` |
| Structural graph augmentation | None | `VERA_GRAPH_AUGMENT` |

Existing configuration files still load. Removed keys are ignored and disappear when configuration is saved; `vera config get` and `vera config set` report them as unknown keys. Removed environment variables no longer affect behavior. The supported query-dependent recall-pool expansion and explicit `references` queries remain available. See [ADR 007](adr/007-ranking-hypotheses.md) for the decision and evidence.

If an index was built with a nonzero character cap, search and incremental update require a full rebuild:

```bash
vera index /path/to/repository
```

Indexes with missing or zero character-cap metadata continue to work. Running `vera update` cannot convert an incompatible index because unchanged files would retain the old chunk boundaries.

## Search Scores

Use the returned result ordering. The JSON `score` remains a pipeline-specific ranking value that may be rank-normalized; it is neither a probability nor comparable across queries. [How it works](how-it-works.md) explains the pipeline.
