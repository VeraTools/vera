# Contributing

## Build from Source

Rust 1.88+ required (see `Cargo.toml` `rust-version` for exact MSRV).

```bash
git clone https://github.com/VeraTools/vera.git
cd vera
bash scripts/bootstrap-vendored-grammars.sh  # Downloads the vendored tree-sitter grammars used by CI.
cargo build --locked
```

## Run Tests

```bash
cargo test --locked --workspace       # all tests
cargo test --locked -p vera-core      # core crate only
```

## Lint & Format

```bash
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Workspace lints deny debugging macros, unfinished implementations, unsafe operations without explicit blocks, undocumented unsafe blocks, and unexplained lint suppressions. Keep each necessary suppression narrow and include a specific `reason`. Document the safety invariant at each unsafe operation.

CI also checks public rustdoc, workflows with Actionlint and ShellCheck, first-party shell scripts, conservative Ruff rules, and npm JavaScript syntax. CodeQL default setup scans the detected Rust, Actions, Python, JavaScript and grammar C/C++ sources.

```bash
cargo metadata --locked --format-version=1 > /dev/null
cargo check --locked --workspace --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps
cargo deny --locked check
cargo audit
cargo machete
actionlint
shellcheck eval/*.sh scripts/*.sh
ruff check --select E4,E7,E9,F benchmarks eval scripts packages/python-cli
```

The supported distributions are GitHub binaries, npm, PyPI and Docker. Workspace crates are private and versioned together from release tags. Release automation and its fixtures use Python 3.11; the installed Python wrapper supports Python 3.9.

## Project Layout

| Crate | What it does |
|-------|-------------|
| `vera-core` | Parsing, indexing, storage, embedding, retrieval pipeline |
| `vera-cli` | CLI interface (clap) |
| `vera-mcp` | MCP server (JSON-RPC over stdio) |
| `vera-serve` | HTTP inference server |
| `eval` | Benchmark harness and evaluation tasks |

The core engine lives in `vera-core`. Most changes happen here:

- `parsing/`: tree-sitter grammars, AST chunking, symbol extraction
- `embedding/`: embedding providers (API, Potion Code, and local ONNX)
- `retrieval/`: BM25, vector search, RRF fusion, reranking
- `storage/`: SQLite metadata, Tantivy BM25 index, sqlite-vec vectors, and the flat SIMD vector sidecar
- `indexing/`: index build and incremental update pipeline

For how the pipeline fits together, see [docs/how-it-works.md](docs/how-it-works.md).

## Adding a Language

See [docs/architecture.md](docs/architecture.md#adding-a-new-language) for the step-by-step checklist. The full language list is at [docs/supported-languages.md](docs/supported-languages.md).

## Conventions

- **Error handling:** `anyhow::Result` in CLI code, `thiserror` for typed errors in `vera-core`
- **Async:** `tokio` runtime for I/O-bound work
- **Tests:** `#[cfg(test)]` modules at the bottom of source files, `tempfile` for filesystem tests
- **Commits:** `type(scope): description`: e.g. `feat(lang): add HTML support`, `fix(retrieval): handle empty query`

## Running Benchmarks

```bash
bash eval/setup-semble-corpus.sh                    # clone the pinned Semble corpus
cargo build --locked --release
cargo run --locked --release --bin vera-eval -- run \
  --tool vera-potion \
  --tasks-dir eval/tasks/semble \
  --corpus eval/semble-corpus.toml \
  --json-only
```

Benchmark details: [docs/benchmarks.md](docs/benchmarks.md).

## Branch Policy

Pull requests target `master` directly.

## Releases and Recovery

Run the release helper from clean `master` after its exact commit passes CI:

```bash
bash scripts/release.sh 2.0.0
```

The helper checks local and remote `master`, existing tags, and CI before tagging. Versions come from the tag; do not commit manifest version bumps.

npm and PyPI have separate publication jobs. Retry a failed job from its workflow run; already published package versions are preserved. Docker recovery verifies the requested archive against its release manifest. Versioned images are preserved, and mutable variant tags follow only the newest published stable release.

To use GitHub-hosted runners for an existing tag:

```bash
gh workflow run release.yml --ref master \
  -f tag=v1.4.2 -f use_github_runners=true
```

A full retry rejects rebuilt assets that differ from the publication. Retry only the failed package job or use `docker.yml` when the binary assets are already complete.

Rehearse all six binary targets and package builds without publication, using an exact source commit and an existing tag for version stamping:

```bash
gh workflow run release.yml --ref master \
  -f tag=v1.4.2 -f publish=false -f revision=<commit-sha>
```

The workflow never moves an existing tag.
