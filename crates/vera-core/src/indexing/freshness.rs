//! Index freshness metadata and stale-index detection.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use tracing::warn;

use crate::CancellationToken;
use crate::config::IndexingConfig;
use crate::discovery;
use crate::storage::metadata::{FileFingerprint, MetadataStore, StampedFileHash};

use super::update::{detect_language_for_path, hash_for_indexing_source};
use super::{file_stamp, index_dir};

pub const INDEXING_CONFIG_KEY: &str = "indexing_config";
const INDEX_REFRESHED_AT_KEY: &str = "index_refreshed_at_unix_ms";
pub const INDEX_FORMAT_VERSION_KEY: &str = "index_format_version";
/// Increment this when the on-disk chunk or index format changes incompatibly.
/// Eval harness `reuse_index` gates on this value; a missing or mismatched
/// version forces a full re-index so stale chunk identity is never reused.
pub const INDEX_FORMAT_VERSION: &str = "2";
/// Written last during index publication; its absence means the index was not
/// fully written (interrupted build). `reuse_index` refuses to reuse an index
/// lacking this marker.
pub const INDEX_COMPLETE_KEY: &str = "index_complete";
pub const INDEX_COMPLETE_VALUE: &str = "1";

/// Legacy indexes lack this key; only an explicit interrupted-update marker fails.
pub fn ensure_index_complete(idx_dir: &Path) -> Result<()> {
    let metadata = MetadataStore::open_existing(&idx_dir.join("metadata.db"))
        .context("failed to open index completeness metadata")?;
    if metadata.get_index_meta(INDEX_COMPLETE_KEY)?.as_deref() == Some("0")
        && !super::lock::IndexLock::is_locked_for_index_dir(idx_dir)
    {
        bail!(
            "the index at {} is incomplete because an update stopped before it finished writing; run `vera update` to repair it or `vera index` to rebuild",
            idx_dir.display()
        );
    }
    Ok(())
}

/// Returns true if the stored index format version matches the current binary.
pub fn index_format_is_current(metadata_store: &MetadataStore) -> bool {
    matches!(
        metadata_store.get_index_meta(INDEX_FORMAT_VERSION_KEY),
        Ok(Some(stored)) if stored == INDEX_FORMAT_VERSION
    )
}

/// Reject indexes built with the retired character-cap chunking experiment.
///
/// Missing or zero caps preserve compatibility with ordinary indexes. Inspect
/// the raw metadata before deserialization can discard historical aliases.
pub fn ensure_index_chunking_compatible(
    metadata_store: &MetadataStore,
    repo_path: &Path,
) -> Result<()> {
    let rebuild = || {
        format!(
            "Run `vera index {}` to rebuild the full index.",
            repo_path.display()
        )
    };
    let Some(encoded) = metadata_store
        .get_index_meta(INDEXING_CONFIG_KEY)
        .context("failed to read saved indexing config")?
    else {
        return Ok(());
    };
    let value: serde_json::Value = serde_json::from_str(&encoded)
        .with_context(|| format!("Invalid saved indexing config. {}", rebuild()))?;
    for key in ["chunk_max_chars", "max_chunk_chars", "max_chunk_characters"] {
        if let Some(cap) = value.get(key)
            && cap.as_u64() != Some(0)
        {
            bail!(
                "Index uses retired character-cap chunking ({key}={cap}). {}",
                rebuild()
            );
        }
    }
    Ok(())
}

/// Summary of drift between the working tree and the current index.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct IndexFreshness {
    pub files_added: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
}

impl IndexFreshness {
    pub fn is_stale(&self) -> bool {
        self.total_changes() > 0
    }

    pub fn total_changes(&self) -> usize {
        self.files_added + self.files_modified + self.files_deleted
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.files_added > 0 {
            parts.push(format!("{} added", self.files_added));
        }
        if self.files_modified > 0 {
            parts.push(format!("{} modified", self.files_modified));
        }
        if self.files_deleted > 0 {
            parts.push(format!("{} deleted", self.files_deleted));
        }
        parts.join(", ")
    }

    /// The stale-index warning shown to users, or `None` when the index covers
    /// the tree. Single-sourced here because the CLI prints it on stderr and
    /// the MCP server attaches it to tool results, and the two must agree.
    pub fn stale_warning(&self, vera_command: &str) -> Option<String> {
        if !self.is_stale() {
            return None;
        }
        Some(format!(
            "warning: index may be stale: {}. Search and grep only cover indexed files. Run `{vera_command} update .` or `{vera_command} watch .`.",
            self.summary()
        ))
    }
}

/// CLI prefix for a repository-specific hint, shell-quoting paths that need it.
pub fn vera_command_for_repo(repo: &Path) -> String {
    let path = repo.display().to_string();
    let shell_safe = |c: char| c.is_ascii_alphanumeric() || "/._-+:@,=~".contains(c);
    if !path.chars().all(shell_safe) {
        format!("vera -C '{}'", path.replace('\'', "'\\''"))
    } else {
        format!("vera -C {path}")
    }
}

pub(crate) fn record_index_snapshot(
    metadata_store: &MetadataStore,
    indexing_config: &IndexingConfig,
) -> Result<()> {
    metadata_store
        .set_index_meta(
            INDEXING_CONFIG_KEY,
            &serde_json::to_string(indexing_config).context("failed to encode indexing config")?,
        )
        .context("failed to store indexing config metadata")?;
    metadata_store
        .set_index_meta(
            INDEX_REFRESHED_AT_KEY,
            &current_time_millis()
                .context("failed to compute index refresh timestamp")?
                .to_string(),
        )
        .context("failed to store index refresh timestamp")?;
    metadata_store
        .set_index_meta(INDEX_FORMAT_VERSION_KEY, INDEX_FORMAT_VERSION)
        .context("failed to store index format version")?;
    // Written last: if this key is absent, the index was interrupted before
    // publication completed. Must be last so a crash before it leaves a
    // recognizably incomplete index.
    metadata_store
        .set_index_meta(INDEX_COMPLETE_KEY, INDEX_COMPLETE_VALUE)
        .context("failed to store index completeness marker")?;
    Ok(())
}

/// Compare the current repo contents against the index metadata.
///
/// New and deleted files are detected via discovery vs tracked files. Modified
/// files use metadata fingerprints, falling back to content hashes when needed.
pub fn detect_staleness(
    repo_path: &Path,
    fallback_config: &IndexingConfig,
) -> Result<IndexFreshness> {
    let repo_root = repo_path
        .canonicalize()
        .with_context(|| format!("failed to resolve repo path: {}", repo_path.display()))?;
    let metadata_path = index_dir(&repo_root).join("metadata.db");
    let metadata_store =
        MetadataStore::open_existing(&metadata_path).context("failed to open metadata store")?;

    ensure_index_chunking_compatible(&metadata_store, &repo_root)?;

    let indexing_config = load_indexing_config(&metadata_store, fallback_config);
    let stored = metadata_store.file_fingerprints()?;
    let stamps = known_stamps(&stored);
    let cancellation = CancellationToken::new();
    let discovery = discovery::discover_files_with_stamps(
        &repo_root,
        &indexing_config,
        &cancellation,
        Some(&stamps),
    )
    .context("failed to discover files for freshness scan")?;
    let classified = classify_files(
        &discovery,
        &stored,
        &repo_root,
        indexing_config.max_file_size_bytes,
        false,
        &cancellation,
    )?;
    if let Err(error) =
        MetadataStore::try_backfill_file_stamps(&metadata_path, &classified.backfills)
    {
        tracing::debug!(%error, "could not backfill freshness file stamps");
    }
    Ok(IndexFreshness {
        files_added: classified.added.len(),
        files_modified: classified.modified.len() + classified.unreadable.len(),
        files_deleted: classified.deleted.len(),
    })
}

pub(crate) fn known_stamps(stored: &HashMap<String, FileFingerprint>) -> HashMap<String, String> {
    stored
        .iter()
        .filter_map(|(path, fingerprint)| {
            // RST hashes include dependencies, so its own stat can never certify it.
            (detect_language_for_path(path) != crate::types::Language::Rst)
                .then(|| {
                    fingerprint
                        .file_stamp
                        .as_ref()
                        .map(|stamp| (path.clone(), stamp.clone()))
                })
                .flatten()
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct FileClassification {
    pub modified: Vec<String>,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    pub unchanged: Vec<String>,
    pub unreadable: Vec<String>,
    pub backfills: Vec<StampedFileHash>,
}

/// Shared by freshness and update. Contents are dropped on the hashing worker;
/// only changed paths and conditional stamp backfills leave this phase.
pub(crate) fn classify_files(
    discovery: &discovery::DiscoveryResult,
    stored: &HashMap<String, FileFingerprint>,
    repo_root: &Path,
    max_file_size_bytes: u64,
    verify: bool,
    cancellation: &CancellationToken,
) -> Result<FileClassification> {
    enum Status {
        Added,
        Modified,
        Unchanged(Option<StampedFileHash>),
        Unreadable,
    }
    let results: Vec<_> = discovery
        .files
        .par_iter()
        .map(|file| -> Result<_> {
            cancellation.check()?;
            let rel_path = &file.relative_path;
            let Some(previous) = stored.get(rel_path) else {
                return Ok((rel_path.clone(), Status::Added));
            };
            let language = detect_language_for_path(rel_path);
            if !verify
                && language != crate::types::Language::Rst
                && let Some(known) = &previous.file_stamp
                && file_stamp::at(&discovery.root_dir, Path::new(rel_path))
                    .ok()
                    .flatten()
                    .is_some_and(|stamp| stamp.value() == known)
            {
                return Ok((rel_path.clone(), Status::Unchanged(None)));
            }
            let (content, stamp) = match file_stamp::read_source_with_stamp_at(
                &discovery.root_dir,
                Path::new(rel_path),
            ) {
                Ok(read) => read,
                Err(err) => {
                    warn!(
                        file = %rel_path,
                        error = %err,
                        "failed to read file during classification"
                    );
                    return Ok((rel_path.clone(), Status::Unreadable));
                }
            };
            let current_hash = hash_for_indexing_source(
                &content,
                rel_path,
                language,
                repo_root,
                max_file_size_bytes,
            );
            let status = if current_hash == previous.content_hash {
                let stamp = stamp.filter(|_| language != crate::types::Language::Rst);
                let backfill = (stamp.is_some() && stamp != previous.file_stamp)
                    .then(|| (rel_path.clone(), current_hash, stamp));
                Status::Unchanged(backfill)
            } else {
                Status::Modified
            };
            Ok((rel_path.clone(), status))
        })
        .collect::<Result<_>>()?;
    cancellation.check()?;
    let mut classified = FileClassification::default();
    let current: HashSet<_> = discovery
        .files
        .iter()
        .map(|file| file.relative_path.as_str())
        .collect();
    classified.deleted = stored
        .keys()
        .filter(|path| !current.contains(path.as_str()))
        .cloned()
        .collect();
    classified.deleted.sort();
    for (path, status) in results {
        match status {
            Status::Added => classified.added.push(path),
            Status::Modified => classified.modified.push(path),
            Status::Unreadable => classified.unreadable.push(path),
            Status::Unchanged(backfill) => {
                classified.unchanged.push(path);
                if let Some(backfill) = backfill {
                    classified.backfills.push(backfill);
                }
            }
        }
    }
    Ok(classified)
}

fn load_indexing_config(
    metadata_store: &MetadataStore,
    fallback_config: &IndexingConfig,
) -> IndexingConfig {
    match metadata_store.get_index_meta(INDEXING_CONFIG_KEY) {
        Ok(Some(encoded)) => match serde_json::from_str(&encoded) {
            Ok(config) => config,
            Err(err) => {
                warn!(error = %err, "failed to decode saved indexing config");
                fallback_config.clone()
            }
        },
        Ok(None) => fallback_config.clone(),
        Err(err) => {
            warn!(error = %err, "failed to read saved indexing config");
            fallback_config.clone()
        }
    }
}

fn current_time_millis() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IndexingConfig;
    use crate::indexing::content_hash;
    use tempfile::tempdir;

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn search_and_stats_refuse_interrupted_updates_unless_locked_or_legacy() {
        use crate::embedding::test_helpers::MockProvider;
        use crate::indexing::{index_repository, update_repository};
        use crate::retrieval::search_service::SearchContext;
        let root = tempdir().unwrap();
        write_file(root.path(), "main.rs", "pub fn searchable() {}\n");
        let provider = MockProvider::new(4);
        let config = crate::config::VeraConfig::default();
        index_repository(root.path(), &provider, &config, "model")
            .await
            .unwrap();
        let idx_dir = index_dir(root.path());
        let metadata = MetadataStore::open(&idx_dir.join("metadata.db")).unwrap();
        let context = SearchContext::bm25_only();
        let filters = crate::types::SearchFilters::default();
        // Warm the cache first: the marker must be checked even on cache hits.
        context
            .search(&idx_dir, "searchable", None, &config, &filters, 5)
            .await
            .unwrap();
        metadata.set_index_meta(INDEX_COMPLETE_KEY, "0").unwrap();
        let stats_error = crate::stats::collect_stats(root.path())
            .unwrap_err()
            .to_string();
        let search_error = context
            .search(&idx_dir, "searchable", None, &config, &filters, 5)
            .await
            .unwrap_err()
            .to_string();
        for error in [stats_error, search_error] {
            assert!(error.contains(&idx_dir.display().to_string()));
            assert!(error.contains("incomplete because an update stopped"));
            assert!(error.contains("`vera update` to repair it or `vera index` to rebuild"));
        }
        let lock = super::super::lock::IndexLock::try_acquire_for_index_dir(&idx_dir)
            .unwrap()
            .unwrap();
        crate::stats::collect_stats(root.path()).unwrap();
        assert!(
            !context
                .search(&idx_dir, "searchable", None, &config, &filters, 5)
                .await
                .unwrap()
                .0
                .is_empty()
        );
        drop(lock);
        // Update intentionally bypasses the refusal, including a no-op repair.
        update_repository(root.path(), &provider, &config, "model")
            .await
            .unwrap();
        assert_eq!(
            metadata
                .get_index_meta(INDEX_COMPLETE_KEY)
                .unwrap()
                .as_deref(),
            Some("1")
        );
        let connection = rusqlite::Connection::open(idx_dir.join("metadata.db")).unwrap();
        connection
            .execute(
                "DELETE FROM index_metadata WHERE key = ?1",
                [INDEX_COMPLETE_KEY],
            )
            .unwrap();
        crate::stats::collect_stats(root.path()).unwrap();
        context
            .search(&idx_dir, "searchable", None, &config, &filters, 5)
            .await
            .unwrap();
    }

    #[test]
    fn stale_warning_names_every_kind_of_drift() {
        let freshness = IndexFreshness {
            files_added: 1,
            files_modified: 2,
            files_deleted: 3,
        };
        assert_eq!(
            freshness.stale_warning("vera").unwrap(),
            "warning: index may be stale: 1 added, 2 modified, 3 deleted. \
             Search and grep only cover indexed files. \
             Run `vera update .` or `vera watch .`."
        );
    }

    #[test]
    fn fresh_index_has_no_stale_warning() {
        assert_eq!(IndexFreshness::default().stale_warning("vera"), None);
    }

    #[test]
    fn stale_warning_uses_the_selected_repository_command() {
        let freshness = IndexFreshness {
            files_modified: 1,
            ..Default::default()
        };
        assert_eq!(
            freshness
                .stale_warning("vera -C '/dependency source'")
                .unwrap(),
            "warning: index may be stale: 1 modified. Search and grep only cover indexed files. Run `vera -C '/dependency source' update .` or `vera -C '/dependency source' watch .`."
        );
    }

    #[test]
    fn repository_command_quotes_only_paths_with_whitespace() {
        for (path, command) in [
            ("/dependency", "vera -C /dependency"),
            ("/dependency source", "vera -C '/dependency source'"),
            ("/user's dependency", "vera -C '/user'\\''s dependency'"),
            ("/user'srepo", "vera -C '/user'\\''srepo'"),
            ("/a$b", "vera -C '/a$b'"),
        ] {
            assert_eq!(vera_command_for_repo(Path::new(path)), command);
        }
    }

    #[test]
    fn retired_character_caps_require_a_full_rebuild_under_every_alias() {
        let metadata = MetadataStore::open_in_memory().unwrap();
        let root = Path::new("fixture-repo");
        ensure_index_chunking_compatible(&metadata, root).unwrap();
        for encoded in [
            r#"{}"#,
            r#"{"chunk_max_chars":0,"max_chunk_chars":0,"max_chunk_characters":0}"#,
        ] {
            metadata
                .set_index_meta(INDEXING_CONFIG_KEY, encoded)
                .unwrap();
            ensure_index_chunking_compatible(&metadata, root).unwrap();
        }
        for key in ["chunk_max_chars", "max_chunk_chars", "max_chunk_characters"] {
            let mut encoded = serde_json::json!({
                "chunk_max_chars": 0,
                "max_chunk_chars": 0,
                "max_chunk_characters": 0,
            });
            encoded[key] = serde_json::json!(750);
            metadata
                .set_index_meta(INDEXING_CONFIG_KEY, &encoded.to_string())
                .unwrap();
            let error = ensure_index_chunking_compatible(&metadata, root)
                .unwrap_err()
                .to_string();
            assert!(error.contains(key));
            assert!(error.contains("vera index fixture-repo"));
        }
        metadata
            .set_index_meta(INDEXING_CONFIG_KEY, "invalid-json")
            .unwrap();
        let error = ensure_index_chunking_compatible(&metadata, root)
            .unwrap_err()
            .to_string();
        assert!(error.contains("vera index fixture-repo"));
    }

    #[test]
    fn detects_added_modified_and_deleted_files() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", "pub fn current() {}\n");
        write_file(dir.path(), "src/new.rs", "pub fn added() {}\n");

        let index_dir = dir.path().join(".vera");
        std::fs::create_dir_all(&index_dir).unwrap();
        let metadata = MetadataStore::open(&index_dir.join("metadata.db")).unwrap();
        metadata
            .set_file_hash("src/lib.rs", &content_hash("pub fn previous() {}\n"))
            .unwrap();
        metadata
            .set_file_hash("src/deleted.rs", &content_hash("pub fn deleted() {}\n"))
            .unwrap();

        let freshness = detect_staleness(dir.path(), &IndexingConfig::default()).unwrap();
        assert_eq!(
            freshness,
            IndexFreshness {
                files_added: 1,
                files_modified: 1,
                files_deleted: 1,
            }
        );
    }

    #[test]
    fn freshness_scan_checks_tracked_files_even_when_snapshot_is_newer() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", "pub fn current() {}\n");

        let index_dir = dir.path().join(".vera");
        std::fs::create_dir_all(&index_dir).unwrap();
        let metadata = MetadataStore::open(&index_dir.join("metadata.db")).unwrap();
        metadata
            .set_file_hash("src/lib.rs", &content_hash("pub fn previous() {}\n"))
            .unwrap();
        metadata
            .set_index_meta(INDEX_REFRESHED_AT_KEY, &u64::MAX.to_string())
            .unwrap();

        let freshness = detect_staleness(dir.path(), &IndexingConfig::default()).unwrap();
        assert_eq!(freshness.files_modified, 1);
    }

    #[test]
    fn freshness_scan_marks_tracked_read_failures_as_modified() {
        let dir = tempdir().unwrap();
        let read_failure_path = dir.path().join("src/unreadable.rs");
        // Model a tracked file being replaced by a directory after discovery.
        std::fs::create_dir_all(&read_failure_path).unwrap();

        let index_dir = dir.path().join(".vera");
        std::fs::create_dir_all(&index_dir).unwrap();
        let metadata = MetadataStore::open(&index_dir.join("metadata.db")).unwrap();
        metadata
            .set_file_hash("src/unreadable.rs", &content_hash("fn indexed() {}\n"))
            .unwrap();

        let mut discovery =
            discovery::discover_files(dir.path(), &IndexingConfig::default()).unwrap();
        discovery.files.push(discovery::DiscoveredFile {
            relative_path: "src/unreadable.rs".into(),
            absolute_path: read_failure_path,
            size: 1,
        });
        let classified = classify_files(
            &discovery,
            &metadata.file_fingerprints().unwrap(),
            dir.path(),
            IndexingConfig::default().max_file_size_bytes,
            false,
            &CancellationToken::new(),
        )
        .unwrap();

        assert_eq!(classified.unreadable, ["src/unreadable.rs"]);
        assert!(classified.deleted.is_empty());
    }

    #[test]
    fn freshness_scan_uses_saved_indexing_config() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "generated/out.rs", "pub fn generated() {}\n");

        let index_dir = dir.path().join(".vera");
        std::fs::create_dir_all(&index_dir).unwrap();
        let metadata = MetadataStore::open(&index_dir.join("metadata.db")).unwrap();

        let saved_config = IndexingConfig {
            extra_excludes: vec!["generated/**".to_string()],
            ..Default::default()
        };
        record_index_snapshot(&metadata, &saved_config).unwrap();

        let freshness = detect_staleness(dir.path(), &IndexingConfig::default()).unwrap();
        assert_eq!(freshness.files_added, 0);
    }

    #[cfg(unix)]
    #[test]
    fn freshness_backfills_old_and_racy_stamps_then_skips_reads() {
        let dir = tempdir().unwrap();
        let content = "pub fn current() {}\n";
        write_file(dir.path(), "lib.rs", content);
        std::fs::create_dir(dir.path().join(".vera")).unwrap();
        let metadata = MetadataStore::open(&dir.path().join(".vera/metadata.db")).unwrap();
        metadata
            .set_file_hash("lib.rs", &content_hash(content))
            .unwrap();
        // A just-written file is inside the race window and cannot be certified.
        assert!(
            !detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .is_stale()
        );
        assert!(
            metadata.file_fingerprints().unwrap()["lib.rs"]
                .file_stamp
                .is_none()
        );
        std::thread::sleep(std::time::Duration::from_millis(3100));
        assert!(
            !detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .is_stale()
        );
        assert!(
            metadata.file_fingerprints().unwrap()["lib.rs"]
                .file_stamp
                .is_some()
        );
        // Simulate an older index with no stored fingerprints.
        let db = rusqlite::Connection::open(dir.path().join(".vera/metadata.db")).unwrap();
        db.execute("UPDATE file_hashes SET file_stamp=NULL", [])
            .unwrap();
        assert!(
            !detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .is_stale()
        );
        assert!(
            metadata.file_fingerprints().unwrap()["lib.rs"]
                .file_stamp
                .is_some()
        );
        let root = discovery::open_root_dir(dir.path()).unwrap();
        let reads = file_stamp::read_counter(&root, Path::new("lib.rs"), false);
        assert!(
            !detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .is_stale()
        );
        assert_eq!(
            reads,
            file_stamp::read_counter(&root, Path::new("lib.rs"), false)
        );

        let modified = std::fs::metadata(dir.path().join("lib.rs"))
            .unwrap()
            .modified()
            .unwrap();
        write_file(dir.path(), "lib.rs", "pub fn changed() {}\n");
        std::fs::File::options()
            .write(true)
            .open(dir.path().join("lib.rs"))
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(
            detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .files_modified,
            1
        );
        write_file(dir.path(), "replacement", "pub fn swapped() {}\n");
        std::fs::rename(dir.path().join("replacement"), dir.path().join("lib.rs")).unwrap();
        assert_eq!(
            detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .files_modified,
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn freshness_works_with_read_only_old_metadata_and_busy_backfills() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        write_file(dir.path(), "lib.rs", "fn current() {}\n");
        let db_path = dir.path().join(".vera/metadata.db");
        std::fs::create_dir(dir.path().join(".vera")).unwrap();
        let metadata = MetadataStore::open(&db_path).unwrap();
        metadata
            .set_file_hash("lib.rs", &content_hash("fn current() {}\n"))
            .unwrap();
        drop(metadata);
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch(
            "ALTER TABLE file_hashes DROP COLUMN file_stamp; PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
        drop(db);
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(
            !detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .is_stale()
        );
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        // SQLite creates WAL/shm with the database's permissions, even for a
        // read-only reader. Restore these too before exercising migration.
        for sidecar in ["metadata.db-wal", "metadata.db-shm"] {
            let path = db_path.with_file_name(sidecar);
            if path.exists() {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
        }
        let metadata = MetadataStore::open(&db_path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3100));
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        let start = std::time::Instant::now();
        assert!(
            !detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .is_stale()
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert!(
            metadata.file_fingerprints().unwrap()["lib.rs"]
                .file_stamp
                .is_none()
        );
        db.execute_batch("ROLLBACK").unwrap();
        drop(db);
        drop(metadata);
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o444)).unwrap();
        // This scan attempts a trustworthy backfill, which must fail harmlessly.
        assert!(
            !detect_staleness(dir.path(), &IndexingConfig::default())
                .unwrap()
                .is_stale()
        );
        assert!(
            MetadataStore::open_existing(&db_path)
                .unwrap()
                .file_fingerprints()
                .unwrap()["lib.rs"]
                .file_stamp
                .is_none()
        );
    }
}
