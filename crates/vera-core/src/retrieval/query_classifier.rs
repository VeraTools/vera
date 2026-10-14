//! Query-type classifier for adaptive retrieval parameters.
//!
//! Detects whether a search query is a natural language (NL) intent query
//! or an identifier/symbol lookup. NL queries benefit from higher vector
//! weight in RRF fusion (lower k), while identifier queries perform best
//! with the standard BM25-heavy fusion.
//!
//! # Heuristics
//!
//! - **Identifier**: camelCase, PascalCase, snake_case, SCREAMING_SNAKE,
//!   dot-separated paths, no spaces, code-like tokens (`::`).
//! - **Natural language**: contains spaces, question words, natural phrasing.
//! - **Ambiguous/single word**: defaults to identifier behavior.

/// The detected query type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryType {
    /// Natural language intent query (e.g., "how are errors handled").
    NaturalLanguage,
    /// Identifier or symbol lookup (e.g., "parse_config", "SearchWorker").
    Identifier,
}

/// Retrieval parameters tuned per query type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueryParams {
    /// Multiplier for vector candidate fetch count relative to limit.
    pub vector_candidate_multiplier: usize,
    /// Weight for BM25 RRF scores during fusion. The vector weight is
    /// `1.0` (the reference). Values above 1.0 give BM25 more influence.
    pub bm25_weight: f64,
    /// Weight for vector RRF scores during fusion.
    pub vector_weight: f64,
}

/// Default parameters for identifier queries: BM25 gets 2.5x weight since
/// identifier matches are primarily lexical (exact names, snake_case, etc.).
/// Vector search often returns loosely related code that dilutes the
/// exact-match signal, so a strong BM25 bias is essential.
const IDENTIFIER_PARAMS: QueryParams = QueryParams {
    vector_candidate_multiplier: 3,
    bm25_weight: 2.5,
    vector_weight: 1.0,
};

/// Parameters for NL intent queries: balanced weights and a larger vector candidate multiplier for broader semantic recall.
const NL_PARAMS: QueryParams = QueryParams {
    vector_candidate_multiplier: 5,
    bm25_weight: 1.0,
    vector_weight: 1.0,
};

/// Classify a query as natural language or identifier.
pub fn classify_query(query: &str) -> QueryType {
    let trimmed = query.trim();

    if trimmed.is_empty() {
        return QueryType::Identifier;
    }

    // Ignore sentence-ending periods while preserving dots inside identifiers and paths.
    let normalized = trimmed.trim_end_matches('.');

    // Multi-word queries with spaces are NL candidates.
    let words: Vec<&str> = normalized.split_whitespace().collect();

    if words.len() == 1 {
        // Single token (word or compound identifier) — treat as identifier
        // since it's more likely a symbol search.
        return QueryType::Identifier;
    }

    // Multi-word: check for identifier-like patterns first.
    // If the query contains path separators or scope operators, it's an identifier.
    if normalized.contains("::") || normalized.contains("->") || normalized.contains('.') {
        return QueryType::Identifier;
    }

    // Check if any word looks like a compound identifier (camelCase, snake_case, etc.).
    let identifier_word_count = words.iter().filter(|w| is_compound_identifier(w)).count();

    // If most words are identifiers (e.g., "parse config struct"), treat as identifier.
    if identifier_word_count > words.len() / 2 {
        return QueryType::Identifier;
    }

    // Check for NL indicators: question words, common NL patterns.
    if has_nl_indicators(normalized, &words) {
        return QueryType::NaturalLanguage;
    }

    // Multi-word with spaces and no strong identifier signals → NL.
    if words.len() >= 3 {
        return QueryType::NaturalLanguage;
    }

    // Two words, no strong signals either way — default to identifier.
    QueryType::Identifier
}

/// Get retrieval parameters for the given query type.
pub fn params_for_query_type(query_type: QueryType) -> QueryParams {
    match query_type {
        QueryType::NaturalLanguage => NL_PARAMS,
        QueryType::Identifier => IDENTIFIER_PARAMS,
    }
}

/// Check if a token looks like a compound identifier.
///
/// Detects: camelCase, PascalCase, snake_case, SCREAMING_SNAKE_CASE,
/// kebab-case (with hyphens in identifiers), path-like tokens.
fn is_compound_identifier(token: &str) -> bool {
    // Contains underscores → snake_case or SCREAMING_SNAKE.
    if token.contains('_') {
        return true;
    }

    // Contains scope operators or path separators.
    if token.contains("::") || token.contains('.') || token.contains('/') || token.contains('\\') {
        return true;
    }

    // camelCase or PascalCase: lowercase followed by uppercase within the token.
    let chars: Vec<char> = token.chars().collect();
    for i in 1..chars.len() {
        if chars[i].is_uppercase() && chars[i - 1].is_lowercase() {
            return true;
        }
    }

    false
}

/// Check for natural language indicators in the query.
fn has_nl_indicators(query: &str, words: &[&str]) -> bool {
    let lower = query.to_lowercase();

    // Question words at the start.
    let question_prefixes = [
        "how ", "what ", "where ", "why ", "when ", "which ", "who ", "does ", "do ", "is ",
        "are ", "can ", "could ", "should ", "find ", "show ", "list ", "get ",
    ];
    for prefix in &question_prefixes {
        if lower.starts_with(prefix) {
            return true;
        }
    }

    // Question mark at end.
    if query.ends_with('?') {
        return true;
    }

    // Common NL words anywhere.
    let nl_words = [
        "the",
        "a",
        "an",
        "of",
        "in",
        "for",
        "to",
        "with",
        "from",
        "that",
        "this",
        "all",
        "each",
        "every",
        "about",
        "between",
        "through",
        "during",
        "before",
        "after",
        "into",
        "using",
        "where",
        "when",
        "how",
        "what",
        "which",
        "does",
        "handle",
        "implement",
        "return",
        "returns",
    ];
    let lower_words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let nl_count = lower_words
        .iter()
        .filter(|w| nl_words.contains(&w.as_str()))
        .count();

    // If at least 2 NL words or ≥40% of words are NL-like, it's NL.
    nl_count >= 2 || (words.len() >= 3 && nl_count * 3 >= words.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_query tests ────────────────────────────────────────

    #[test]
    fn classify_camel_case_as_identifier() {
        assert_eq!(classify_query("parseConfig"), QueryType::Identifier);
        assert_eq!(classify_query("handleError"), QueryType::Identifier);
        assert_eq!(classify_query("getUserById"), QueryType::Identifier);
    }

    #[test]
    fn classify_pascal_case_as_identifier() {
        assert_eq!(classify_query("SearchWorker"), QueryType::Identifier);
        assert_eq!(classify_query("HttpClient"), QueryType::Identifier);
        assert_eq!(classify_query("DatabaseConnection"), QueryType::Identifier);
    }

    #[test]
    fn classify_snake_case_as_identifier() {
        assert_eq!(classify_query("parse_config"), QueryType::Identifier);
        assert_eq!(classify_query("get_user_by_id"), QueryType::Identifier);
        assert_eq!(classify_query("MAX_RETRIES"), QueryType::Identifier);
    }

    #[test]
    fn classify_scope_path_as_identifier() {
        assert_eq!(classify_query("std::io::Error"), QueryType::Identifier);
        assert_eq!(
            classify_query("crate::retrieval::hybrid"),
            QueryType::Identifier
        );
        assert_eq!(
            classify_query("config.retrieval.rrf_k"),
            QueryType::Identifier
        );
    }

    #[test]
    fn classify_sentence_ending_period_as_natural_language() {
        assert_eq!(
            classify_query("how does auth work."),
            QueryType::NaturalLanguage
        );
    }

    #[test]
    fn classify_dotted_identifiers_and_paths_as_identifier() {
        assert_eq!(classify_query("foo.bar"), QueryType::Identifier);
        assert_eq!(classify_query("src/main.rs"), QueryType::Identifier);
        assert_eq!(classify_query("module::symbol"), QueryType::Identifier);
        assert_eq!(classify_query("vera_core::config"), QueryType::Identifier);
    }

    #[test]
    fn classify_nl_questions_as_natural_language() {
        assert_eq!(
            classify_query("how are errors handled"),
            QueryType::NaturalLanguage
        );
        assert_eq!(
            classify_query("what does this function return"),
            QueryType::NaturalLanguage
        );
        assert_eq!(
            classify_query("where is the database connection configured"),
            QueryType::NaturalLanguage
        );
    }

    #[test]
    fn classify_nl_phrases_as_natural_language() {
        assert_eq!(
            classify_query("find all error handling code"),
            QueryType::NaturalLanguage
        );
        assert_eq!(
            classify_query("show the authentication logic"),
            QueryType::NaturalLanguage
        );
        assert_eq!(
            classify_query("code that handles user input validation"),
            QueryType::NaturalLanguage
        );
    }

    #[test]
    fn classify_question_mark_as_natural_language() {
        assert_eq!(
            classify_query("what does authenticate do?"),
            QueryType::NaturalLanguage
        );
    }

    #[test]
    fn classify_single_plain_word_as_identifier() {
        assert_eq!(classify_query("authenticate"), QueryType::Identifier);
        assert_eq!(classify_query("config"), QueryType::Identifier);
        assert_eq!(classify_query("error"), QueryType::Identifier);
    }

    #[test]
    fn classify_empty_query_as_identifier() {
        assert_eq!(classify_query(""), QueryType::Identifier);
        assert_eq!(classify_query("  "), QueryType::Identifier);
    }

    #[test]
    fn classify_two_word_identifier_like() {
        // Two words where one is a compound identifier → identifier.
        assert_eq!(classify_query("parse_config struct"), QueryType::Identifier);
    }

    #[test]
    fn classify_mixed_nl_with_identifier() {
        // NL phrasing with embedded identifiers.
        assert_eq!(
            classify_query("how does parse_config work"),
            QueryType::NaturalLanguage
        );
    }

    // ── params_for_query_type tests ─────────────────────────────────

    #[test]
    fn nl_params_have_higher_vector_multiplier() {
        let nl = params_for_query_type(QueryType::NaturalLanguage);
        let id = params_for_query_type(QueryType::Identifier);
        assert!(
            nl.vector_candidate_multiplier > id.vector_candidate_multiplier,
            "NL should fetch more vector candidates: {} > {}",
            nl.vector_candidate_multiplier,
            id.vector_candidate_multiplier
        );
    }

    #[test]
    fn identifier_params_preserve_defaults() {
        let id = params_for_query_type(QueryType::Identifier);
        assert_eq!(id.vector_candidate_multiplier, 3);
    }

    // ── is_compound_identifier tests ────────────────────────────────

    #[test]
    fn compound_identifier_detection() {
        assert!(is_compound_identifier("parseConfig"));
        assert!(is_compound_identifier("SearchWorker"));
        assert!(is_compound_identifier("parse_config"));
        assert!(is_compound_identifier("MAX_RETRIES"));
        assert!(is_compound_identifier("std::io"));
        assert!(is_compound_identifier("path/to/file"));
        assert!(!is_compound_identifier("error"));
        assert!(!is_compound_identifier("config"));
        assert!(!is_compound_identifier("the"));
    }

    // ── has_nl_indicators tests ─────────────────────────────────────

    #[test]
    fn nl_indicators_detected() {
        assert!(has_nl_indicators(
            "how are errors handled",
            &["how", "are", "errors", "handled"]
        ));
        assert!(has_nl_indicators(
            "find the config",
            &["find", "the", "config"]
        ));
        assert!(!has_nl_indicators(
            "parseConfig struct",
            &["parseConfig", "struct"]
        ));
    }
}
