# Local inference with llama.cpp

Vera's API mode talks to any OpenAI-compatible embedding and reranking endpoint, so you can serve models yourself with [llama.cpp](https://github.com/ggerganov/llama.cpp) and keep everything on your machine. The model files below are examples; use any GGUF embedding or reranker model you prefer.

You need:

- llama.cpp built with `llama-server`
- an embedding model in GGUF format, for example `coderankembed-q8_0.gguf`
- optionally, a reranker model such as `bge-reranker-base-q8_0.gguf`
- optionally, a chat model for `vera search --deep`

## 1. Start the embedding server

```bash
llama-server \
  -m /path/to/coderankembed-q8_0.gguf \
  --port 8059 \
  --embedding \
  -c 8192
```

Check that it responds:

```bash
curl -s http://localhost:8059/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{"input": "test", "model": "coderankembed-q8_0.gguf"}' | head -c 200
```

## 2. Start a reranker server (optional)

```bash
llama-server \
  -m /path/to/bge-reranker-base-q8_0.gguf \
  --port 8060 \
  --reranking \
  -c 512
```

Check it:

```bash
curl -s http://localhost:8060/v1/rerank \
  -H "Content-Type: application/json" \
  -d '{"query": "test", "documents": ["hello", "world"], "model": "bge-reranker-base-q8_0.gguf"}' | head -c 200
```

## 3. Start a completion server (optional)

With a chat completion endpoint configured, `vera search --deep` expands your query into several phrasings and merges the results. Without one, `--deep` follows symbols from the first results instead.

```bash
llama-server \
  -m /path/to/your-completion-model.gguf \
  --port 8061 \
  -c 4096
```

## 4. Point Vera at the servers

Export the endpoints. Local servers do not check keys, but Vera requires a value for the embedding and reranker keys. The completion key is optional.

```bash
export EMBEDDING_MODEL_BASE_URL="http://localhost:8059/v1"
export EMBEDDING_MODEL_ID="coderankembed-q8_0.gguf"
export EMBEDDING_MODEL_API_KEY="not-needed"

# Optional reranker
export RERANKER_MODEL_BASE_URL="http://localhost:8060/v1"
export RERANKER_MODEL_ID="bge-reranker-base-q8_0.gguf"
export RERANKER_MODEL_API_KEY="not-needed"

# Optional completion model for --deep
export VERA_COMPLETION_BASE_URL="http://localhost:8061/v1"
export VERA_COMPLETION_MODEL_ID="your-completion-model.gguf"
export VERA_COMPLETION_API_KEY="not-needed"

# Optional: skip the daily update check when offline
export VERA_NO_UPDATE_CHECK="1"
```

Save the embedding and reranker endpoints to Vera's config. With the reranker variables set, setup also turns reranking on:

```bash
vera setup --api --yes
```

The completion variables are read from the environment and are not saved by setup, so keep them in your shell profile.

To enter the same values interactively instead, run `vera setup`, choose API mode, then Custom.

## 5. Index and search

```bash
vera index .
vera search "authentication logic"
vera search "config loading" --deep
```

## 6. Tune for smaller models

Local models often have shorter context windows than hosted ones. If you see truncation or context errors:

```bash
# Smaller chunks for embedding models with short context
vera config set indexing.max_chunk_bytes 1800

# Fewer and shorter documents per reranker request
export VERA_MAX_RERANK_BATCH=8
export VERA_MAX_RERANK_DOC_CHARS=1200

# More room for reasoning models used with --deep
export VERA_COMPLETION_MAX_TOKENS=16384
export VERA_COMPLETION_TIMEOUT_SECS=120
```

After changing `indexing.max_chunk_bytes`, run `vera index .` so every file is chunked with the new limit.

## 7. Use it from an MCP client

Configure the client as described in [MCP](mcp.md). The `vera mcp` process must see any environment variables you rely on, such as the completion settings, so set them in your shell profile or in a wrapper script the client launches.

## Troubleshooting

| Symptom | Fix |
|---|---|
| Embedding requests fail | Check that the server is running and that `EMBEDDING_MODEL_ID` matches the model name the server reports. |
| Reranker context errors | Lower `VERA_MAX_RERANK_DOC_CHARS` or `VERA_MAX_RERANK_BATCH`. |
| `--deep` behaves like normal search | The completion variables are not set in this environment. |
| `--deep` fails during query expansion | Make sure the completion model returns JSON; raise `VERA_COMPLETION_MAX_TOKENS` for reasoning models. |
| Indexing is slow | Lower `indexing.max_chunk_bytes`, or exclude generated and vendored code in `.veraignore`. |
