//! Deep search via RAG Fusion.
//!
//! 1. Decompose the user query into targeted sub-queries using a completion model.
//! 2. Execute standard hybrid search for each sub-query in parallel.
//! 3. Merge all results with weighted reciprocal rank fusion (original query
//!    receives higher weight).
//!
//! Falls back to iterative (symbol-following) search when no completion
//! endpoint is configured.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use anyhow::{Result, anyhow};
use tracing::{debug, warn};

use crate::config::VeraConfig;
use crate::retrieval::bm25::search_bm25;
use crate::types::{SearchFilters, SearchResult};

use super::completion_client::CompletionClient;
use super::hybrid::fuse_rrf_multi_weighted;
use super::search_service::{SearchContext, SearchTimings};
use super::{multi_query_candidate_limit, normalize_queries};

/// Execute deep search: RAG-fusion if a completion endpoint is configured,
/// otherwise fall back to iterative symbol-following search.
pub async fn execute_deep_search_with_context(
    context: &SearchContext,
    index_dir: &Path,
    query: &str,
    intent: Option<&str>,
    config: &VeraConfig,
    filters: &SearchFilters,
    result_limit: usize,
) -> Result<(Vec<SearchResult>, SearchTimings)> {
    let start = Instant::now();
    let plan = prepare_deep_search(index_dir, query).await?;
    let (results, mut timings) = execute_prepared_deep_search(
        context,
        index_dir,
        &plan,
        intent,
        config,
        filters,
        result_limit,
    )
    .await?;
    timings.total = Some(start.elapsed());
    Ok((results, timings))
}

/// Query expansion is independent of the target index after preparation.
/// `None` selects the existing iterative fallback, which needs no completion API.
pub struct DeepSearchPlan {
    query: String,
    expanded: Option<Vec<String>>,
}

/// Collect hints from the primary index and make at most one completion call.
/// Reuse the returned plan for every index in a cross-index search.
pub async fn prepare_deep_search(index_dir: &Path, query: &str) -> Result<DeepSearchPlan> {
    let completion_client = match tokio::task::spawn_blocking(
        CompletionClient::from_env_if_configured,
    )
    .await
    {
        Ok(Ok(client)) => client,
        Ok(Err(e)) => {
            warn!(error = %e, "completion client init failed, falling back to iterative search");
            None
        }
        Err(e) => {
            warn!(error = %e, "completion client init task failed, falling back to iterative search");
            None
        }
    };

    let Some(completion_client) = completion_client else {
        return Ok(DeepSearchPlan {
            query: query.to_string(),
            expanded: None,
        });
    };

    // BM25 pre-filter: run a cheap keyword search to gather codebase context
    // (symbol names and file paths) that helps the LLM generate better rewrites.
    let context_hints = {
        let index_dir = index_dir.to_path_buf();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || bm25_context_hints(&index_dir, &query))
            .await
            .map_err(|e| anyhow!("BM25 context hint task failed: {e}"))?
    };
    debug!(
        hints = context_hints.len(),
        "BM25 pre-filter produced context hints for query expansion"
    );

    let expanded = {
        let completion_client = completion_client.clone();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || {
            completion_client.expand_query_with_context(&query, &context_hints)
        })
        .await
        .map_err(|e| anyhow!("completion query expansion task failed: {e}"))?
        .map_err(|e| anyhow!("failed to generate deep-search query candidates: {e}"))?
    };

    let queries = dedupe_queries_with_original(query, expanded);
    if queries.len() <= 1 {
        return Err(anyhow!(
            "query expansion produced no additional rewrites; \
             check completion model output"
        ));
    }

    Ok(DeepSearchPlan {
        query: query.to_string(),
        expanded: Some(queries),
    })
}

/// Search one index using an already-expanded query, or follow its symbols
/// iteratively when no completion endpoint was available during preparation.
pub async fn execute_prepared_deep_search(
    context: &SearchContext,
    index_dir: &Path,
    plan: &DeepSearchPlan,
    intent: Option<&str>,
    config: &VeraConfig,
    filters: &SearchFilters,
    result_limit: usize,
) -> Result<(Vec<SearchResult>, SearchTimings)> {
    let Some(queries) = &plan.expanded else {
        return super::iterative_search::execute_iterative_search_with_context(
            context,
            index_dir,
            &plan.query,
            intent,
            config,
            filters,
            result_limit,
            1,
        )
        .await;
    };
    let overall_start = Instant::now();

    let per_query_limit = multi_query_candidate_limit(result_limit);

    let query_count = queries.len();

    let mut aggregated_timings = SearchTimings::default();
    let mut per_query_results: Vec<Vec<SearchResult>> = vec![Vec::new(); query_count];
    let mut per_query_weights: Vec<f64> = vec![0.0; query_count];

    // Run the candidate searches concurrently: latency then tracks the
    // slowest rewrite instead of the sum. Results keep their per-query slots.
    let outcomes =
        futures::future::join_all(queries.iter().map(|query| {
            context.search(index_dir, query, intent, config, filters, per_query_limit)
        }))
        .await;

    for (idx, result) in outcomes.into_iter().enumerate() {
        match result {
            Ok((results, timings)) => {
                aggregated_timings.merge(&timings);
                per_query_results[idx] = results;
                // Original query (idx 0) gets 2x weight.
                per_query_weights[idx] = if idx == 0 { 2.0 } else { 1.0 };
            }
            Err(e) if idx == 0 => return Err(e),
            Err(e) => {
                warn!(query = %queries[idx], error = %e, "deep-search subquery failed; continuing");
            }
        }
    }

    // Drop failed subqueries. The original query (idx 0) succeeded or returned
    // above, so an index with no hits yields an empty list, not an error, and a
    // cross-index search keeps the other indexes' results.
    let (filled_results, filled_weights): (Vec<_>, Vec<_>) = per_query_results
        .into_iter()
        .zip(per_query_weights)
        .filter(|(_, weight)| *weight > 0.0)
        .unzip();

    let slices: Vec<&[SearchResult]> = filled_results.iter().map(Vec::as_slice).collect();
    let fused = fuse_rrf_multi_weighted(
        &slices,
        &filled_weights,
        config.retrieval.rrf_k,
        result_limit,
    );

    aggregated_timings.total = Some(overall_start.elapsed());
    Ok((fused, aggregated_timings))
}

fn dedupe_queries_with_original(original: &str, alternatives: Vec<String>) -> Vec<String> {
    let mut all = Vec::with_capacity(alternatives.len() + 1);
    all.push(original.to_string());
    all.extend(alternatives);
    normalize_queries(&all)
}

/// Run a quick BM25 search and extract deduplicated symbol names and file
/// paths from the top results. These hints give the LLM real identifiers
/// from the codebase so it can produce more targeted query rewrites.
const BM25_PREFILTER_LIMIT: usize = 10;
const MAX_CONTEXT_HINTS: usize = 15;

fn bm25_context_hints(index_dir: &Path, query: &str) -> Vec<String> {
    let results = match search_bm25(index_dir, query, BM25_PREFILTER_LIMIT) {
        Ok(r) => r,
        Err(e) => {
            debug!(error = %e, "BM25 pre-filter failed, continuing without context");
            return Vec::new();
        }
    };

    let mut seen = HashSet::new();
    let mut hints = Vec::new();

    for r in &results {
        if let Some(ref sym) = r.symbol_name {
            let hint = format!("symbol: {sym}");
            if seen.insert(hint.clone()) {
                hints.push(hint);
            }
        }
        let hint = format!("file: {}", r.file_path);
        if seen.insert(hint.clone()) {
            hints.push(hint);
        }
        if hints.len() >= MAX_CONTEXT_HINTS {
            break;
        }
    }

    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn prepared_expansion_is_reused_across_indexes_without_a_completion_client() {
        let scratch =
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        let config = VeraConfig::default();
        let filters = SearchFilters::default();
        let mut repos = Vec::new();
        for name in ["primary_marker", "extra_marker"] {
            let repo = tempfile::tempdir_in(&scratch).unwrap();
            std::fs::write(
                repo.path().join("lib.rs"),
                format!("pub fn shared_marker() {{ {name}(); }}\n"),
            )
            .unwrap();
            crate::indexing::pipeline::index_repository_with_progress_and_cancellation(
                repo.path(),
                None::<&crate::embedding::DynamicProvider>,
                &config,
                "",
                |_| {},
                &crate::CancellationToken::new(),
            )
            .await
            .unwrap();
            repos.push(repo);
        }
        // Expansion is prepared outside the per-index loop. The executor accepts
        // only this reusable plan, not a completion client or expansion callback.
        let plan = DeepSearchPlan {
            query: "shared_marker".into(),
            expanded: Some(dedupe_queries_with_original(
                "shared_marker",
                vec!["marker".into()],
            )),
        };
        let context = SearchContext::bm25_only();
        let dirs: Vec<_> = repos
            .iter()
            .map(|repo| crate::indexing::index_dir(repo.path()))
            .collect();
        let outcomes = futures::future::join_all(dirs.iter().map(|dir| {
            execute_prepared_deep_search(&context, dir, &plan, None, &config, &filters, 5)
        }))
        .await;
        for (outcome, marker) in outcomes.into_iter().zip(["primary_marker", "extra_marker"]) {
            let (results, _) = outcome.unwrap();
            assert!(results.iter().any(|result| result.content.contains(marker)));
        }
        // An index with no hits returns an empty list, so a cross-index search
        // keeps the other indexes' results instead of failing.
        let empty = DeepSearchPlan {
            query: "absent_token".into(),
            expanded: Some(vec!["absent_token".into(), "missing_token".into()]),
        };
        let (results, _) =
            execute_prepared_deep_search(&context, &dirs[0], &empty, None, &config, &filters, 5)
                .await
                .unwrap();
        assert!(results.is_empty());
        assert_eq!(plan.expanded.unwrap(), ["shared_marker", "marker"]);
    }

    #[test]
    fn dedupe_preserves_original_first() {
        let queries = dedupe_queries_with_original(
            "auth token refresh",
            vec![
                "jwt expiry handling".to_string(),
                "auth middleware".to_string(),
                "AUTH TOKEN REFRESH".to_string(),
            ],
        );
        assert_eq!(
            queries,
            vec![
                "auth token refresh",
                "jwt expiry handling",
                "auth middleware"
            ]
        );
    }

    #[test]
    fn per_query_limit_overfetches() {
        assert_eq!(multi_query_candidate_limit(5), 20);
        assert_eq!(multi_query_candidate_limit(20), 40);
    }

    #[test]
    fn merge_timings_sums() {
        let mut target = SearchTimings::default();
        let incoming = SearchTimings {
            rerank_outcome: super::super::RerankOutcome::Reranked,
            semantic_fallback: Some("vector search failed".to_string()),
            index_open: Some(Duration::from_millis(5)),
            embedding: Some(Duration::from_millis(10)),
            bm25: Some(Duration::from_millis(20)),
            vector: Some(Duration::from_millis(30)),
            fusion: Some(Duration::from_millis(40)),
            reranking: Some(Duration::from_millis(50)),
            augmentation: Some(Duration::from_millis(60)),
            total: None,
        };
        target.merge(&incoming);
        target.merge(&SearchTimings::default());
        target.merge(&incoming);
        assert_eq!(
            target.semantic_fallback.as_deref(),
            Some("vector search failed")
        );
        assert_eq!(target.embedding, Some(Duration::from_millis(20)));
        assert_eq!(target.bm25, Some(Duration::from_millis(40)));
        assert_eq!(target.rerank_outcome, super::super::RerankOutcome::Reranked);
    }
}
