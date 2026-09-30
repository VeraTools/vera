//! Content coverage, symbol definitions, and chunk structure signals.

use crate::chunk_text::file_name;
use crate::retrieval::query_classifier::QueryType;
use crate::retrieval::query_utils::{
    content_declares_public_symbol, content_starts_with_impl, file_stem,
};
use crate::types::{SearchResult, SymbolType};

use super::query::{QueryFeatures, identifier_stems, shares_keyword_stem};

/// Minimum coverage ratio required before applying the content boost.
pub(super) const COVERAGE_MIN_RATIO: f64 = 0.5;

/// Content coverage boost weight.
pub(super) const COVERAGE_WEIGHT: f64 = 2.4;

/// Coverage weight for multi-word identifier queries.
pub(super) const COVERAGE_WEIGHT_IDENT: f64 = 2.0;

/// Coverage curve exponent. Values above 1 damp partial coverage while
/// preserving the full-coverage signal.
pub(super) const COVERAGE_EXPONENT: f64 = 2.0;

/// Lower coverage weight for multi-facet queries with explicit conjunctions.
pub(super) const COVERAGE_WEIGHT_CONJ: f64 = 1.6;

/// Coverage weight for the query: strongest for single-topic NL questions,
/// gentler when the query is multi-facet (explicit conjunction) or names a
/// symbol (symbol-definition signals should dominate there), and for
/// Identifier queries which have symbol-specific signals already.
pub(super) fn coverage_weight(features: &QueryFeatures) -> f64 {
    if features.query_type != QueryType::NaturalLanguage {
        return COVERAGE_WEIGHT_IDENT;
    }
    if features.has_conjunction || !features.embedded_symbols.is_empty() {
        COVERAGE_WEIGHT_CONJ
    } else {
        COVERAGE_WEIGHT
    }
}

/// Coverage of query content words in the chunk. Returns the covered
/// fraction when the signal applies and clears the minimum ratio.
pub(super) fn coverage_ratio(features: &QueryFeatures, content: &str) -> Option<f64> {
    let coverage_keywords: Vec<&String> = features.raw_keywords.iter().collect();
    if coverage_keywords.len() < 2 {
        return None;
    }
    let content_lower = content.to_ascii_lowercase();
    let covered = coverage_keywords
        .iter()
        .filter(|kw| content_covers_keyword(&content_lower, kw))
        .count();
    let ratio = covered as f64 / coverage_keywords.len() as f64;
    (ratio >= COVERAGE_MIN_RATIO).then_some(ratio)
}

/// Inflection-tolerant coverage. Queries inflect verbs ("parsing")
/// while code prose uses other forms ("parses"); stripping a trailing
/// "ing"/"ed"/"s" (4+ char stem kept) recovers those matches.
fn content_covers_keyword(content_lower: &str, keyword: &str) -> bool {
    if content_lower.contains(keyword) {
        return true;
    }
    for suffix in ["ing", "ed", "s"] {
        if let Some(stem) = keyword.strip_suffix(suffix)
            && stem.len() >= 4
            && content_lower.contains(stem)
        {
            return true;
        }
    }
    false
}

/// Embedded-symbol content-definition weight for natural-language queries.
pub(super) const CONTENT_SYMBOL_WEIGHT_EMBEDDED: f64 = 1.5;

/// Content-definition weight for identifier queries.
pub(super) const CONTENT_SYMBOL_WEIGHT_IDENT: f64 = 3.0;

pub(super) fn prefers_source_over_docs(features: &QueryFeatures) -> bool {
    features.query_type == QueryType::NaturalLanguage
        && features.query_word_count >= 4
        && !features.wants_config_paths
        && !features.wants_runtime_paths
        && !features.wants_archive_paths
}

/// Pool-relative content-based symbol definition boost. A chunk whose
/// text actually defines the queried symbol ("class Session",
/// "CREATE TABLE sessions") is the definition site regardless of what symbol
/// metadata extraction produced. Symbol queries scale the boost by
/// 3.0 * pool max; embedded symbols in NL queries get half strength (the
/// symbol may be incidental to the question). The file named after the
/// symbol earns the 1.5x stem-aligned tier.
pub(super) fn apply_content_symbol_boost(
    features: &QueryFeatures,
    scores: &mut [f64],
    results: &[SearchResult],
    max_score: f64,
) {
    let targets: Vec<(String, f64)> = match features.query_type {
        QueryType::Identifier => {
            let Some(name) = features.exact_identifier_case.as_deref() else {
                return;
            };
            // Match the final segment of qualified names ("std::io::Error" ->
            // "Error"); keep the full form as a fallback name.
            let short = name
                .rsplit([':', '\\', '.'])
                .next()
                .unwrap_or(name)
                .to_string();
            let mut targets = vec![(short, CONTENT_SYMBOL_WEIGHT_IDENT)];
            if !targets[0].0.eq_ignore_ascii_case(name) {
                targets.push((name.to_string(), CONTENT_SYMBOL_WEIGHT_IDENT));
            }
            targets
        }
        QueryType::NaturalLanguage => {
            if features.embedded_symbols.is_empty() {
                return;
            }
            features
                .embedded_symbols
                .iter()
                .map(|symbol| (symbol.clone(), CONTENT_SYMBOL_WEIGHT_EMBEDDED))
                .collect()
        }
    };

    let identifier_query = features.query_type == QueryType::Identifier;
    for (score, result) in scores.iter_mut().zip(results) {
        // A definition inside test/example content is a fixture or usage
        // sample, not the definition site a symbol query is after.
        if definition_site_role_blocked(features, result) {
            continue;
        }
        let mut boost = 0.0_f64;
        for (name, unit) in &targets {
            // Backstop only: chunks whose symbol metadata already matches the
            // identifier are handled by the metadata symbol boost. The content
            // scan covers extraction gaps (SQL DDL, multi-symbol chunks).
            if identifier_query
                && result
                    .symbol_name
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
            {
                continue;
            }
            if content_defines_symbol(&result.content, name) {
                let filename = file_name(&result.file_path).to_ascii_lowercase();
                let stem = file_stem(&filename);
                let stem_matched = stem_matches_symbol(stem, name);
                // Identifier lookups get a stronger stem-aligned tier, while
                // unrestricted content matching still covers extraction gaps.
                let tier = if stem_matched { 1.5 } else { 1.0 };
                boost = boost.max(unit * max_score * tier);
            }
        }
        *score += boost;
    }
}

/// Shared directory-classification constants (single definition to prevent drift).
const TEST_DIRS: &[&str] = &[
    "t",
    "test",
    "tests",
    "testing",
    "__tests__",
    "spec",
    "specs",
    "testdata",
    "fixture",
    "fixtures",
];

const EXAMPLE_DIRS: &[&str] = &[
    "example",
    "examples",
    "sample",
    "samples",
    "demo",
    "demos",
    "bench",
    "benches",
    "benchmark",
    "benchmarks",
];

/// A chunk counts as a definition site for the content-symbol boost only
/// when its path marks it as source-like. Definitions in test, example, and
/// bench trees are fixtures or usage samples; they qualify only when the
/// query explicitly asks for those paths.
///
/// Directory components decide, not bare filename tokens: a first-class
/// module named `testing.py` (click's CliRunner lives in
/// src/click/testing.py) or `example.py` is still a definition site.
fn definition_site_role_blocked(features: &QueryFeatures, result: &SearchResult) -> bool {
    let lower = result.file_path.to_ascii_lowercase();
    let mut parts = lower.rsplit('/');
    let filename = parts.next().unwrap_or("");
    let in_test_dir = parts.clone().any(|dir| TEST_DIRS.contains(&dir));
    let in_example_dir = parts.any(|dir| EXAMPLE_DIRS.contains(&dir));
    if in_test_dir || is_test_filename(filename) {
        return !features.wants_test_paths;
    }
    if in_example_dir {
        return !features.wants_example_paths;
    }
    false
}

/// Conventional test-file names: test_foo.py, foo_test.go, foo.test.ts,
/// foo-spec.rb. Deliberately narrower than substring matching so modules
/// like `testing.py` or `attest.py` stay source-like.
fn is_test_filename(filename: &str) -> bool {
    filename.starts_with("test_")
        || filename.starts_with("test-")
        || filename.starts_with("spec_")
        || filename.starts_with("spec-")
        || filename.contains("_test.")
        || filename.contains("-test.")
        || filename.contains(".test.")
        || filename.contains("_spec.")
        || filename.contains("-spec.")
        || filename.contains(".spec.")
}

/// Stem-vs-symbol match: exact, underscore-normalised, or plural-adjusted.
fn stem_matches_symbol(stem: &str, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let stem_norm = stem.replace('_', "");
    stem == name
        || stem_norm == name
        || stem.trim_end_matches('s') == name
        || stem_norm.trim_end_matches('s') == name
}

/// General definition keywords, case-sensitive. Matching is done on the
/// original line: code keywords are lowercase in practice, and
/// case-insensitive matching misfires on prose like "Class" or "Module".
const DEF_KEYWORDS: &[&str] = &[
    "abstract class",
    "data class",
    "class",
    "module",
    "defmodule",
    "def",
    "interface",
    "struct",
    "enum",
    "trait",
    "type",
    "func",
    "function",
    "object",
    "fn",
    "fun",
    "package",
    "namespace",
    "protocol",
    "record",
    "typedef",
];

/// SQL DDL definition keywords, matched case-insensitively.
const SQL_DEF_KEYWORDS: &[&str] = &[
    "create table",
    "create view",
    "create procedure",
    "create function",
];

/// Does the chunk content define `symbol`? General keywords match
/// case-sensitively, SQL DDL case-insensitively (conventionally uppercase).
fn content_defines_symbol(content: &str, symbol: &str) -> bool {
    if symbol.len() < 2 {
        return false;
    }
    // Quick reject: the symbol must appear in the chunk at all.
    let symbol_lower;
    if !content.contains(symbol) {
        symbol_lower = symbol.to_ascii_lowercase();
        if !content.to_ascii_lowercase().contains(&symbol_lower) {
            return false;
        }
    }
    let symbol_lower = symbol.to_ascii_lowercase();
    for line in content.lines() {
        if line_defines_symbol(line, symbol, false, DEF_KEYWORDS) {
            return true;
        }
        if line.to_ascii_lowercase().contains(&symbol_lower)
            && line_defines_symbol(
                &line.to_ascii_lowercase(),
                &symbol_lower,
                true,
                SQL_DEF_KEYWORDS,
            )
        {
            return true;
        }
    }
    false
}

/// Scan a line for `keyword symbol` definition sites, e.g. "class Session"
/// or "defmodule Phoenix.Router". Keyword must start the line or follow
/// whitespace.
fn line_defines_symbol(
    line: &str,
    symbol: &str,
    case_insensitive: bool,
    keywords: &[&str],
) -> bool {
    let mut start = 0;
    while start < line.len() {
        let mut earliest: Option<(usize, &str)> = None;
        for keyword in keywords {
            let mut from = start;
            while let Some(pos) = line[from..].find(keyword) {
                let abs = from + pos;
                let left_ok = abs == 0 || line.as_bytes()[abs - 1].is_ascii_whitespace();
                if left_ok {
                    earliest = Some(match earliest {
                        Some((prev, kw)) if prev <= abs => (prev, kw),
                        _ => (abs, keyword),
                    });
                    break;
                }
                from = abs + 1;
            }
        }
        let Some((pos, keyword)) = earliest else {
            return false;
        };
        if rest_starts_with_symbol(&line[pos + keyword.len()..], symbol, case_insensitive) {
            return true;
        }
        start = pos + keyword.len();
    }
    false
}

/// Check whether the text after a definition keyword names `symbol`,
/// skipping namespace qualifiers ("defmodule Phoenix.Router" defines Router).
/// The symbol must end at a delimiter so "Session" does not match
/// "SessionStore".
fn rest_starts_with_symbol(rest: &str, symbol: &str, case_insensitive: bool) -> bool {
    let mut text = rest.trim_start();
    loop {
        let ident_len = text
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if ident_len == 0 {
            return false;
        }
        let (ident, after) = text.split_at(ident_len);
        if let Some(stripped) = after.strip_prefix("::") {
            text = stripped.trim_start();
            continue;
        }
        if let Some(stripped) = after.strip_prefix('.')
            && stripped
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            text = stripped.trim_start();
            continue;
        }
        let matches = if case_insensitive {
            ident.eq_ignore_ascii_case(symbol)
        } else {
            ident == symbol
        };
        return matches
            && (after.is_empty()
                || after
                    .chars()
                    .next()
                    .is_some_and(|c| matches!(c, ' ' | '\t' | '<' | '(' | '{' | ':' | '[' | ';')));
    }
}

pub(super) fn is_public_symbol(result: &SearchResult) -> bool {
    content_declares_public_symbol(&result.content)
}

pub(super) fn looks_like_impl_block(result: &SearchResult) -> bool {
    content_starts_with_impl(&result.content)
}

pub(super) fn symbol_keyword_bonus(symbol_name: &str, keywords: &[String]) -> f64 {
    let tokens = identifier_stems(symbol_name);

    if tokens.is_empty() {
        return 0.0;
    }

    if tokens
        .iter()
        .any(|token| keywords.iter().any(|keyword| keyword == token))
    {
        return 0.5;
    }

    if tokens.iter().any(|token| {
        keywords
            .iter()
            .any(|keyword| shares_keyword_stem(token, keyword))
    }) {
        return 0.32;
    }

    0.0
}

pub(super) fn is_definition_symbol(symbol_type: Option<SymbolType>) -> bool {
    matches!(
        symbol_type,
        Some(
            SymbolType::Class
                | SymbolType::Struct
                | SymbolType::Trait
                | SymbolType::Interface
                | SymbolType::Enum
                | SymbolType::Function
                | SymbolType::Method
                | SymbolType::Module
        )
    )
}

pub(super) fn is_reexport_barrel(result: &SearchResult) -> bool {
    let filename = file_name(&result.file_path).to_ascii_lowercase();
    if !matches!(
        filename.as_str(),
        "index.ts" | "index.tsx" | "index.js" | "index.jsx" | "mod.rs"
    ) {
        return false;
    }

    let non_empty: Vec<&str> = result
        .content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .collect();
    if non_empty.is_empty() || non_empty.len() > 24 {
        return false;
    }

    let reexports = non_empty
        .iter()
        .filter(|line| {
            line.starts_with("export ")
                || line.starts_with("pub use ")
                || line.starts_with("pub mod ")
                || line.starts_with("module.exports")
        })
        .count();
    reexports > 0 && reexports * 4 >= non_empty.len() * 3
}

pub(super) fn prefers_structural_chunks(features: &QueryFeatures) -> bool {
    features.query_type == QueryType::NaturalLanguage
        && features.exact_identifier.is_none()
        && features.query_word_count >= 4
        && !features.wants_config_paths
}

pub(super) fn structural_chunk_bias(result: &SearchResult) -> f64 {
    let lines = chunk_line_span(result);
    let mut bonus = 0.0;

    match result.symbol_type {
        Some(
            SymbolType::Struct | SymbolType::Class | SymbolType::Trait | SymbolType::Interface,
        ) => {
            bonus += 0.38;
        }
        Some(SymbolType::Enum | SymbolType::Module) => {
            bonus += 0.28;
        }
        Some(SymbolType::Block) if looks_like_impl_block(result) || lines >= 24 => {
            bonus += 0.24;
        }
        Some(SymbolType::Variable) => {
            bonus -= 0.45;
        }
        Some(SymbolType::Method | SymbolType::Function) if lines <= 8 => {
            bonus -= 0.32;
        }
        _ => {}
    }

    if lines <= 4 {
        bonus -= 0.2;
    } else if (12..=120).contains(&lines) {
        bonus += 0.12;
    }

    bonus
}

pub(super) fn chunk_line_span(result: &SearchResult) -> u32 {
    result.line_end.saturating_sub(result.line_start) + 1
}

/// Check if chunk content defines a symbol using language-agnostic keyword matching.
///
/// Looks for definition keywords (class, struct, def, function, etc.) followed
/// by a symbol name that matches query keywords. This is stronger than just
/// checking symbol_type metadata because it confirms the chunk is the actual
/// definition site, not just a reference.
pub(super) fn content_defines_query_keyword(content: &str, keywords: &[String]) -> bool {
    static DEFINITION_PREFIXES: &[&str] = &[
        "class ",
        "struct ",
        "enum ",
        "trait ",
        "interface ",
        "type ",
        "module ",
        "def ",
        "fn ",
        "func ",
        "function ",
        "fun ",
        "pub fn ",
        "pub struct ",
        "pub enum ",
        "pub trait ",
        "pub type ",
        "pub mod ",
        "export class ",
        "export function ",
        "export interface ",
        "export type ",
        "export enum ",
        "export default class ",
        "export default function ",
        "abstract class ",
        "data class ",
        "object ",
        "protocol ",
        "record ",
        "namespace ",
        "package ",
        "defmodule ",
    ];

    // Only consider keywords with 5+ chars to avoid false positives
    // from common short words like "file", "type", "list".
    let long_keywords: Vec<&String> = keywords.iter().filter(|k| k.len() >= 5).collect();
    if long_keywords.is_empty() {
        return false;
    }

    for line in content.lines().take(5) {
        let trimmed = line.trim();
        for prefix in DEFINITION_PREFIXES {
            let rest = trimmed.strip_prefix(*prefix);
            if let Some(rest) = rest {
                // Extract the symbol name after the keyword.
                let symbol: String = rest
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    .collect();
                if symbol.len() >= 3 {
                    let sym_stems = identifier_stems(&symbol);
                    let matches_keyword = long_keywords.iter().any(|kw| {
                        sym_stems
                            .iter()
                            .any(|s| s == kw.as_str() || shares_keyword_stem(s, kw))
                    });
                    if matches_keyword {
                        return true;
                    }
                }
            }
        }
    }
    false
}
