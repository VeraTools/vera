//! Seed a linked worktree from its main checkout, then refresh before publication.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};

use crate::CancellationToken;
use crate::config::{InferenceBackend, VeraConfig};
use crate::embedding::{EmbeddingProvider, create_dynamic_provider};

use super::lock::IndexLock;
use super::{UpdateOptions, UpdateSummary, find_index_root, index_dir, index_exists};

pub struct LinkedWorktree {
    pub root: PathBuf,
    pub main: PathBuf,
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(cwd).args(args);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
    ] {
        command.env_remove(name);
    }
    let output = command.output().ok()?;
    output.status.success().then_some(())?;
    String::from_utf8(output.stdout).ok()
}

/// A linked worktree's `.git` is a file pointing into `<common>/worktrees/<name>`;
/// submodules point into `<common>/modules/` instead. No subprocess, so cheap per ancestor.
pub(crate) fn is_linked_worktree_root(dir: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(dir.join(".git")) else {
        return false;
    };
    contents
        .trim()
        .strip_prefix("gitdir:")
        .and_then(|git_dir| Path::new(git_dir.trim()).parent()?.file_name())
        .is_some_and(|name| name == "worktrees")
}

/// Non-git directories, ordinary checkouts, and worktrees of bare repositories do not seed.
pub fn linked_worktree(cwd: &Path) -> Option<LinkedWorktree> {
    let paths = git(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ],
    )?;
    let mut paths = paths.lines();
    let root = PathBuf::from(paths.next()?);
    let git_dir = paths.next()?;
    let common_dir = paths.next()?;
    if git_dir == common_dir {
        return None;
    }
    let listing = git(cwd, &["worktree", "list", "--porcelain"])?;
    let first = listing.split("\n\n").next()?;
    if first.lines().any(|line| line == "bare") {
        return None;
    }
    let main = PathBuf::from(first.lines().next()?.strip_prefix("worktree ")?);
    Some(LinkedWorktree { root, main })
}

pub struct SeededWorktree {
    pub root: PathBuf,
    pub main: PathBuf,
    pub summary: UpdateSummary,
}

impl SeededWorktree {
    pub fn note(&self) -> String {
        format!(
            "note: seeded this worktree's index from {} and updated {} changed files",
            self.main.display(),
            self.summary.files_added + self.summary.files_modified + self.summary.files_deleted
        )
    }
}

struct StagingDir(PathBuf);

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
        // Leaves `.vera.build` in place when another build still uses it.
        if let Some(parent) = self.0.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}

/// An unpublished copy. The destination's normal index lock is held until publication.
pub struct WorktreeSeed {
    worktree: LinkedWorktree,
    staging: StagingDir,
    _lock: IndexLock,
}

impl WorktreeSeed {
    pub fn prepare(cwd: &Path) -> Result<Option<Self>> {
        if find_index_root(cwd).is_some() {
            return Ok(None);
        }
        let Some(worktree) = linked_worktree(cwd) else {
            return Ok(None);
        };
        if !index_exists(&worktree.main) || index_exists(&worktree.root) {
            return Ok(None);
        }
        let failure_context = format!(
            "Could not seed this worktree from {}. Run `vera index .` from {} to create a usable worktree index",
            index_dir(&worktree.main).display(),
            worktree.root.display()
        );
        Self::prepare_from(worktree).context(failure_context)
    }

    fn prepare_from(worktree: LinkedWorktree) -> Result<Option<Self>> {
        let lock = IndexLock::acquire_blocking_for_repo(&worktree.root)?;
        if index_exists(&worktree.root) {
            return Ok(None);
        }
        let main_lock = IndexLock::acquire_blocking_for_repo(&worktree.main)?;
        let metadata = crate::storage::metadata::MetadataStore::open_existing(
            &index_dir(&worktree.main).join("metadata.db"),
        )?;
        if metadata
            .get_index_meta(super::freshness::INDEX_COMPLETE_KEY)?
            .as_deref()
            == Some("0")
        {
            bail!("main-checkout index is incomplete; repair it before seeding a worktree");
        }
        drop(metadata);
        // Everything under .vera.build is excluded even with --no-default-excludes.
        let parent = worktree.root.join(".vera.build");
        fs::create_dir_all(&parent)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let staging = loop {
            let path = parent.join(format!(
                "worktree-seed-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => break StagingDir(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        copy_index(&index_dir(&worktree.main), &staging.0.join("index")).with_context(|| {
            format!(
                "failed to seed from {}",
                index_dir(&worktree.main).display()
            )
        })?;
        drop(main_lock);
        Ok(Some(Self {
            worktree,
            staging,
            _lock: lock,
        }))
    }

    pub fn is_full_text_only(&self) -> Result<bool> {
        super::is_full_text_only(&self.staging.0.join("index"))
    }

    fn failure(&self, error: anyhow::Error) -> anyhow::Error {
        anyhow::anyhow!(
            "Could not seed this worktree from {}: {error:#}. Run `vera index .` from {} to create a usable worktree index.",
            index_dir(&self.worktree.main).display(),
            self.worktree.root.display()
        )
    }

    /// Build the configured provider only after determining the source index's mode.
    pub async fn update_configured(
        self,
        config: &VeraConfig,
        backend: InferenceBackend,
    ) -> Result<SeededWorktree> {
        let provider = if self
            .is_full_text_only()
            .map_err(|error| self.failure(error))?
        {
            None
        } else {
            Some(
                create_dynamic_provider(config, backend)
                    .await
                    .map_err(|error| self.failure(error))?,
            )
        };
        self.update(
            provider.as_ref().map(|(provider, _)| provider),
            config,
            provider.as_ref().map_or("", |(_, model)| model),
        )
        .await
    }

    pub async fn update<P: EmbeddingProvider>(
        self,
        provider: Option<&P>,
        config: &VeraConfig,
        model: &str,
    ) -> Result<SeededWorktree> {
        let result = async {
            let staged_index = self.staging.0.join("index");
            let summary = super::update::update_repository_at_index(
                (&self.worktree.root, Some((&staged_index, &self._lock))),
                provider,
                config,
                model,
                &UpdateOptions {
                    verify: true,
                    ..Default::default()
                },
                |_| {},
                &CancellationToken::new(),
            )
            .await?;
            if !summary.parse_errors.is_empty() {
                bail!(
                    "worktree update could not parse {} files",
                    summary.parse_errors.len()
                );
            }
            // Kept data for these files would be the main checkout's.
            if summary.files_unreadable > 0 || summary.files_deferred > 0 {
                bail!(
                    "worktree update left {} files unread",
                    summary.files_unreadable + summary.files_deferred
                );
            }
            fs::rename(&staged_index, index_dir(&self.worktree.root))
                .context("failed to publish worktree index")?;
            Ok(SeededWorktree {
                root: self.worktree.root.clone(),
                main: self.worktree.main.clone(),
                summary,
            })
        }
        .await;
        result.map_err(|error| self.failure(error))
    }
}

fn copy_index(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let text = name.to_string_lossy();
        // WAL files are data, not locks. Keep them; SQLite regenerates shared memory.
        if text.ends_with(".lock")
            || text.ends_with("-shm")
            || text == "stale-warning.json"
            || text.ends_with(".resume")
            || text.ends_with(".build")
            || text.ends_with(".old")
        {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_index(&entry.path(), &destination.join(name))?;
        } else if kind.is_file() {
            fs::copy(entry.path(), destination.join(name))?;
        } else {
            bail!(
                "unsupported entry in source index: {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::test_helpers::MockProvider;
    use crate::indexing::{index_repository, missing_index_message};
    use crate::storage::{bm25::Bm25Index, metadata::MetadataStore};

    struct Repositories {
        _scratch: tempfile::TempDir,
        main: PathBuf,
        worktree: PathBuf,
    }

    fn repositories() -> Option<Repositories> {
        if !Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("skipping linked-worktree test: git is unavailable");
            return None;
        }
        let base = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
        fs::create_dir_all(&base).unwrap();
        let scratch = tempfile::tempdir_in(base).unwrap();
        let main = scratch.path().join("main");
        let worktree = scratch.path().join("lane");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]).unwrap();
        fs::write(main.join("deleted.rs"), "pub fn main_only_canary() {}\n").unwrap();
        fs::write(main.join("changed.rs"), "pub fn original_version() {}\n").unwrap();
        git(&main, &["add", "."]).unwrap();
        git(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "initial",
            ],
        )
        .unwrap();
        git(
            &main,
            &["worktree", "add", "-qb", "lane", worktree.to_str().unwrap()],
        )
        .unwrap();
        Some(Repositories {
            _scratch: scratch,
            main,
            worktree,
        })
    }

    async fn indexed_main(repo: &Repositories) -> (MockProvider, VeraConfig) {
        let provider = MockProvider::new(8);
        let config = VeraConfig::default();
        index_repository(&repo.main, &provider, &config, "mock-model")
            .await
            .unwrap();
        (provider, config)
    }

    #[tokio::test]
    async fn linked_worktree_search_and_lookup_reflect_added_and_deleted_files() {
        let Some(repo) = repositories() else { return };
        let (provider, config) = indexed_main(&repo).await;
        fs::remove_file(repo.worktree.join("deleted.rs")).unwrap();
        fs::write(
            repo.worktree.join("added.rs"),
            "pub fn worktree_only_canary() {}\n",
        )
        .unwrap();
        git(&repo.worktree, &["add", "."]).unwrap();
        git(
            &repo.worktree,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "worktree changes",
            ],
        )
        .unwrap();
        let nested = repo.worktree.join("src/nested");
        fs::create_dir_all(&nested).unwrap();
        let seed = WorktreeSeed::prepare(&nested).unwrap().unwrap();
        assert!(
            !index_exists(&repo.worktree),
            "unrefreshed seed must not be visible"
        );
        let seeded = seed
            .update(Some(&provider), &config, "mock-model")
            .await
            .unwrap();
        assert_eq!(seeded.root, repo.worktree);
        assert_eq!(seeded.summary.files_added, 1);
        assert_eq!(seeded.summary.files_deleted, 1);
        assert_eq!(find_index_root(&nested), Some(repo.worktree.clone()));
        let index = index_dir(&repo.worktree);
        let bm25 = Bm25Index::open(&index.join("bm25")).unwrap();
        assert!(bm25.search("main_only_canary", 10).unwrap().is_empty());
        assert!(!bm25.search("worktree_only_canary", 10).unwrap().is_empty());
        let metadata = MetadataStore::open_existing(&index.join("metadata.db")).unwrap();
        assert!(
            metadata
                .get_chunks_by_file("deleted.rs")
                .unwrap()
                .is_empty()
        );
        assert!(
            metadata.get_chunks_by_file("added.rs").unwrap()[0]
                .content
                .contains("worktree_only_canary")
        );
        assert!(
            metadata
                .indexed_files()
                .unwrap()
                .iter()
                .all(|path| !Path::new(path).is_absolute())
        );
        assert!(
            !git(
                &repo.worktree,
                &["status", "--porcelain", "--untracked-files=all"]
            )
            .unwrap()
            .contains(".vera")
        );
        let main_bm25 = Bm25Index::open(&index_dir(&repo.main).join("bm25")).unwrap();
        assert!(!main_bm25.search("main_only_canary", 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn linked_worktree_changed_content_replaces_main_version() {
        let Some(repo) = repositories() else { return };
        let (provider, config) = indexed_main(&repo).await;
        fs::write(
            repo.worktree.join("changed.rs"),
            "pub fn worktree_version() {}\n",
        )
        .unwrap();
        let seeded = WorktreeSeed::prepare(&repo.worktree)
            .unwrap()
            .unwrap()
            .update(Some(&provider), &config, "mock-model")
            .await
            .unwrap();
        assert_eq!(seeded.summary.files_modified, 1);
        assert_eq!(seeded.summary.files_unchanged, 1);
        let bm25 = Bm25Index::open(&index_dir(&repo.worktree).join("bm25")).unwrap();
        assert!(bm25.search("original_version", 10).unwrap().is_empty());
        assert!(!bm25.search("worktree_version", 10).unwrap().is_empty());
    }

    #[test]
    fn no_index_preserves_existing_behavior_and_message() {
        let Some(repo) = repositories() else { return };
        assert!(WorktreeSeed::prepare(&repo.main).unwrap().is_none());
        assert!(WorktreeSeed::prepare(&repo.worktree).unwrap().is_none());
        assert_eq!(
            missing_index_message(&repo.main),
            format!(
                "no index found in {} or any parent directory. Run `vera index .` from the repository root (a first index can take minutes), or search with rg until it exists.",
                repo.main.display()
            )
        );
        assert!(missing_index_message(&repo.worktree).contains("This is a linked git worktree"));
        assert!(!index_exists(&repo.worktree));
    }

    #[tokio::test]
    async fn update_failure_removes_seeded_copy() {
        let Some(repo) = repositories() else { return };
        let (provider, config) = indexed_main(&repo).await;
        let seed = WorktreeSeed::prepare(&repo.worktree).unwrap().unwrap();
        let staging = seed.staging.0.clone();
        let error = seed
            .update(Some(&provider), &config, "wrong-model")
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("Index was created with model"), "{error}");
        assert!(error.contains(&index_dir(&repo.main).display().to_string()));
        assert!(error.contains("vera index ."));
        assert!(!staging.exists());
        assert!(!index_dir(&repo.worktree).exists());
        assert!(index_exists(&repo.main));
    }

    #[tokio::test]
    async fn worktree_nested_in_indexed_main_gets_its_own_index() {
        let Some(repo) = repositories() else { return };
        let (provider, config) = indexed_main(&repo).await;
        let nested = repo.main.join(".worktrees/nested-lane");
        git(
            &repo.main,
            &["worktree", "add", "-qb", "nested", nested.to_str().unwrap()],
        )
        .unwrap();
        fs::write(nested.join("changed.rs"), "pub fn nested_version() {}\n").unwrap();
        assert_eq!(find_index_root(&nested), None);
        assert_eq!(find_index_root(&repo.main), Some(repo.main.clone()));
        WorktreeSeed::prepare(&nested)
            .unwrap()
            .unwrap()
            .update(Some(&provider), &config, "mock-model")
            .await
            .unwrap();
        assert_eq!(find_index_root(&nested), Some(nested.clone()));
        let bm25 = Bm25Index::open(&index_dir(&nested).join("bm25")).unwrap();
        assert!(bm25.search("original_version", 10).unwrap().is_empty());
        assert!(!bm25.search("nested_version", 10).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_worktree_file_fails_the_seed() {
        use std::os::unix::fs::PermissionsExt;
        let Some(repo) = repositories() else { return };
        let (provider, config) = indexed_main(&repo).await;
        let unreadable = repo.worktree.join("changed.rs");
        fs::write(&unreadable, "pub fn worktree_version() {}\n").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&unreadable).is_ok() {
            eprintln!("skipping unreadable-file test: permissions are not enforced");
            return;
        }
        let error = WorktreeSeed::prepare(&repo.worktree)
            .unwrap()
            .unwrap()
            .update(Some(&provider), &config, "mock-model")
            .await
            .err()
            .unwrap()
            .to_string();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(error.contains("left 1 files unread"), "{error}");
        assert!(!index_dir(&repo.worktree).exists());
    }

    #[tokio::test]
    async fn keyword_worktree_seed_does_not_create_an_embedding_provider() {
        let Some(repo) = repositories() else { return };
        let config = VeraConfig::default();
        super::super::pipeline::index_repository_with_progress_and_cancellation(
            &repo.main,
            None::<&MockProvider>,
            &config,
            "",
            |_| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        fs::write(
            repo.worktree.join("changed.rs"),
            "pub fn keyword_worktree() {}\n",
        )
        .unwrap();
        WorktreeSeed::prepare(&repo.worktree)
            .unwrap()
            .unwrap()
            .update_configured(&config, InferenceBackend::Api)
            .await
            .unwrap();
        assert!(super::super::is_full_text_only(&index_dir(&repo.worktree)).unwrap());
    }
}
