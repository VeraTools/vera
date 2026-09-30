//! Pool coherence and file saturation for multi-file results.

use crate::retrieval::query_classifier::QueryType;
use crate::types::SearchResult;
use std::collections::HashMap;

use super::query::QueryFeatures;

// Ranking signal weights and thresholds for deterministic result shaping.
/// Per-file score-sum coherence, boosting only the file's best chunk.
pub(super) const COHERENCE_WEIGHT: f64 = 0.4;

/// Coherence weight for natural-language queries.
pub(super) const COHERENCE_WEIGHT_NL: f64 = 0.4;

/// Maximum chunks from the same file before saturation decay kicks in.
pub(super) const FILE_SATURATION_THRESHOLD: usize = 1;

/// Multiplicative penalty per extra chunk from the same file beyond the threshold.
/// 0.35 means each successive same-file chunk keeps 35% of its score, pushing
/// it below results from other files in most cases.
pub(super) const FILE_SATURATION_DECAY: f64 = 0.35;

/// File coherence: files whose chunks collectively score well are
/// likely the file the user needs. Sum each file's (clamped) combined scores
/// and boost only the file's best chunk, proportional to the file's share of
/// the strongest file. Boosting every chunk (count-based) lets one file flood
/// the window; boosting only the best surfaces the cluster without the flood.
pub(super) fn apply_coherence_boost(
    features: &QueryFeatures,
    scores: &mut [f64],
    results: &[SearchResult],
    max_score: f64,
) {
    // Identifier queries benefit from a stronger coherence weight; for
    // natural-language questions it costs intent ordering.
    let weight = if features.query_type == QueryType::NaturalLanguage {
        COHERENCE_WEIGHT_NL
    } else {
        COHERENCE_WEIGHT
    };
    let updates: Vec<(usize, f64)> = {
        let mut file_sum: HashMap<&str, f64> = HashMap::new();
        for (score, result) in scores.iter().zip(results) {
            *file_sum.entry(result.file_path.as_str()).or_default() += score.max(0.0);
        }
        let max_file_sum = file_sum.values().copied().fold(0.0_f64, f64::max).max(1e-6);

        let mut best_chunk: HashMap<&str, usize> = HashMap::new();
        for (i, (score, result)) in scores.iter().zip(results).enumerate() {
            best_chunk
                .entry(result.file_path.as_str())
                .and_modify(|j| {
                    if *score > scores[*j] {
                        *j = i;
                    }
                })
                .or_insert(i);
        }

        best_chunk
            .into_iter()
            .map(|(path, i)| {
                let boost = weight
                    * max_score
                    * (file_sum.get(path).copied().unwrap_or(0.0) / max_file_sum);
                (i, boost)
            })
            .collect()
    };

    for (i, boost) in updates {
        scores[i] += boost;
    }
}

pub(super) fn diversify_by_file(results: Vec<SearchResult>) -> Vec<SearchResult> {
    if results.len() <= 1 {
        return results;
    }

    let mut file_counts: HashMap<String, usize> = HashMap::new();
    let mut scored: Vec<(f64, usize, SearchResult)> = results
        .into_iter()
        .enumerate()
        .map(|(idx, result)| {
            let count = file_counts.entry(result.file_path.clone()).or_insert(0);
            *count += 1;
            let effective_score = if *count > FILE_SATURATION_THRESHOLD {
                let excess = (*count - FILE_SATURATION_THRESHOLD) as f64;
                result.score * FILE_SATURATION_DECAY.powf(excess)
            } else {
                result.score
            };
            (effective_score, idx, result)
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });

    scored.into_iter().map(|(_, _, result)| result).collect()
}
