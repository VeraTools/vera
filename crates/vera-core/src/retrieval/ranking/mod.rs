//! Query-aware ranking heuristics layered on top of dense + lexical retrieval.
//!
//! These heuristics intentionally stay simple and deterministic. They target
//! recurring benchmark failures that single-vector retrieval struggles with:
//! config files at repo root, test/docs noise, symbol-type disambiguation, and
//! same-file crowding for multi-file questions.

use crate::config::VeraConfig;
use crate::types::{SearchFilters, SearchResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RankingStage {
    Initial,
    PostRerank,
}

mod content;
mod diversification;
mod path;
pub(crate) mod query;
pub(crate) mod score;

#[cfg(test)]
mod tests;

pub(crate) use path::file_role_label;
pub(crate) use query::is_path_weighted_query;

use content::apply_content_symbol_boost;
use diversification::{apply_coherence_boost, diversify_by_file};
use path::apply_keyword_path_boost;
use query::QueryFeatures;
use score::score_prior_with_config;

#[cfg(test)]
pub(crate) fn apply_query_ranking(
    query: &str,
    results: Vec<SearchResult>,
    stage: RankingStage,
) -> Vec<SearchResult> {
    apply_query_ranking_with_filters(query, results, stage, &SearchFilters::default())
}

pub(crate) fn apply_query_ranking_with_filters(
    query: &str,
    results: Vec<SearchResult>,
    stage: RankingStage,
    filters: &SearchFilters,
) -> Vec<SearchResult> {
    apply_query_ranking_with_filters_and_config(
        query,
        results,
        stage,
        filters,
        &VeraConfig::default(),
    )
}

pub(crate) fn apply_query_ranking_with_filters_and_config(
    query: &str,
    results: Vec<SearchResult>,
    stage: RankingStage,
    filters: &SearchFilters,
    config: &VeraConfig,
) -> Vec<SearchResult> {
    if results.len() <= 1 {
        return results;
    }

    let features = QueryFeatures::from_query(query);
    let wants_diversity = features.wants_multi_file_diversity;
    let scores = score_pool_with_config(&features, stage, filters, &results, config);
    finish_ranking(results, scores, wants_diversity)
}

/// Score under each subquery's features and keep each result's best score.
/// Joining queries would promote only the first target and crowd out the rest.
pub(crate) fn apply_query_ranking_multi_query_with_config(
    queries: &[String],
    results: Vec<SearchResult>,
    stage: RankingStage,
    filters: &SearchFilters,
    config: &VeraConfig,
) -> Vec<SearchResult> {
    if queries.is_empty() || results.len() <= 1 {
        return results;
    }

    let mut scores = vec![f64::NEG_INFINITY; results.len()];
    let mut wants_diversity = false;
    for query in queries {
        let features = QueryFeatures::from_query(query);
        wants_diversity |= features.wants_multi_file_diversity;
        for (best, score) in scores.iter_mut().zip(score_pool_with_config(
            &features, stage, filters, &results, config,
        )) {
            *best = best.max(score);
        }
    }
    finish_ranking(results, scores, wants_diversity)
}

/// Combine retrieval position and additive priors, then apply pool-relative
/// boosts in their established order so signal strength tracks confidence.
fn score_pool_with_config(
    features: &QueryFeatures,
    stage: RankingStage,
    filters: &SearchFilters,
    results: &[SearchResult],
    config: &VeraConfig,
) -> Vec<f64> {
    let retrieval = &config.retrieval;
    let len = results.len() as f64;
    let mut scores: Vec<f64> = results
        .iter()
        .enumerate()
        .map(|(idx, result)| {
            let base_rank = 1.0 - (idx as f64 / len);
            let prior = score_prior_with_config(features, result, stage, filters, retrieval);
            base_rank + prior
        })
        .collect();

    let max_score = scores.iter().copied().fold(0.0_f64, f64::max).max(1e-6);
    apply_coherence_boost(features, &mut scores, results, max_score);
    if retrieval.ranking_filename_stem_boost_enabled() {
        apply_keyword_path_boost(features, &mut scores, results, max_score, retrieval);
    }
    if retrieval.ranking_definition_boost_enabled() {
        apply_content_symbol_boost(features, &mut scores, results, max_score);
    }

    scores
}

fn finish_ranking(
    results: Vec<SearchResult>,
    scores: Vec<f64>,
    wants_diversity: bool,
) -> Vec<SearchResult> {
    let mut scored: Vec<(f64, usize, SearchResult)> = results
        .into_iter()
        .enumerate()
        .zip(scores)
        .map(|((idx, mut result), score)| {
            result.score = score;
            (score, idx, result)
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });

    let reranked = scored.into_iter().map(|(_, _, result)| result).collect();
    let reranked = if wants_diversity {
        diversify_by_file(reranked)
    } else {
        reranked
    };
    stamp_rank_scores(reranked)
}

fn stamp_rank_scores(mut results: Vec<SearchResult>) -> Vec<SearchResult> {
    let len = results.len().max(1) as f64;
    for (idx, result) in results.iter_mut().enumerate() {
        result.score = 1.0 - (idx as f64 / len);
    }
    results
}
