# ADR 011: Indexing reliability for API backends and scripted consumers

## Summary

Vera v2.0.0 makes API-embedding indexing resumable, refuses half-written indexes, sends larger and fewer concurrent requests, retries with backoff and requeue, and reports what happened in the index and update JSON summaries. The machine-readable CLI surface that subprocess consumers depend on stays additive.

Status: Accepted

## Context

Tools such as PR review bots run `vera index . --json` or `vera update . --json` non-interactively, keep only the tail of stderr, and parse stdout. A user-measured run on a 168-file, 1,744-chunk Rust repository with v1 API defaults (8 concurrent requests, 16 inputs in flight, 60-second timeout) ran 435 seconds, then failed on a timeout and left no index. The same endpoint with 2 requests of 128 inputs and a 120-second timeout finished in 63 seconds. Stderr showed no progress, retries, or timeouts during the long run.

## Decisions

### Completeness and resume

- A full `vera index` builds a sibling `.vera.build/` and publishes it only on success, so a failure never replaces a healthy `.vera/`.
- A failed API build keeps finished embeddings in `<repo>/.vera.resume/embeddings.db`. Entries are keyed by the SHA-256 of the chunk text, and the file records an identity of model name plus document prefix; a mismatched identity discards the file. The next build reuses matching vectors (`embeddings_reused`) and deletes the checkpoint after it publishes. The checkpoint is best-effort: if it cannot be written, indexing continues without it. Only API builds checkpoint.
- `vera update` writes into the live index, so it sets `index_complete` to `"0"` in index metadata before writing and back to `"1"` when done. Search and `vera stats` refuse an index marked `"0"` unless a writer still holds the index lock. The next `vera update` repairs every file the interrupted run touched, without a `max_files` cap. Indexes without the key (pre-v2) are treated as complete.

Known limitation: when a batch is split because it exceeds the model context, the sub-batch vectors are not checkpointed. A rerun re-embeds them; results are not affected.

### Request shaping

- Defaults: `embedding.max_in_flight_inputs` 256 (two 128-input requests at a time) and `embedding.timeout_secs` 120. A `config_format` marker upgrades saved v1 defaults (16 and 60) once, per field; other explicit values stay.
- The request loop is a sliding window: a finished request frees its slot immediately instead of waiting for the slowest request in its group. A provider error that has already arrived wins over cancellation, and the first terminal error is the one reported.
- Retries use jittered exponential backoff (500 ms base, 30 s cap; 2 s base for rate limits) and honor `Retry-After` up to 60 seconds. A batch that exhausts its retries goes to the back of the queue, up to twice, still honoring `Retry-After`. Authentication and other non-retryable 4xx errors fail at once.

### Reporting and output contract

- Index and update JSON summaries add `embedding_requests` (HTTP attempts), `embedding_retries`, `embedding_timeouts`, `embedding_failed_batches`, `embeddings_reused`, and `phase_secs`. Existing fields are unchanged.
- Progress goes to stderr, throttled, including when stderr is not a terminal. On failure stdout stays empty, the exit code is nonzero, a stats line precedes the error, and the error is the last stderr line.
- Default `vera search --json` stays a bare array. Reranking state is opt-in through `--rerank-status`. Every reranker fallback still prints `reranker unavailable` on stderr.
- `vera --version` prints `vera X.Y.Z`. Config values persist under `core_config` in `$VERA_HOME/config.json`; unknown keys fail and `null` clears a key.
- Error text drops endpoint URLs and redacts tokens and key-value credentials from provider responses.

### Potion Code batching

The Potion tokenizer pads each batch to its longest text, and model2vec pools the pad tokens, so a chunk's vector depended on its batch neighbors. Each text is now tokenized alone, in parallel. Vectors match single-text query embedding, and indexing the Zig corpus takes about 18 seconds instead of 25 to 27.

## Evidence

A deterministic local mock endpoint (base latency plus per-input cost, a seeded 90-second tail on about one request in 30, and 1 or 4 server lanes) served a 168-file, 1,772-chunk fixture. Each row is 5 seeds at each lane count; times are real-world seconds.

| Configuration | Successful runs | Median, 4 lanes | Worst, 4 lanes |
|---|---:|---:|---:|
| v1 defaults, v1 code | 0 of 10 | n/a | n/a |
| v1 defaults, v2 retries and window | 10 of 10 | 677 s | 802 s |
| v2 defaults, group barrier | 10 of 10 | 118.5 s | 204 s |
| v2 defaults, sliding window | 10 of 10 | 100.9 s | 128 s |

With one server lane, v2 defaults take a median of 149 seconds with or without the window.

On the 1,251-task Semble suite with fresh Potion indexes, v2 scored nDCG@10 0.84371 and Recall@5 0.91993 against a baseline of 0.84371 and 0.91873. Two rebuilds of the same code differ by about 0.0004 nDCG because tied scores reorder; search over identical indexes was byte-identical to the build before these changes on every task.
