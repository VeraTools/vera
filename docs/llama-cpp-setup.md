# Local Inference Setup (API Mode + llama.cpp)

Vera's `api` backend mode works with any OpenAI-compatible embedding and reranking endpoints. This guide covers running those endpoints locally with [llama.cpp](https://github.com/ggerganov/llama.cpp).

## Prerequisites

- llama.cpp built with `llama-server` (or a compatible server binary)
- Embedding model GGUF file (e.g. `coderankembed-q8_0.gguf`)
- Reranker model GGUF file (e.g. `bge-reranker-base-q8_0.gguf`)
- Optional: a completion model for `vera search --deep` RAG-fusion query expansion

## 1. Start the Embedding Server

```bash
llama-server \
  -m /path/to/coderankembed-q8_0.gguf \
  --port 8059 \
  --embedding \
  -c 8192
```

Verify it works:

```bash
curl -s http://localhost:8059/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{"input": "test", "model": "coderankembed-q8_0.gguf"}' | head -c 200
```

## 2. Start the Reranker Server (Optional)

```bash
llama-server \
  -m /path/to/bge-reranker-base-q8_0.gguf \
  --port 8060 \
  --reranking \
  -c 512
```

Verify:

```bash
curl -s http://localhost:8060/v1/reranking \
  -H "Content-Type: application/json" \
  -d '{"query": "test", "documents": ["hello", "world"], "model": "bge-reranker-base-q8_0.gguf"}' | head -c 200
```

## 3. Start a Completion Server (Optional, for `--deep` Search)

Any OpenAI-compatible chat completion endpoint works. This enables RAG-fusion query expansion for `vera search --deep`.

```bash
llama-server \
  -m /path/to/your-completion-model.gguf \
  --port 8061 \
  -c 4096
```

## 4. Configure Vera

### Option A: Environment Variables

Run commands directly from this shell, or persist the config with `vera setup --api --yes` after exporting the variables.

```bash
# Embedding (required)
export EMBEDDING_MODEL_BASE_URL="http://localhost:8059/v1"
export EMBEDDING_MODEL_ID="coderankembed-q8_0.gguf"
export EMBEDDING_MODEL_API_KEY="not-needed"

# Reranker (optional)
export RERANKER_MODEL_BASE_URL="http://localhost:8060/v1"
export RERANKER_MODEL_ID="bge-reranker-base-q8_0.gguf"
export RERANKER_MODEL_API_KEY="not-needed"

# Completion for --deep search (optional)
export VERA_COMPLETION_BASE_URL="http://localhost:8061/v1"
export VERA_COMPLETION_MODEL_ID="your-completion-model.gguf"
export VERA_COMPLETION_API_KEY="not-needed"
export VERA_COMPLETION_MAX_TOKENS="16384"
export VERA_COMPLETION_TIMEOUT_SECS="120"

# Skip update checks in offline environments
export VERA_NO_UPDATE_CHECK="1"

# Optional: persist the embedding and reranker endpoints in Vera config
vera setup --api --yes
```

### Option B: Interactive Setup

```bash
vera setup
# Choose API mode, then Custom and enter the URLs and model IDs above.
```

## 5. Index and Search

```bash
vera index .
vera search "authentication logic"
vera search "config loading" --deep   # uses completion endpoint for query expansion
```

## 6. Tuning for Local Models

Local models often have smaller context windows than cloud APIs. Adjust these settings if you see truncation or quality issues:

```bash
# Reduce chunk size for smaller embedding models
vera config set indexing.max_chunk_bytes 1800

# Limit reranker batch size and document length
export VERA_MAX_RERANK_BATCH=8
export VERA_MAX_RERANK_DOC_CHARS=1200

# Increase completion budget for reasoning models
export VERA_COMPLETION_MAX_TOKENS=16384
export VERA_COMPLETION_TIMEOUT_SECS=120
```

## 7. MCP Server Setup

To use Vera as an MCP tool server with local inference:

```bash
vera mcp
```

For MCP client configuration (e.g. in `opencode.json` or similar):

```json
{
  "mcpServers": {
    "vera": {
      "command": "vera",
      "args": ["mcp"],
      "cwd": "/absolute/path/to/repo"
    }
  }
}
```

Make sure the environment variables from step 4 are available to the MCP process (e.g. via shell profile or wrapper script).

## Troubleshooting

- **Embedding errors**: verify the server is running and the model ID matches the GGUF filename
- **Reranker context errors**: lower `VERA_MAX_RERANK_DOC_CHARS` or `VERA_MAX_RERANK_BATCH`
- **`--deep` behaves like normal search**: completion env vars are not set; Vera falls back to iterative symbol-following search
- **`--deep` fails with query expansion errors**: ensure the completion model returns JSON; increase `VERA_COMPLETION_MAX_TOKENS` for reasoning models
- **Slow indexing**: reduce `indexing.max_chunk_bytes` or add exclusions to `.veraignore`
