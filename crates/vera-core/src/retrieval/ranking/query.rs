//! Query feature extraction for ranking heuristics.

use crate::chunk_text::file_name;
use crate::retrieval::query_classifier::{QueryType, classify_query};
use crate::retrieval::query_utils::{
    looks_like_compound_identifier, looks_like_filename, trim_query_token,
};
use crate::types::SymbolType;

/// Stopwords excluded from the raw content-word set.
const QUERY_STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was", "one",
    "our", "out", "has", "have", "been", "were", "they", "this", "that", "with", "from", "what",
    "when", "where", "how", "why", "which", "does",
];

#[derive(Debug, Clone)]
pub(super) struct QueryFeatures {
    pub(super) query_word_count: usize,
    pub(super) path_fragment: Option<String>,
    pub(super) exact_filename: Option<String>,
    pub(super) exact_identifier_case: Option<String>,
    pub(super) exact_identifier: Option<String>,
    pub(super) keywords: Vec<String>,
    /// Raw content words of the query: tokens longer than 2 chars, minus a
    /// small stopword list. No singularization or curated exclusions.
    pub(super) raw_keywords: Vec<String>,
    /// CamelCase/snake_case identifiers embedded in NL queries.
    /// E.g., "How does StateManager handle transitions" yields ["StateManager"].
    pub(super) embedded_symbols: Vec<String>,
    pub(super) requested_symbol_types: Vec<SymbolType>,
    pub(super) query_type: QueryType,
    pub(super) wants_test_paths: bool,
    pub(super) wants_docs_paths: bool,
    pub(super) wants_example_paths: bool,
    pub(super) wants_config_paths: bool,
    pub(super) wants_runtime_paths: bool,
    pub(super) wants_archive_paths: bool,
    pub(super) wants_compat_paths: bool,
    pub(super) wants_type_declarations: bool,
    pub(super) requested_versions: Vec<String>,
    pub(super) wants_multi_file_diversity: bool,
    pub(super) mentions_implementation: bool,
    pub(super) mentions_definition: bool,
    /// Query joins facets with an explicit conjunction ("A and B").
    pub(super) has_conjunction: bool,
}

impl QueryFeatures {
    pub(super) fn from_query(query: &str) -> Self {
        let lower = query.trim().to_ascii_lowercase();
        let query_type = classify_query(query);
        let raw_tokens: Vec<&str> = query.split_whitespace().collect();
        let cleaned_tokens: Vec<String> = query
            .split_whitespace()
            .map(clean_query_token)
            .filter(|token| !token.is_empty())
            .collect();
        let path_fragment = cleaned_tokens
            .iter()
            .find(|token| looks_like_path_fragment(token))
            .cloned();
        let exact_filename = cleaned_tokens
            .iter()
            .find(|token| looks_like_filename(token) && !looks_like_qualified_identifier(token))
            .map(|token| file_name(token).to_string());
        let exact_identifier = raw_tokens
            .iter()
            .map(|token| trim_query_token(token))
            .find(|token| {
                !token.is_empty()
                    && (looks_like_qualified_identifier(token)
                        || (!looks_like_filename(&token.to_ascii_lowercase())
                            && looks_like_compound_identifier(token)))
            })
            .map(|token| token.to_ascii_lowercase())
            .or_else(|| {
                if query_type == QueryType::Identifier && cleaned_tokens.len() == 1 {
                    cleaned_tokens
                        .first()
                        .filter(|token| {
                            !looks_like_filename(token) || looks_like_qualified_identifier(token)
                        })
                        .cloned()
                } else {
                    None
                }
            });
        let exact_identifier_case = exact_identifier.as_ref().and_then(|_| {
            raw_tokens
                .iter()
                .map(|token| trim_query_token(token))
                .find(|token| {
                    !token.is_empty()
                        && (looks_like_qualified_identifier(token)
                            || (!looks_like_filename(&token.to_ascii_lowercase())
                                && looks_like_compound_identifier(token)))
                })
                .map(ToString::to_string)
        });
        let keywords = cleaned_tokens
            .iter()
            .filter(|token| {
                (!looks_like_filename(token) || looks_like_qualified_identifier(token))
                    && !is_query_stopword(token)
            })
            .map(|token| normalize_token(token))
            .filter(|token| !token.is_empty())
            .collect();
        let raw_keywords: Vec<String> = raw_tokens
            .iter()
            .map(|token| trim_query_token(token).to_ascii_lowercase())
            .filter(|word| word.len() > 2 && !QUERY_STOPWORDS.contains(&word.as_str()))
            .collect();
        let requested_symbol_types = requested_symbol_types(&lower);

        // Extract CamelCase/snake_case identifiers embedded in NL queries.
        // "How does StateManager handle transitions" → ["statemanager"]
        let embedded_symbols = if query_type == QueryType::NaturalLanguage {
            extract_embedded_symbols(&raw_tokens, exact_identifier.as_deref())
        } else {
            Vec::new()
        };

        Self {
            query_word_count: raw_tokens.len(),
            path_fragment,
            exact_identifier_case,
            raw_keywords,
            wants_test_paths: mentions_any(&lower, &["test", "tests", "spec", "__tests__"]),
            wants_docs_paths: mentions_any(&lower, &["docs", "documentation", "readme"]),
            wants_example_paths: mentions_any(&lower, &["example", "examples", "demo", "sample"]),
            wants_config_paths: is_path_weighted_query(query)
                || mentions_any(
                    &lower,
                    &["configuration", "config", "workspace", "settings"],
                ),
            // "runtime" alone is ambiguous: intent queries discuss runtime
            // *behavior* ("catching runtime panics", "the async runtime")
            // far more often than runtime artifacts. Only unambiguous
            // artifact vocabulary triggers the runtime-extract preference.
            wants_runtime_paths: mentions_any(
                &lower,
                &[
                    "bundle",
                    "bundles",
                    "minified",
                    "minify",
                    "extract",
                    "extracted",
                    "asar",
                    "dist",
                ],
            ),
            wants_archive_paths: mentions_any(
                &lower,
                &["archive", "archived", "legacy", "snapshot", "deprecated"],
            ),
            wants_compat_paths: mentions_any(
                &lower,
                &[
                    "compat",
                    "compatibility",
                    "legacy",
                    "shim",
                    "polyfill",
                    "adapter",
                ],
            ),
            wants_type_declarations: mentions_any(
                &lower,
                &["declaration", "declarations", ".d.ts", "types", "typings"],
            ),
            requested_versions: requested_versions(&cleaned_tokens),
            wants_multi_file_diversity: !is_path_weighted_query(query)
                && (query_type == QueryType::NaturalLanguage
                    || (exact_identifier.is_some() && raw_tokens.len() <= 2)),
            has_conjunction: raw_tokens.iter().any(|token| {
                let t = trim_query_token(token);
                t.eq_ignore_ascii_case("and") || t.eq_ignore_ascii_case("or")
            }),
            mentions_implementation: mentions_any(
                &lower,
                &[
                    "implementation",
                    "implementations",
                    "impl",
                    "mounted",
                    "registration",
                ],
            ),
            mentions_definition: mentions_any(
                &lower,
                &[
                    "definition",
                    "definitions",
                    "define",
                    "declared",
                    "declaration",
                ],
            ),
            exact_filename,
            exact_identifier,
            keywords,
            embedded_symbols,
            requested_symbol_types,
            query_type,
        }
    }
}

/// Check whether a token is a scope-qualified identifier rather than a file
/// name whose extension happens to contain a dot.
pub(crate) fn looks_like_qualified_identifier(token: &str) -> bool {
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    };

    if token.contains("::") {
        return token
            .split("::")
            .all(|scope| scope.split('.').all(&valid_segment));
    }

    let segments: Vec<_> = token.split('.').collect();
    segments.len() >= 3 && segments.iter().all(|segment| valid_segment(segment))
}

pub(super) fn requested_symbol_types(query: &str) -> Vec<SymbolType> {
    let mut symbol_types = Vec::new();
    if query.contains("trait") {
        symbol_types.push(SymbolType::Trait);
    }
    if query.contains("class") {
        symbol_types.push(SymbolType::Class);
    }
    if query.contains("interface") {
        symbol_types.push(SymbolType::Interface);
    }
    if query.contains("struct") {
        symbol_types.push(SymbolType::Struct);
    }
    if query.contains("enum") {
        symbol_types.push(SymbolType::Enum);
    }
    if query.contains("function") {
        symbol_types.push(SymbolType::Function);
    }
    if query.contains("method") {
        symbol_types.push(SymbolType::Method);
    }
    symbol_types
}

pub(super) fn requested_versions(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .filter(|token| {
            token.len() >= 2
                && token.starts_with('v')
                && token[1..].chars().all(|ch| ch.is_ascii_digit())
        })
        .cloned()
        .collect()
}

pub(super) fn looks_like_path_fragment(token: &str) -> bool {
    token.contains('/') || token.contains('\\')
}

pub(super) fn clean_query_token(token: &str) -> String {
    trim_query_token(token).to_ascii_lowercase()
}

pub(super) fn mentions_any(query: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| query.contains(needle))
}

pub(super) fn is_query_stopword(token: &str) -> bool {
    matches!(
        token,
        "and"
            | "or"
            | "the"
            | "a"
            | "an"
            | "of"
            | "in"
            | "to"
            | "for"
            | "with"
            | "across"
            | "where"
            | "definition"
            | "definitions"
            | "configured"
            | "configuration"
    )
}

pub(super) fn normalize_token(token: &str) -> String {
    let token = token.to_ascii_lowercase();
    let trimmed = token.trim_end_matches('s');
    if trimmed.len() >= 3 {
        trimmed.to_string()
    } else {
        token
    }
}

pub(super) fn shares_keyword_stem(left: &str, right: &str) -> bool {
    // Use minimum 4-char prefix overlap so short stems like "route" match
    // "routing" and "depend" matches "dependency". Longer words use longer
    // thresholds to avoid false positives.
    let shorter = left.len().min(right.len());
    let threshold = if shorter <= 5 { 4 } else { 5 };
    common_prefix_len(left, right) >= threshold
}

pub(super) fn common_prefix_len(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(l, r)| l == r)
        .count()
}

pub(super) fn identifier_stems(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .flat_map(split_camel_identifier)
        .map(|part| normalize_token(&part))
        .filter(|part| !part.is_empty() && !is_query_stopword(part))
        .collect()
}

pub(super) fn split_camel_identifier(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }

    let mut parts = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = value.char_indices().collect();
    for idx in 1..chars.len() {
        let (_, prev) = chars[idx - 1];
        let (byte_idx, current) = chars[idx];
        let boundary = (prev.is_ascii_lowercase() && current.is_ascii_uppercase())
            || (prev.is_ascii_alphabetic() && current.is_ascii_digit())
            || (prev.is_ascii_digit() && current.is_ascii_alphabetic());
        if boundary {
            parts.push(value[start..byte_idx].to_ascii_lowercase());
            start = byte_idx;
        }
    }
    parts.push(value[start..].to_ascii_lowercase());
    parts
}

/// Extract CamelCase/camelCase identifiers embedded in NL queries.
///
/// "How does StateManager handle transitions" → ["statemanager"]
/// "Where is the parseConfig function" → ["parseconfig"]
///
/// These are compound identifiers that contain mixed case transitions,
/// indicating a specific code symbol the user is asking about.
pub(super) fn extract_embedded_symbols(
    raw_tokens: &[&str],
    exact_identifier: Option<&str>,
) -> Vec<String> {
    let exact_lower = exact_identifier.map(|s| s.to_ascii_lowercase());
    raw_tokens
        .iter()
        .filter_map(|token| {
            let trimmed = trim_query_token(token);
            if trimmed.len() < 4 {
                return None;
            }
            // Must have a case transition (CamelCase or camelCase).
            let has_case_transition = trimmed
                .as_bytes()
                .windows(2)
                .any(|pair| pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase());
            if !has_case_transition {
                return None;
            }
            let lower = trimmed.to_ascii_lowercase();
            // Skip if this is already the exact_identifier (already boosted separately).
            if exact_lower.as_deref() == Some(&lower) {
                return None;
            }
            Some(lower)
        })
        .collect()
}

pub(crate) fn is_path_weighted_query(query: &str) -> bool {
    let lower = query.trim().to_ascii_lowercase();
    if lower.contains(".toml")
        || lower.contains(".json")
        || lower.contains(".yaml")
        || lower.contains(".yml")
        || lower.contains(".ini")
        || lower.contains(".conf")
        || lower.contains("dockerfile")
        || lower.contains("makefile")
        || lower.contains("cmakelists.txt")
    {
        return true;
    }

    // A slash alone does not make a path query: prose like "read/write
    // request handling" must stay semantic. Require a slash-bearing token
    // that is the whole query or has path shape (prefix or file extension).
    let tokens: Vec<&str> = lower
        .split_whitespace()
        .map(crate::retrieval::query_utils::trim_query_token)
        .filter(|token| !token.is_empty())
        .collect();
    tokens
        .iter()
        .any(|token| is_path_shaped_token(token, tokens.len() == 1))
}

fn is_path_shaped_token(token: &str, single_token_query: bool) -> bool {
    if !token.contains('/') && !token.contains('\\') {
        return false;
    }
    if single_token_query
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.starts_with('~')
        || token.as_bytes().get(1) == Some(&b':')
    // Windows drive prefix
    {
        return true;
    }
    // src/main.rs: the last path segment carries a file extension.
    let last_segment = token.rsplit(['/', '\\']).next().unwrap_or(token);
    last_segment.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_scope_qualified_identifiers_as_exact_symbols() {
        let features = QueryFeatures::from_query("std::io::Error");

        assert_eq!(features.exact_filename, None);
        assert_eq!(features.exact_identifier.as_deref(), Some("std::io::error"));
        assert_eq!(
            features.exact_identifier_case.as_deref(),
            Some("std::io::Error")
        );
    }

    #[test]
    fn rejects_dotted_file_names_as_qualified_identifiers() {
        assert!(!looks_like_qualified_identifier("config.toml"));
        assert!(looks_like_qualified_identifier("config.retrieval.rrf_k"));
        assert!(looks_like_qualified_identifier("crate::retrieval::hybrid"));
    }
}
