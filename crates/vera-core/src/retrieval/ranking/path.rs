//! Filename, directory, and explicit path signals.

use crate::chunk_text::file_name;
use crate::config::RetrievalConfig;
use crate::corpus::{ContentClass, classify_content, classify_path, content_class_label};
use crate::retrieval::query_classifier::QueryType;
use crate::retrieval::query_utils::file_stem;
use crate::types::{Language, SearchResult};
use std::collections::HashMap;

use super::query::{QueryFeatures, common_prefix_len, identifier_stems};

/// Pool-relative filename and parent-directory keyword boost scale.
pub(super) const KEYWORD_PATH_WEIGHT: f64 = 1.0;

/// Pool-relative keyword path boost: when query keywords match a file's stem
/// or its immediate parent directory (exact or prefix, 3+ chars), every chunk
/// of that file gains `max_score * match_ratio`. Scaling by the pool's best
/// score keeps the signal proportional to retrieval confidence, and the
/// match-ratio form rewards files named after the whole query over files
/// matching a single incidental keyword.
pub(super) fn apply_keyword_path_boost(
    features: &QueryFeatures,
    scores: &mut [f64],
    results: &[SearchResult],
    max_score: f64,
    retrieval: &RetrievalConfig,
) {
    // Explicit runtime intent overrides filename keyword inference: a user
    // asking for the runtime extract does not want the like-named source file
    // boosted past it. (Other intent flags are not gated: e.g. "compat" is
    // both a content-class flag and a legitimate path keyword, and the
    // role gate below already excludes non-source files from the boost.)
    if features.query_type != QueryType::NaturalLanguage
        || features.wants_config_paths
        || features.wants_runtime_paths
    {
        return;
    }
    // Gating knob: skip symbol queries when the exact-identifier machinery
    // is engaged. This suppresses the filename inference for symbol lookups
    // which already have dedicated boosts.
    if retrieval.ranking_filename_stem_skip_symbol_queries_enabled()
        && (features.exact_identifier.is_some() || !features.embedded_symbols.is_empty())
    {
        return;
    }
    if features.keywords.is_empty() {
        return;
    }
    let keywords: Vec<&str> = features
        .keywords
        .iter()
        .map(String::as_str)
        .filter(|kw| kw.len() > 2)
        .collect();
    if keywords.is_empty() {
        return;
    }

    let min_ratio = retrieval.ranking_filename_stem_min_ratio_effective();
    let mut bonuses = Vec::with_capacity(results.len());
    {
        let mut bonus_cache: HashMap<&str, f64> = HashMap::new();
        for result in results {
            let bonus = *bonus_cache
                .entry(result.file_path.as_str())
                .or_insert_with(|| {
                    let role =
                        classify_content(&result.file_path, result.language, &result.content);
                    if !matches!(
                        role,
                        ContentClass::Source | ContentClass::Config | ContentClass::Unknown
                    ) {
                        return 0.0;
                    }
                    let ratio = keyword_path_match_ratio(&keywords, &result.file_path);
                    if ratio >= min_ratio {
                        KEYWORD_PATH_WEIGHT * max_score * ratio
                    } else {
                        0.0
                    }
                });
            bonuses.push(bonus);
        }
    }
    for (score, bonus) in scores.iter_mut().zip(bonuses) {
        *score += bonus;
    }
}

/// Fraction of query keywords that match the file stem's or immediate parent
/// directory's sub-tokens (exact, or prefix with a 3-char minimum).
pub(crate) fn keyword_path_match_ratio(keywords: &[&str], file_path: &str) -> f64 {
    if keywords.is_empty() {
        return 0.0;
    }
    let lowered = file_path.to_ascii_lowercase();
    let stem = file_stem(file_name(&lowered));
    let mut parts = identifier_stems(stem);
    if let Some((dirs, _)) = lowered.rsplit_once('/')
        && let Some(parent) = dirs.rsplit('/').next()
    {
        parts.extend(identifier_stems(parent));
    }
    if parts.is_empty() {
        return 0.0;
    }

    let matched = keywords
        .iter()
        .filter(|kw| {
            parts.iter().any(|part| {
                part == *kw
                    || (kw.len() >= 3 && part.starts_with(*kw))
                    || (part.len() >= 3 && kw.starts_with(part.as_str()))
            })
        })
        .count();

    (matched as f64 / keywords.len() as f64).min(1.0)
}

pub(super) fn tokenize_path(path: &str) -> Vec<&str> {
    path.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect()
}

pub(super) fn contains_token(tokens: &[&str], expected: &[&str]) -> bool {
    tokens.iter().any(|token| expected.contains(token))
}

pub(super) fn is_internal_definition_path(path: &str) -> bool {
    let tokens = tokenize_path(path);
    contains_token(&tokens, &["sansio", "internal", "bindings"])
}

pub(super) fn path_matches_fragment(path: &str, fragment: &str) -> bool {
    path == fragment || path.ends_with(fragment) || path.contains(fragment)
}

/// Check if a file stem shares a 6+ char prefix with an identifier.
/// Strips namespace prefixes (e.g. "sinatra::showexceptions" → "showexceptions")
/// so that "format" matches "formatter" but "sinatra" doesn't match "sinatra::ShowExceptions".
pub(super) fn file_stem_prefix_matches_identifier(stem: &str, identifier: &str) -> bool {
    let stem_lower = stem.to_ascii_lowercase();
    let ident_lower = identifier.to_ascii_lowercase();
    let bare_ident = ident_lower
        .rsplit_once("::")
        .map(|(_, name)| name)
        .unwrap_or(&ident_lower);
    common_prefix_len(&stem_lower, bare_ident) >= 6
}

pub(super) fn identifier_matches_parent_dir(identifier: &str, path: &str) -> bool {
    parent_dir_stems(path)
        .iter()
        .any(|stem| stem.eq_ignore_ascii_case(identifier))
}

pub(super) fn parent_dir_stems(path: &str) -> Vec<String> {
    let Some((dirs, _)) = path.rsplit_once('/') else {
        return Vec::new();
    };
    dirs.split('/')
        .rev()
        .take(3)
        .flat_map(identifier_stems)
        .collect()
}

pub(super) fn is_compat_path(path: &str) -> bool {
    let tokens = tokenize_path(path);
    contains_token(
        &tokens,
        &[
            "compat",
            "compatibility",
            "legacy",
            "shim",
            "shims",
            "polyfill",
            "polyfills",
        ],
    )
}

pub(super) fn is_typescript_declaration(path: &str) -> bool {
    path.ends_with(".d.ts") || path.ends_with(".d.mts") || path.ends_with(".d.cts")
}

pub(super) fn version_path_bonus(features: &QueryFeatures, path: &str) -> f64 {
    if features.requested_versions.is_empty() {
        return 0.0;
    }

    let tokens = tokenize_path(path);
    if tokens.iter().any(|token| {
        features
            .requested_versions
            .iter()
            .any(|version| version == token)
    }) {
        return 0.55;
    }

    if tokens.iter().any(|token| {
        token.len() >= 2
            && token.starts_with('v')
            && token[1..].chars().all(|ch| ch.is_ascii_digit())
    }) {
        return -0.34;
    }

    -0.08
}

pub(crate) fn file_role_label(file_path: &str, language: Language) -> &'static str {
    content_class_label(classify_path(file_path, language))
}
