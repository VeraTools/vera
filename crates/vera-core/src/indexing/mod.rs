//! Index construction and maintenance.
//!
//! This module is responsible for:
//! - Orchestrating the indexing pipeline (discover → parse → chunk → embed → store)
//! - Building BM25 full-text indexes via Tantivy
//! - Building vector indexes via sqlite-vec
//! - Incremental update logic (detect changed files, re-index only those)

pub(crate) mod checkpoint;
pub mod estimate;
pub(crate) mod file_stamp;
pub mod freshness;
mod git_exclude;
pub mod lock;
pub mod pipeline;
pub mod progress;
mod telemetry;
pub mod update;

pub use telemetry::PhaseSecs;

pub use estimate::{IndexEstimate, estimate_index};
pub use freshness::{IndexFreshness, detect_staleness};
pub use pipeline::{
    FULL_TEXT_ONLY_MODE, FileError, INDEX_MODE_KEY, IndexProgress, IndexSummary, index_dir,
    index_repository, index_repository_with_progress, is_full_text_only,
};
pub use update::{
    UpdateOptions, UpdateProgress, UpdateSummary, content_hash, update_repository,
    update_repository_with_options_and_progress,
    update_repository_with_options_and_progress_and_cancellation, update_repository_with_progress,
};

/// Whether a repository has index metadata, rather than just a stray `.vera/`.
pub fn index_exists(root: &std::path::Path) -> bool {
    index_dir(root).join("metadata.db").is_file()
}

/// Walk up from `start` and return the nearest directory containing an index.
///
/// Indexed paths are stored relative to that root, so a CLI command or MCP
/// server started in a subdirectory must use the ancestor's index rather than
/// look for (or create) one of its own. A bare `.vera/` is not an index: the
/// legacy Vera home (`~/.vera`) and stray directories must not match.
pub fn find_index_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    start
        .ancestors()
        .find(|candidate| index_exists(candidate))
        .map(std::path::Path::to_path_buf)
}

/// Why `root` is too broad to index without an explicit request, if it is:
/// a filesystem root or the user's home directory. Automatic indexing (the
/// MCP server's first search) refuses these; `vera index <dir>` still works.
pub fn broad_root_reason(root: &std::path::Path) -> Option<&'static str> {
    let resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if resolved.parent().is_none() {
        return Some("it is a filesystem root");
    }
    let home = dirs::home_dir().map(|home| home.canonicalize().unwrap_or(home));
    (home.as_deref() == Some(resolved.as_path())).then_some("it is the home directory")
}

/// Guidance for commands that require an index but could not find one.
pub fn missing_index_message(root: &std::path::Path) -> String {
    let mut message = format!(
        "no index found in {} or any parent directory. Run `vera index .` from the repository root (a first index can take minutes), or search with rg until it exists.",
        root.display()
    );
    if root
        .ancestors()
        .any(|root| root.join(".vera.resume/embeddings.db").is_file())
    {
        message.push_str("\nA previous indexing run stopped early; `vera index` will resume from its saved embeddings.");
    }
    message
}

/// Truncate embedding vectors to at most `max_dim` dimensions.
///
/// This supports Matryoshka-style dimension reduction: Qwen3 embeddings
/// retain good retrieval quality at lower dimensions, and truncation
/// dramatically reduces index storage size.
///
/// Returns the actual stored dimensionality.
pub(crate) fn truncate_embeddings(embeddings: &mut [(String, Vec<f32>)], max_dim: usize) -> usize {
    if max_dim == 0 || embeddings.is_empty() {
        return embeddings.first().map(|(_, v)| v.len()).unwrap_or(0);
    }

    let original_dim = embeddings.first().map(|(_, v)| v.len()).unwrap_or(0);
    if original_dim <= max_dim {
        return original_dim;
    }

    tracing::debug!(
        original_dim,
        truncated_dim = max_dim,
        "truncating embedding vectors"
    );

    for (_, vec) in embeddings.iter_mut() {
        vec.truncate(max_dim);
    }

    max_dim
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_index_root_walks_up_to_the_nearest_index() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let nested = root.join("crates/foo/src");
        std::fs::create_dir_all(root.join(".vera")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join(".vera").join("metadata.db"), []).unwrap();
        let other = temp.path().join("noindex/sub");
        std::fs::create_dir_all(&other).unwrap();

        assert_eq!(find_index_root(&root), Some(root.clone()));
        assert_eq!(find_index_root(&nested), Some(root));
        assert_eq!(find_index_root(&other), None);
    }

    #[test]
    fn find_index_root_ignores_a_bare_vera_directory_without_an_index() {
        // The legacy Vera home (`~/.vera`) and stray directories hold models
        // and config, never a searchable index.
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = home.join("src/project");
        std::fs::create_dir_all(home.join(".vera")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        assert_eq!(find_index_root(&project), None);

        std::fs::write(home.join(".vera").join("metadata.db"), []).unwrap();
        assert_eq!(find_index_root(&project), Some(home.clone()));
        assert_eq!(find_index_root(&home), Some(home));
    }

    #[test]
    fn broad_roots_are_the_filesystem_root_and_home() {
        assert!(broad_root_reason(std::path::Path::new("/")).is_some());
        if let Some(home) = dirs::home_dir() {
            assert!(broad_root_reason(&home).is_some());
        }
        let project = tempfile::tempdir().unwrap();
        assert_eq!(broad_root_reason(project.path()), None);
    }
}
