//! Additive priors in their established signal order.

use crate::chunk_text::file_name;
use crate::config::RetrievalConfig;
use crate::corpus::{ContentClass, classify_content};
use crate::retrieval::query_classifier::QueryType;
use crate::retrieval::query_utils::{file_stem, path_depth};
use crate::types::{SearchFilters, SearchResult, SymbolType};

use super::RankingStage;
use super::content::{
    COVERAGE_EXPONENT, content_defines_query_keyword, coverage_ratio, coverage_weight,
    is_definition_symbol, is_public_symbol, is_reexport_barrel, looks_like_impl_block,
    prefers_source_over_docs, prefers_structural_chunks, structural_chunk_bias,
    symbol_keyword_bonus,
};
use super::path::{
    file_stem_prefix_matches_identifier, identifier_matches_parent_dir, is_compat_path,
    is_internal_definition_path, is_typescript_declaration, path_matches_fragment,
    version_path_bonus,
};
use super::query::{QueryFeatures, identifier_stems, normalize_token, shares_keyword_stem};

pub(super) fn score_prior_with_config(
    features: &QueryFeatures,
    result: &SearchResult,
    stage: RankingStage,
    filters: &SearchFilters,
    retrieval_config: &RetrievalConfig,
) -> f64 {
    let stage_weight = match stage {
        RankingStage::Initial => 1.0,
        RankingStage::PostRerank => 0.55,
    };
    let depth = path_depth(&result.file_path) as f64;
    let role = classify_content(&result.file_path, result.language, &result.content);
    let mut bonus = 0.0;
    let file_path = result.file_path.to_ascii_lowercase();
    let result_filename = file_name(&result.file_path).to_ascii_lowercase();
    let allow_filename_semantic_bonus = matches!(
        role,
        ContentClass::Source | ContentClass::Config | ContentClass::Unknown
    );
    let path_fragment_match = features
        .path_fragment
        .as_deref()
        .is_some_and(|fragment| path_matches_fragment(&file_path, fragment));
    let filename_boost_allowed = features.path_fragment.is_none() || path_fragment_match;

    if path_fragment_match {
        bonus += stage_weight * 1.2;
    }

    if let Some(filename) = features.exact_filename.as_deref() {
        if filename_boost_allowed && result_filename == filename {
            let filename_bonus = if features.wants_config_paths {
                if depth == 0.0 {
                    1.15
                } else {
                    (0.45 - depth.min(5.0) * 0.08).max(0.08)
                }
            } else if depth == 0.0 {
                0.9
            } else {
                (0.6 - depth.min(5.0) * 0.06).max(0.12)
            };
            bonus += stage_weight * filename_bonus;
        } else if filename_boost_allowed && file_path.ends_with(filename) {
            bonus += stage_weight * 0.15;
        }
    }

    if features.wants_config_paths && matches!(role, ContentClass::Config) {
        bonus += stage_weight
            * if depth == 0.0 {
                0.35
            } else {
                (0.2 - depth.min(5.0) * 0.03).max(0.05)
            };
    }

    if let Some(identifier) = features.exact_identifier.as_deref() {
        if result
            .symbol_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(identifier))
        {
            // Symbol name matches the query identifier. This is the strongest
            // signal: developers searching "Axios" want the Axios class definition.
            let is_definition_chunk = is_definition_symbol(result.symbol_type);
            let stem_aligns = file_stem(&result_filename).eq_ignore_ascii_case(identifier);
            let base_symbol_bonus = if features.query_word_count <= 2 {
                if is_definition_chunk { 1.6 } else { 0.7 }
            } else if is_definition_chunk {
                1.2
            } else {
                0.55
            };
            bonus += stage_weight * base_symbol_bonus;
            bonus += stage_weight * if depth <= 2.0 { 0.18 } else { 0.05 };
            if features.requested_symbol_types.contains(&SymbolType::Class)
                && is_internal_definition_path(&file_path)
            {
                bonus -= stage_weight * 0.35;
            }
            if features
                .exact_identifier_case
                .as_deref()
                .is_some_and(|name| result.symbol_name.as_deref() == Some(name))
            {
                bonus += stage_weight * 0.28;
            }
            // Extra boost when the file stem also matches (e.g., Axios in Axios.js).
            if stem_aligns {
                bonus += stage_weight * 0.45;
            }
        } else if file_stem(&result_filename).eq_ignore_ascii_case(identifier) {
            bonus += stage_weight * 0.35;
        } else if file_stem_prefix_matches_identifier(file_stem(&result_filename), identifier) {
            bonus += stage_weight * 0.28;
        } else if identifier_matches_parent_dir(identifier, &file_path) {
            bonus += stage_weight * 0.22;
        }
    }

    if features.query_type == QueryType::NaturalLanguage
        && !features.keywords.is_empty()
        && !features.wants_config_paths
        && allow_filename_semantic_bonus
        && let Some(symbol_name) = result.symbol_name.as_deref()
    {
        let symbol_bonus = symbol_keyword_bonus(symbol_name, &features.keywords);
        if symbol_bonus > 0.0 {
            bonus += stage_weight * symbol_bonus;
        }
    }

    if !features.requested_symbol_types.is_empty()
        && result
            .symbol_type
            .is_some_and(|sym| features.requested_symbol_types.contains(&sym))
    {
        bonus += stage_weight * 0.62;
        if features
            .exact_identifier_case
            .as_deref()
            .is_some_and(|name| result.symbol_name.as_deref() == Some(name))
        {
            bonus += stage_weight * 0.2;
        }
    } else if !features.requested_symbol_types.is_empty() {
        bonus -= stage_weight
            * if features.exact_identifier_case.is_some() {
                0.9
            } else {
                0.55
            };
    }

    if features.mentions_definition && is_definition_symbol(result.symbol_type) {
        bonus += stage_weight
            * if result.symbol_name.is_some() {
                0.34
            } else {
                0.18
            };
    }

    // Boost definition chunks for NL queries when their symbol name overlaps
    // query keywords. Definitions are the canonical location for a concept;
    // they should strongly outrank incidental mentions. Use a weaker boost
    // for broad multi-keyword queries where the symbol match is partial.
    // Gated by ranking_definition_boost (issue #196 signal).
    if retrieval_config.ranking_definition_boost_enabled()
        && features.query_type == QueryType::NaturalLanguage
        && is_definition_symbol(result.symbol_type)
        && result.symbol_name.is_some()
        && let Some(symbol_name) = result.symbol_name.as_deref()
    {
        let sym_stems = identifier_stems(symbol_name);
        // Count keyword overlaps where the keyword is non-trivial (5+ chars)
        // to avoid short keywords like "file", "type", "list" causing false boosts.
        let overlap_count = features
            .keywords
            .iter()
            .filter(|kw| {
                kw.len() >= 5
                    && sym_stems
                        .iter()
                        .any(|s| s == kw.as_str() || shares_keyword_stem(s, kw))
            })
            .count();
        if overlap_count > 0 {
            // Scale by overlap ratio: single keyword match in a 5-word query
            // gets a modest boost; full overlap gets the maximum.
            let long_keywords = features
                .keywords
                .iter()
                .filter(|k| k.len() >= 5)
                .count()
                .max(1);
            let ratio = (overlap_count as f64 / long_keywords as f64).min(1.0);

            // Extra boost when the file stem also matches the symbol.
            let stem = file_stem(&result_filename);
            let stem_aligns = file_stem(&result_filename).eq_ignore_ascii_case(symbol_name)
                || sym_stems.iter().any(|s| {
                    s == &normalize_token(stem) || shares_keyword_stem(s, &normalize_token(stem))
                });
            let base_boost = if stem_aligns { 1.5 } else { 1.0 };
            bonus += stage_weight * base_boost * ratio;
        }
    }

    // Content-based definition detection: if the chunk's content defines
    // a symbol matching query keywords (via language-agnostic prefix matching),
    // boost it. This catches cases where symbol_type metadata is missing
    // or too coarse. Skip when the user wants non-source content.
    // Use a mild boost; the metadata-based definition boost above handles
    // strong signals. Gated by ranking_definition_boost.
    if retrieval_config.ranking_definition_boost_enabled()
        && features.query_type == QueryType::NaturalLanguage
        && !features.keywords.is_empty()
        && !features.wants_runtime_paths
        && !features.wants_config_paths
        && content_defines_query_keyword(&result.content, &features.keywords)
    {
        bonus += stage_weight * 0.3;
    }

    // Content keyword coverage: multi-concept NL questions are answered by
    // the chunk that mentions every concept, and BM25's frequency saturation
    // can bury such a chunk under single-concept keyword-dense noise. Reward
    // the fraction of distinct query keywords the chunk content covers.
    // Explicit config-path requests use path intent instead of content
    // coverage.
    if !features.wants_config_paths
        && let Some(ratio) = coverage_ratio(features, &result.content)
    {
        bonus += stage_weight * coverage_weight(features) * ratio.powf(COVERAGE_EXPONENT);
    }

    // --- Noise penalties ---
    if !features.wants_test_paths && matches!(role, ContentClass::Test) {
        bonus -= stage_weight * 0.95;
    }
    if matches!(role, ContentClass::Archive) {
        if features.wants_archive_paths {
            bonus += stage_weight * 0.18;
        } else {
            bonus -= stage_weight * 0.85;
        }
    }
    if matches!(role, ContentClass::Runtime) {
        if features.wants_runtime_paths {
            bonus += stage_weight * 0.95;
        } else {
            bonus -= stage_weight * 0.72;
        }
    } else if features.wants_runtime_paths {
        bonus -= stage_weight * 0.24;
    }
    if !features.wants_docs_paths && matches!(role, ContentClass::Docs) {
        bonus -= stage_weight
            * if prefers_source_over_docs(features) {
                0.95
            } else {
                0.55
            };
    }
    if !features.wants_example_paths && matches!(role, ContentClass::Example | ContentClass::Bench)
    {
        bonus -= stage_weight * 0.55;
    }
    if !features.wants_compat_paths && is_compat_path(&file_path) {
        bonus -= stage_weight * 0.65;
    } else if features.wants_compat_paths && is_compat_path(&file_path) {
        bonus += stage_weight * 0.32;
    }
    if !features.wants_type_declarations && is_typescript_declaration(&file_path) {
        bonus -= stage_weight * 0.82;
    }
    if is_reexport_barrel(result) && !features.mentions_definition {
        bonus -= stage_weight * 0.95;
    }
    bonus += stage_weight * version_path_bonus(features, &file_path);
    if matches!(role, ContentClass::Generated) {
        bonus -= stage_weight
            * if features.wants_runtime_paths {
                0.18
            } else {
                0.95
            };
        if filters.include_generated == Some(false) {
            bonus -= stage_weight * 0.8;
        }
    }
    if matches!(role, ContentClass::Source | ContentClass::Config) {
        bonus += stage_weight
            * if features.query_type == QueryType::Identifier || features.path_fragment.is_some() {
                if depth <= 2.0 { 0.24 } else { 0.12 }
            } else if depth <= 2.0 {
                0.12
            } else {
                0.05
            };
    }
    if let Some(scope) = filters.scope {
        if crate::corpus::matches_scope(role, scope, filters.include_generated.unwrap_or(true)) {
            bonus += stage_weight * 0.18;
        } else {
            bonus -= stage_weight * 1.1;
        }
    }

    if features.mentions_implementation && looks_like_impl_block(result) {
        bonus += stage_weight * 0.18;
    }

    if features.query_type == QueryType::NaturalLanguage && is_public_symbol(result) {
        bonus += stage_weight * 0.05;
    }

    if prefers_structural_chunks(features) {
        bonus += stage_weight * structural_chunk_bias(result);
    }

    bonus
}
