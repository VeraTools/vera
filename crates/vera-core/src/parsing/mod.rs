//! Source code parsing using tree-sitter.
//!
//! This module is responsible for:
//! - Loading tree-sitter grammars for supported languages
//! - Parsing source files into ASTs
//! - Extracting symbol-level chunks (functions, classes, structs, etc.)
//! - Tier 0 fallback chunking for unsupported languages
//!
//! # Architecture
//!
//! - [`languages`] — Grammar loading and language detection
//! - [`extractor`] — AST node extraction rules per language
//! - [`chunker`] — Symbol-to-chunk conversion and large symbol splitting

mod c_family;
pub mod chunker;
pub mod extractor;
pub mod languages;
pub mod references;
pub mod signatures;
pub mod sphinx;
pub mod type_relations;

use anyhow::{Context, Result};
use tree_sitter::Parser;

use crate::config::IndexingConfig;
use crate::types::{Chunk, Language};

/// Parsing diagnostics captured alongside chunk extraction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseDiagnostics {
    /// Whether the tree-sitter parse tree contained error nodes.
    pub tree_has_error: bool,
    /// Whether Vera had to fall back to Tier 0 line chunking.
    pub used_tier0_fallback: bool,
}

/// Parse a source file and return both code chunks and parser diagnostics.
pub fn parse_file_with_diagnostics(
    source: &str,
    file_path: &str,
    language: Language,
    config: &IndexingConfig,
) -> Result<(Vec<Chunk>, Vec<references::RawReference>, ParseDiagnostics)> {
    // Special-case formats that don't use standard symbol extraction.
    if language == Language::Markdown {
        let chunks = chunker::markdown_section_chunks(source, file_path);
        return Ok((
            chunker::split_oversized_chunks(chunks, config.max_chunk_bytes),
            Vec::new(),
            ParseDiagnostics::default(),
        ));
    }
    if language == Language::Rst {
        let (chunks, diagnostics) = parse_rst_section_chunks(source, file_path)?;
        return Ok((
            chunker::split_oversized_chunks(chunks, config.max_chunk_bytes),
            Vec::new(),
            diagnostics,
        ));
    }
    if language.prefers_file_chunking() {
        let chunks = chunker::whole_file_chunk(source, file_path, language);
        return Ok((
            chunker::split_oversized_chunks(chunks, config.max_chunk_bytes),
            Vec::new(),
            ParseDiagnostics::default(),
        ));
    }
    if uses_indexing_tier0_fallback(language) {
        let chunks = chunker::tier0_line_chunks(source, file_path, language);
        return Ok((
            chunker::split_oversized_chunks(chunks, config.max_chunk_bytes),
            Vec::new(),
            ParseDiagnostics {
                used_tier0_fallback: true,
                ..ParseDiagnostics::default()
            },
        ));
    }

    let grammar = match languages::tree_sitter_grammar_for_path(language, file_path) {
        Some(g) => g,
        None => {
            let chunks = chunker::tier0_line_chunks(source, file_path, language);
            return Ok((
                chunker::split_oversized_chunks(chunks, config.max_chunk_bytes),
                Vec::new(),
                ParseDiagnostics {
                    used_tier0_fallback: true,
                    ..ParseDiagnostics::default()
                },
            ));
        }
    };

    let mut parser = Parser::new();
    parser
        .set_language(&grammar)
        .context("failed to load tree-sitter grammar")?;

    let (tree, language) = parse_tree(&mut parser, source, file_path, language)
        .context("tree-sitter parsing returned None")?;
    let tree_has_error = tree.root_node().has_error();

    // Single symbol extraction pass reused for both chunking and references.
    let symbols = extractor::extract_symbols(&tree, source.as_bytes(), language);
    let refs =
        references::extract_references_with_symbols(&tree, source.as_bytes(), language, &symbols);
    let chunks = chunker::chunks_from_symbols(&symbols, source, file_path, language, config);

    let used_tier0_fallback = chunks.is_empty() && !source.trim().is_empty();
    let chunks = if used_tier0_fallback {
        chunker::tier0_line_chunks(source, file_path, language)
    } else {
        chunks
    };

    Ok((
        chunker::split_oversized_chunks(chunks, config.max_chunk_bytes),
        refs,
        ParseDiagnostics {
            tree_has_error,
            used_tier0_fallback,
        },
    ))
}

/// Parse with source-preserving recovery for C-family annotations and headers.
pub(crate) fn parse_tree(
    parser: &mut Parser,
    source: &str,
    file_path: &str,
    language: Language,
) -> Option<(tree_sitter::Tree, Language)> {
    let tree = parser.parse(source, None)?;
    c_family::recover(parser, source, file_path, language, tree)
}

fn uses_indexing_tier0_fallback(language: Language) -> bool {
    matches!(language, Language::Haskell)
}

/// Parse a source file and produce chunks (without references).
///
/// Convenience wrapper around [`parse_file_with_diagnostics`] for callers that only need chunks.
pub fn parse_and_chunk(
    source: &str,
    file_path: &str,
    language: Language,
    config: &IndexingConfig,
) -> Result<Vec<Chunk>> {
    let (chunks, _refs, _diagnostics) =
        parse_file_with_diagnostics(source, file_path, language, config)?;
    Ok(chunks)
}

fn parse_rst_section_chunks(
    source: &str,
    file_path: &str,
) -> Result<(Vec<Chunk>, ParseDiagnostics)> {
    let grammar = languages::tree_sitter_grammar(Language::Rst)
        .context("missing tree-sitter grammar for reStructuredText")?;

    let mut parser = Parser::new();
    parser
        .set_language(&grammar)
        .context("failed to load reStructuredText grammar")?;

    let tree = parser
        .parse(source, None)
        .context("tree-sitter parsing returned None")?;
    let tree_has_error = tree.root_node().has_error();

    let headings = extractor::extract_rst_section_titles(&tree, source.as_bytes());
    if headings.is_empty() {
        return Ok((
            chunker::tier0_line_chunks(source, file_path, Language::Rst),
            ParseDiagnostics {
                tree_has_error,
                used_tier0_fallback: true,
            },
        ));
    }

    Ok((
        chunker::rst_section_chunks(source, file_path, &headings),
        ParseDiagnostics {
            tree_has_error,
            used_tier0_fallback: false,
        },
    ))
}

/// Parse a source file and extract call-site references only.
///
/// Only works for languages with tree-sitter grammars. Returns an empty
/// vec for unsupported languages or parse failures. Without a path, ambiguous
/// `.h` files cannot be distinguished from explicit C sources; use
/// [`parse_and_extract_references_for_path`] when the file path is available.
pub fn parse_and_extract_references(
    source: &str,
    language: Language,
) -> Vec<references::RawReference> {
    parse_and_extract_references_for_path(source, "", language)
}

/// Extract call-site references using the same path-aware grammar selection
/// and C/C++ header recovery as indexing. Prefer this when a path is known.
pub fn parse_and_extract_references_for_path(
    source: &str,
    file_path: &str,
    language: Language,
) -> Vec<references::RawReference> {
    let grammar = match languages::tree_sitter_grammar_for_path(language, file_path) {
        Some(g) => g,
        None => return Vec::new(),
    };
    let mut parser = Parser::new();
    if parser.set_language(&grammar).is_err() {
        return Vec::new();
    }
    let (tree, language) = match parse_tree(&mut parser, source, file_path, language) {
        Some(parsed) => parsed,
        None => return Vec::new(),
    };
    references::extract_references(&tree, source.as_bytes(), language)
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod metadata_tests;
