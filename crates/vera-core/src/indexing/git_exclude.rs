//! Keep index artifacts out of Git without editing tracked ignore rules.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use anyhow::{Result, bail};

const COMMENT: &str = "# Vera index files";
const ARTIFACTS: [&str; 5] = [
    ".vera/",
    ".vera.build/",
    ".vera.old/",
    ".vera.resume/",
    ".vera.lock",
];

pub(super) fn ensure(root: &Path) {
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            tracing::debug!(%error, "could not resolve index root for git excludes");
            return;
        }
    };
    static HANDLED_ROOTS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let mut handled = HANDLED_ROOTS.get_or_init(Mutex::default).lock().unwrap();
    if handled.contains(&root) {
        return;
    }
    if let Err(error) = ensure_excluded(&root) {
        tracing::debug!(%error, "could not exclude Vera index files from git");
    }
    // Failed attempts count too, so watch updates never repeatedly spawn git.
    handled.insert(root);
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root);
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
    ] {
        command.env_remove(variable);
    }
    command
}

fn git_path(root: &Path, args: &[&str]) -> Result<PathBuf> {
    let output = git_command(root).arg("rev-parse").args(args).output()?;
    if !output.status.success() {
        bail!("git rev-parse {args:?} failed");
    }
    Ok(PathBuf::from(
        String::from_utf8(output.stdout)?.trim_end_matches(['\r', '\n']),
    ))
}

fn ensure_excluded(root: &Path) -> Result<()> {
    let top = git_path(root, &["--show-toplevel"])?.canonicalize()?;
    let relative = root
        .strip_prefix(top)?
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if relative.contains(['\n', '\r']) {
        bail!("index root contains line breaks; cannot write git exclude rules");
    }
    // Directory names are literal, not gitignore globs.
    let mut prefix = String::from("/");
    for ch in relative.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '\\' | ' ') {
            prefix.push('\\');
        }
        prefix.push(ch);
    }
    if !relative.is_empty() {
        prefix.push('/');
    }
    let exclude = root.join(git_path(root, &["--git-path", "info/exclude"])?);
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(exclude)?;
    // Linked worktrees can share this file while holding different index locks.
    super::lock::lock_exclusive_blocking(&file)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    let missing: String = ARTIFACTS
        .iter()
        .map(|artifact| format!("{prefix}{artifact}"))
        .filter(|entry| !content.lines().any(|line| line == entry))
        .map(|entry| format!("{entry}\n"))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if let Some(offset) = content
        .split_inclusive('\n')
        .scan(0, |offset, line| {
            *offset += line.len();
            Some((line.trim_end_matches(['\r', '\n']), *offset))
        })
        .find_map(|(line, offset)| (line == COMMENT).then_some(offset))
    {
        content.insert_str(offset, &missing);
    } else {
        content.push_str(&format!("{COMMENT}\n{missing}"));
    }
    file.seek(SeekFrom::Start(0))?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexing::lock::IndexLock;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = git_command(root).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn acquire(root: &Path) {
        drop(IndexLock::acquire_blocking_for_repo(root).unwrap());
    }

    fn assert_ignored(root: &Path) {
        for artifact in ARTIFACTS {
            let path = if artifact.ends_with('/') {
                let dir = root.join(artifact);
                std::fs::create_dir_all(&dir).unwrap();
                dir.join("artifact")
            } else {
                root.join(artifact)
            };
            std::fs::write(&path, []).unwrap();
            git(root, &["check-ignore", path.to_str().unwrap()]);
        }
    }

    #[test]
    fn index_lock_adds_only_missing_excludes_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q"]);
        let exclude = root.join(".git/info/exclude");
        std::fs::write(&exclude, "# user's rules\n/.vera/\nkeep-me").unwrap();
        std::fs::write(root.join(".gitignore"), "user-rule\n").unwrap();
        acquire(root);
        let first = std::fs::read_to_string(&exclude).unwrap();
        assert!(first.starts_with("# user's rules\n/.vera/\nkeep-me\n"));
        for artifact in ARTIFACTS {
            assert_eq!(
                first
                    .lines()
                    .filter(|line| *line == format!("/{artifact}"))
                    .count(),
                1
            );
        }
        assert_eq!(first.matches(COMMENT).count(), 1);
        acquire(root);
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), first);
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            "user-rule\n"
        );
        assert_ignored(root);
    }

    #[test]
    fn inherited_git_environment_does_not_redirect_excludes() {
        let foreign = tempfile::tempdir().unwrap();
        git(foreign.path(), &["init", "-q"]);
        let git_dir = foreign.path().join(".git");
        let exclude = git_dir.join("info/exclude");
        let original = std::fs::read_to_string(&exclude).unwrap();
        crate::test_env::run_env_test(
            "indexing::git_exclude::tests::inherited_git_environment_probe",
            &[
                ("GIT_DIR", Some(git_dir.to_str().unwrap())),
                ("GIT_WORK_TREE", Some(foreign.path().to_str().unwrap())),
                ("GIT_COMMON_DIR", Some(git_dir.to_str().unwrap())),
                (
                    "GIT_INDEX_FILE",
                    Some(git_dir.join("index").to_str().unwrap()),
                ),
                (
                    "GIT_OBJECT_DIRECTORY",
                    Some(git_dir.join("objects").to_str().unwrap()),
                ),
            ],
        );
        assert_eq!(std::fs::read_to_string(exclude).unwrap(), original);
    }

    #[test]
    #[ignore = "driven by inherited_git_environment_does_not_redirect_excludes"]
    fn inherited_git_environment_probe() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        acquire(temp.path());
        assert_ignored(temp.path());
    }

    #[tokio::test]
    async fn linked_worktree_and_nested_index_share_one_exclude_section() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q"]);
        git(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "init",
                "--allow-empty",
            ],
        );
        let worktree = temp.path().join("linked");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-qb",
                "linked",
                worktree.to_str().unwrap(),
            ],
        );
        for file in ["first.rs", "second.rs"] {
            std::fs::write(worktree.join(file), "fn example() {}\n").unwrap();
        }
        let provider = crate::embedding::test_helpers::MockProvider::new(8);
        let config = crate::config::VeraConfig::default();
        crate::indexing::index_repository(&worktree, &provider, &config, "test")
            .await
            .unwrap();
        let exclude = root.join(".git/info/exclude");
        let indexed = std::fs::read_to_string(&exclude).unwrap();
        crate::indexing::update_repository(&worktree, &provider, &config, "test")
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), indexed);
        assert_ignored(&worktree);
        let nested = worktree.join("nested [code]");
        std::fs::create_dir(&nested).unwrap();
        acquire(&nested);
        assert_ignored(&nested);
        let first = std::fs::read_to_string(&exclude).unwrap();
        assert_eq!(first.matches(COMMENT).count(), 1);
        assert!(first.contains("/nested\\ \\[code\\]/.vera.lock\n"));
        acquire(&nested);
        assert_eq!(std::fs::read_to_string(exclude).unwrap(), first);
        assert!(
            !git(
                &worktree,
                &["status", "--porcelain", "--untracked-files=all"]
            )
            .contains(".vera")
        );
    }

    #[test]
    fn exclusion_failure_does_not_fail_index_lock() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        let exclude = temp.path().join(".git/info/exclude");
        std::fs::remove_file(&exclude).unwrap();
        std::fs::create_dir(exclude).unwrap();
        acquire(temp.path());
    }

    #[test]
    fn successful_exclusion_is_not_retried_for_the_same_canonical_root() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        acquire(temp.path());
        let exclude = temp.path().join(".git/info/exclude");
        let edited = "# user replaced exclude rules\n";
        std::fs::write(&exclude, edited).unwrap();
        acquire(&temp.path().join("."));
        assert_eq!(std::fs::read_to_string(exclude).unwrap(), edited);
    }

    #[test]
    fn failed_exclusion_is_not_retried_for_the_same_root() {
        let temp = tempfile::tempdir().unwrap();
        acquire(temp.path());
        git(temp.path(), &["init", "-q"]);
        let exclude = temp.path().join(".git/info/exclude");
        let original = std::fs::read_to_string(&exclude).unwrap();
        acquire(temp.path());
        assert_eq!(std::fs::read_to_string(exclude).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn nested_roots_with_line_breaks_leave_excludes_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        let exclude = temp.path().join(".git/info/exclude");
        let original = std::fs::read_to_string(&exclude).unwrap();
        for name in ["pkg\nname", "pkg\rname"] {
            let nested = temp.path().join(name);
            std::fs::create_dir(&nested).unwrap();
            acquire(&nested);
            assert_eq!(std::fs::read_to_string(&exclude).unwrap(), original);
        }
    }

    #[cfg(unix)]
    #[test]
    fn nested_root_with_literal_backslash_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        let nested = temp.path().join("pkg\\name");
        std::fs::create_dir(&nested).unwrap();
        acquire(&nested);
        assert_ignored(&nested);
    }
}
