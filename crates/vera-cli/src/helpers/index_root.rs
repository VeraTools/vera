//! Index-root discovery, preparation, and freshness warnings.

use std::io::Write;
use std::path::{Path, PathBuf};

pub fn warn_if_index_stale(repo_path: &Path, indexing_config: &vera_core::config::IndexingConfig) {
    match vera_core::indexing::detect_staleness(repo_path, indexing_config) {
        Ok(freshness) => {
            if let Some(warning) = freshness.stale_warning() {
                print_stale_warning(
                    &vera_core::indexing::index_dir(repo_path),
                    &freshness.summary(),
                    &warning,
                );
            }
        }
        Err(err) => {
            tracing::debug!(error = %err, "failed to check index freshness");
        }
    }
}

/// Name of the dedupe record persisted inside the index directory.
const STALE_WARNING_FILE: &str = "stale-warning.json";
/// Window in which an identical stale warning is printed at most once.
const STALE_WARNING_DEDUP_SECS: u64 = 600;

#[derive(serde::Deserialize)]
struct StaleWarningRecord {
    summary: String,
    warned_at_unix_secs: u64,
}

/// Whether the stale-index warning should print again: only when the stored
/// summary differs from the current one or the last print is older than the
/// dedupe window.
fn stale_warning_should_print(
    stored: Option<(&str, u64)>,
    current_summary: &str,
    now_unix_secs: u64,
) -> bool {
    match stored {
        Some((summary, warned_at)) => {
            summary != current_summary
                || now_unix_secs.saturating_sub(warned_at) >= STALE_WARNING_DEDUP_SECS
        }
        None => true,
    }
}

/// Print the stale-index warning, deduplicated per index: an identical warning
/// that was printed within [`STALE_WARNING_DEDUP_SECS`] is suppressed, and the
/// `{summary, warned_at_unix_secs}` record is rewritten on each print.
/// `VERA_STALE_WARNING_ALWAYS=1` bypasses the dedupe; any IO failure falls
/// back to printing.
fn print_stale_warning(index_dir: &Path, summary: &str, warning: &str) {
    let always = std::env::var("VERA_STALE_WARNING_ALWAYS").is_ok_and(|v| v == "1");
    if !always {
        let record_path = index_dir.join(STALE_WARNING_FILE);
        let stored = std::fs::read_to_string(&record_path)
            .ok()
            .and_then(|data| serde_json::from_str::<StaleWarningRecord>(&data).ok());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if !stale_warning_should_print(
            stored
                .as_ref()
                .map(|r| (r.summary.as_str(), r.warned_at_unix_secs)),
            summary,
            now,
        ) {
            return;
        }
        let stderr = std::io::stderr();
        let mut err = stderr.lock();
        let _ = writeln!(err, "{warning}");
        let record = serde_json::json!({"summary": summary, "warned_at_unix_secs": now});
        let _ = std::fs::write(&record_path, record.to_string());
    } else {
        let stderr = std::io::stderr();
        let mut err = stderr.lock();
        let _ = writeln!(err, "{warning}");
    }
}

/// Walk up from `start` and return the nearest directory containing a `.vera/`
/// index. Indexed paths are stored relative to that root, so commands run in a
/// subdirectory must resolve the ancestor, not the cwd. A bare `.vera/`
/// directory is not an index: the legacy Vera home (`~/.vera`) and stray or
/// partially created directories must not match, or read commands would
/// fabricate an empty metadata store inside them.
pub fn find_index_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(candidate) = dir {
        if vera_core::indexing::index_dir(candidate)
            .join("metadata.db")
            .is_file()
        {
            return Some(candidate.to_path_buf());
        }
        dir = candidate.parent();
    }
    None
}

/// The paste-ready error shown when no `.vera/` index exists in `cwd` or any
/// parent directory.
pub fn missing_index_message(cwd: &Path) -> String {
    format!(
        "no index found in {} or any parent directory.\nRun `vera index .` from the repository root, then rerun this command.",
        cwd.display()
    )
}

/// Resolve the index root for `cwd` via [`find_index_root`], printing the
/// `note: using index at <root>` line once when the root is an ancestor.
pub fn resolve_index_root(cwd: &Path) -> Option<PathBuf> {
    let root = find_index_root(cwd)?;
    if root != cwd {
        eprintln!("note: using index at {}", root.display());
    }
    Some(root)
}

pub fn prepare_indexed_repo(
    indexing_config: &vera_core::config::IndexingConfig,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    let cwd = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("failed to get current directory: {e}"))?;
    let Some(repo_root) = resolve_index_root(&cwd) else {
        anyhow::bail!(missing_index_message(&cwd));
    };
    let index_dir = vera_core::indexing::index_dir(&repo_root);
    // Index format version must match: legacy suffixed rows would be silently wrong.
    {
        let metadata_path = index_dir.join("metadata.db");
        if metadata_path.is_file()
            && let Ok(store) = vera_core::storage::metadata::MetadataStore::open(&metadata_path)
            && !vera_core::indexing::freshness::index_format_is_current(&store)
        {
            anyhow::bail!(
                "Index format version mismatch (expected {}, found {:?}). Run `vera index {}` to rebuild the index.",
                vera_core::indexing::freshness::INDEX_FORMAT_VERSION,
                store
                    .get_index_meta(vera_core::indexing::freshness::INDEX_FORMAT_VERSION_KEY)
                    .unwrap_or(None),
                repo_root.display()
            );
        }
    }
    warn_if_index_stale(&repo_root, indexing_config);
    Ok((repo_root, index_dir))
}

pub fn should_offer_auto_index(json_output: bool, is_terminal: bool) -> bool {
    !json_output && is_terminal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_freshness_summary_formats_nonzero_counts() {
        let freshness = vera_core::indexing::IndexFreshness {
            files_added: 2,
            files_modified: 1,
            files_deleted: 3,
        };
        assert_eq!(freshness.summary(), "2 added, 1 modified, 3 deleted");
    }

    #[test]
    fn auto_index_offer_requires_human_output_and_a_terminal() {
        assert!(should_offer_auto_index(false, true));
        assert!(!should_offer_auto_index(true, true));
        assert!(!should_offer_auto_index(false, false));
        assert!(!should_offer_auto_index(true, false));
    }

    #[test]
    fn missing_index_message_preserves_the_cli_contract() {
        let message = missing_index_message(Path::new("/repo/sub/dir"));
        assert_eq!(
            message,
            "no index found in /repo/sub/dir or any parent directory.\n\
             Run `vera index .` from the repository root, then rerun this command."
        );
    }

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
        // and config, never a searchable index; they must not match, or read
        // commands would fabricate an empty metadata store inside them.
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = home.join("src/project");
        std::fs::create_dir_all(home.join(".vera")).unwrap();
        std::fs::create_dir_all(&project).unwrap();

        // A `.vera` without metadata.db is not an index.
        assert_eq!(find_index_root(&project), None);
        assert_eq!(find_index_root(&home), None);

        // With a metadata.db it becomes one.
        std::fs::write(home.join(".vera").join("metadata.db"), []).unwrap();
        assert_eq!(find_index_root(&project), Some(home.clone()));
        assert_eq!(find_index_root(&home), Some(home));
    }

    #[test]
    fn stale_warning_dedupe_suppresses_a_repeat_inside_the_window() {
        // No record: print.
        assert!(stale_warning_should_print(None, "1 added", 1_000));
        // Same summary inside the window: suppress.
        assert!(!stale_warning_should_print(
            Some(("1 added", 900)),
            "1 added",
            1_000
        ));
        // Same summary past the window: print again.
        assert!(stale_warning_should_print(
            Some(("1 added", 100)),
            "1 added",
            1_000
        ));
        // Changed summary inside the window: print.
        assert!(stale_warning_should_print(
            Some(("1 added", 900)),
            "2 added",
            1_000
        ));
    }
}
