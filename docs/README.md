# Vera documentation

New to Vera? Start with the [README](../README.md) for what it is and a three-command quick start.

## Get started

- [Installation](installation.md): install methods, choosing a backend, agent skills, updates, and uninstalling.
- [Query guide](query-guide.md): how to phrase queries and which command answers which question.
- [Features](features.md): a tour of every command, grouped by task.

## Connect it to your tools

- [MCP](mcp.md): set up the MCP server in Claude Code, Codex, Cursor, VS Code, and other clients.
- [Docker](docker.md): run Vera and its MCP server in a container, on CPU or GPU.
- [llama.cpp](llama-cpp-setup.md): use your own local embedding, reranking, and completion servers.

## Reference

- [Models and backends](models.md): CPU, GPU, and API options, with measurements.
- [Configuration](configuration.md): every `vera config` key and environment variable.
- [Supported languages](supported-languages.md): languages, file types, and how each is chunked.
- [Troubleshooting](troubleshooting.md): fixes for common setup, indexing, and search problems.
- [Manual install](manual-install.md): install models and runtimes by hand on restricted networks.

## How it is built and measured

- [How Vera works](how-it-works.md): the search pipeline from parsing to ranking.
- [Benchmarks](benchmarks.md): current results, the agent benchmark, and methodology.
- [Benchmark history](benchmarks-history.md): earlier results, model screenings, and rejected experiments.
- [Architecture](architecture.md): crate and module layout, for contributors.
- [Architecture decisions](adr/000-decision-summary.md): why each major component was chosen.

## Releases

- [What's new](whats-new.md): release notes.
- [Migrating to v2](migration-v2.md): what changed for 1.x users.
