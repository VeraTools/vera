# Contributing

## Build from source

You need Rust 1.88 or later. The exact minimum is the `rust-version` field in `Cargo.toml`.

```bash
git clone https://github.com/VeraTools/vera.git
cd vera
bash scripts/bootstrap-vendored-grammars.sh  # downloads the vendored tree-sitter grammars that are not in git
cargo build --locked
```

## Run tests

```bash
cargo test --locked --workspace       # all tests
cargo test --locked -p vera-core      # core crate only
```

## Lint and format

```bash
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Workspace lints reject debugging macros, unfinished implementations, unsafe operations outside explicit `unsafe` blocks, unsafe blocks without a safety comment, and lint suppressions without a reason. If you need a suppression, keep it narrow and give it a specific `reason`. Document the safety invariant at each unsafe operation.

CI also builds the public rustdoc, lints workflows with actionlint, lints shell scripts with ShellCheck, runs a conservative set of Ruff rules on the Python code, and syntax-checks the npm wrapper. CodeQL scans the Rust, Actions, Python, JavaScript, and grammar C/C++ sources. To run the same checks locally:

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
git ls-files 'packages/npm-cli/*.js' 'packages/npm-cli/**/*.js' | xargs -n1 node --check
```

CodeQL runs only in CI.

Vera ships as GitHub release binaries, the npm and PyPI wrappers, and Docker images. The workspace crates are not published; they are versioned together from the release tag. Release automation and its test fixtures use Python 3.11, while the published Python wrapper supports Python 3.9 and later.

## Project layout

| Crate | What it does |
|-------|-------------|
| `vera-core` | Parsing, indexing, storage, embedding, retrieval pipeline |
| `vera-cli` | CLI interface (clap) |
| `vera-mcp` | MCP server (JSON-RPC over stdio) |
| `vera-serve` | HTTP inference server |
| `eval` | Benchmark harness and evaluation tasks |

Most changes happen in `vera-core`:

- `parsing/`: tree-sitter grammars, AST chunking, symbol extraction
- `embedding/`: embedding providers (API, Potion Code, and local ONNX)
- `retrieval/`: BM25, vector search, RRF fusion, reranking
- `storage/`: SQLite metadata, Tantivy BM25 index, sqlite-vec vectors, and the flat SIMD vector sidecar
- `indexing/`: index build and incremental update pipeline

For how the pipeline fits together, see [docs/how-it-works.md](docs/how-it-works.md). The module map is in [docs/architecture.md](docs/architecture.md).

## Add a language

Follow the checklist in [docs/architecture.md](docs/architecture.md#adding-a-new-language). The current language list is in [docs/supported-languages.md](docs/supported-languages.md).

## Conventions

- Error handling: `anyhow::Result` in CLI code, `thiserror` for typed errors in `vera-core`.
- Async: the `tokio` runtime for I/O-bound work; keep CPU-bound work synchronous.
- Tests: `#[cfg(test)]` modules at the bottom of source files, with `tempfile` for filesystem tests.
- Commits: `type(scope): description`, for example `feat(lang): add HTML support` or `fix(retrieval): handle empty query`.
- Ranking changes must improve the full 1,251-task suite by at least 0.5% to ship; see [docs/benchmarks.md](docs/benchmarks.md#what-the-numbers-do-and-do-not-show).

## Run the benchmark

```bash
bash eval/setup-semble-corpus.sh                    # clone the pinned Semble corpus
cargo build --locked --release
cargo run --locked --release --bin vera-eval -- run \
  --tool vera-potion \
  --tasks-dir eval/tasks/semble \
  --corpus eval/semble-corpus.toml \
  --json-only
```

Methodology and current results are in [docs/benchmarks.md](docs/benchmarks.md).

## Branches

Open pull requests against `master`.

## Releases and recovery

Run the release helper from a clean `master` checkout once that exact commit has passed CI:

```bash
bash scripts/release.sh 2.0.0
```

Before tagging, the helper checks local and remote `master`, existing tags, and CI status. Versions come from the tag, so do not commit version bumps to the manifests.

npm and PyPI publish in separate jobs. If one fails, rerun that job from its workflow run; package versions that were already published stay as they are. Docker recovery checks the requested archive against its release manifest. Versioned images are never overwritten, and the moving variant tags (such as `cpu`) only ever point at the newest stable release.

To rerun the release for an existing tag on GitHub-hosted runners:

```bash
gh workflow run release.yml --ref master \
  -f tag=v1.4.2 -f use_github_runners=true
```

A full rerun refuses rebuilt assets that differ from what was already published. If the binary assets are complete, rerun only the failed package job, or use `docker.yml` for the images.

To rehearse all six binary targets and the package builds without publishing anything, pass a source commit and an existing tag for the version number:

```bash
gh workflow run release.yml --ref master \
  -f tag=v1.4.2 -f publish=false -f revision=<commit-sha>
```

The workflow never moves an existing tag.
