# Storage: SQLite + sqlite-vec + Tantivy

Vera stores chunk metadata, embedding vectors (768-dim in this original evaluation setup; the current Potion Code default is 256-dim and stored dimensions are configurable), and a BM25 full-text index. We compared SQLite+sqlite-vec vs LanceDB for the metadata/vector layer. Tantivy handles BM25 in both cases. it's the obvious Rust choice there.

## Benchmarks

AMD Ryzen 5 7600X3D, 30GB RAM. 10K chunks with 768-dim vectors.

### Write throughput

| Backend | Time (ms) | Chunks/sec |
|---------|-----------|------------|
| SQLite + sqlite-vec | 1,317 | 7,596 |
| LanceDB | 41 | 241,440 |

### Vector query latency (100 KNN queries, top-10, brute-force)

| Backend | p50 (ms) | p95 (ms) |
|---------|----------|----------|
| SQLite + sqlite-vec | 9.7 | 10.0 |
| LanceDB | 1.9 | 2.8 |

### BM25 query latency (Tantivy, shared)

p50: 0.067ms, p95: 0.133ms.

### Build complexity

| | SQLite | LanceDB |
|--|--------|---------|
| Dependency count | ~60 crates | 537 crates |
| Full build | ~40s | ~150s |
| Async required | No | Yes |

## Why SQLite wins

LanceDB is 5× faster at vector queries and 32× faster at writes, but SQLite's numbers are already well within budget (10ms vector query vs 500ms target, 1.3s index vs 60s target). What actually matters:

- 60 crates vs 537. binary size, compile time, supply chain risk
- Synchronous API. simpler code, no async runtime needed for storage
- Single `.db` file. trivial to inspect, back up, or delete
- rusqlite has 10+ years of production use

## Trade-offs

- 5× slower vector queries (10ms vs 2ms). irrelevant at Vera's scale
- sqlite-vec is young (v0.1.x). small API surface makes it easy to replace later
- No ANN index. brute-force is fine for <100K chunks
