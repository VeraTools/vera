#!/usr/bin/env bash
# Measure token reduction: Vera search output vs loading full files.
#
# For each query, compares:
#   (a) Vera's search output (markdown codeblocks, default format)
#   (b) Full content of every unique file referenced in the results
#
# Token counts use a simple whitespace+punctuation heuristic (~4 chars/token)
# which closely approximates GPT/Claude tokenizers for code.

set -euo pipefail

VERA="${VERA_BIN:-target/release/vera}"
LIMIT="${LIMIT:-10}"

# Representative queries spanning different search patterns
QUERIES=(
  "hybrid search with reranking"
  "parse tree-sitter grammar"
  "BM25 keyword search"
  "embedding provider"
  "index incremental update"
  "search filters language path"
  "SQLite storage chunks"
  "MCP server JSON-RPC"
  "symbol extraction classify node"
  "error handling anyhow"
  "config runtime defaults"
  "file discovery gitignore"
  "how does vector search work"
  "authentication middleware"
  "Cargo.toml dependencies"
  "CLI argument parsing clap"
  "cross-encoder reranker"
  "reciprocal rank fusion"
  "chunk splitting strategy"
  "download ONNX model"
)

estimate_tokens() {
  # ~4 chars per token, standard approximation for code
  local chars
  chars=$(wc -c < "$1")
  echo $(( (chars + 3) / 4 ))
}

total_vera_tokens=0
total_file_tokens=0
query_count=0

printf "%-50s %10s %10s %8s\n" "Query" "Vera" "FullFiles" "Reduction"
printf "%-50s %10s %10s %8s\n" "-----" "----" "---------" "---------"

for query in "${QUERIES[@]}"; do
  # Get Vera's default output (markdown codeblocks)
  vera_output=$("$VERA" search "$query" --limit "$LIMIT" 2>/dev/null || true)

  if [ -z "$vera_output" ]; then
    continue
  fi

  # Save vera output to temp file for token counting
  vera_tmp=$(mktemp)
  echo "$vera_output" > "$vera_tmp"
  vera_tokens=$(estimate_tokens "$vera_tmp")

  # Extract unique file paths from the output
  # Format: ```path/to/file:start-end ...
  files_tmp=$(mktemp)
  echo "$vera_output" | grep -oP '(?<=^```)([^:]+)' | sort -u > "$files_tmp"

  # Concatenate full file contents
  full_tmp=$(mktemp)
  : > "$full_tmp"
  while IFS= read -r fpath; do
    if [ -f "$fpath" ]; then
      cat "$fpath" >> "$full_tmp"
      echo "" >> "$full_tmp"
    fi
  done < "$files_tmp"

  file_tokens=$(estimate_tokens "$full_tmp")

  if [ "$file_tokens" -gt 0 ]; then
    reduction=$(awk "BEGIN { printf \"%.1f%%\", (1 - $vera_tokens / $file_tokens) * 100 }")
    total_vera_tokens=$((total_vera_tokens + vera_tokens))
    total_file_tokens=$((total_file_tokens + file_tokens))
    query_count=$((query_count + 1))
    printf "%-50s %10d %10d %8s\n" "${query:0:50}" "$vera_tokens" "$file_tokens" "$reduction"
  fi

  rm -f "$vera_tmp" "$files_tmp" "$full_tmp"
done

echo ""
if [ "$total_file_tokens" -gt 0 ]; then
  avg_reduction=$(awk "BEGIN { printf \"%.1f%%\", (1 - $total_vera_tokens / $total_file_tokens) * 100 }")
  echo "Queries measured: $query_count"
  echo "Total Vera tokens: $total_vera_tokens"
  echo "Total full-file tokens: $total_file_tokens"
  echo "Average token reduction: $avg_reduction"
fi
