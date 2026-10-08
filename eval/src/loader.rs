//! Task and corpus loading for the evaluation harness.
//!
//! Loads benchmark tasks from JSON files in the tasks/ directory and
//! parses the corpus manifest from corpus.toml.

use anyhow::{Context, Result};
use std::path::Path;

use crate::types::{BenchmarkTask, CorpusManifest};

/// Load all benchmark tasks from a directory.
///
/// Reads all `.json` files in the directory, parsing each as a `BenchmarkTask`.
/// Returns tasks sorted by ID for deterministic ordering.
pub fn load_tasks(tasks_dir: &Path) -> Result<Vec<BenchmarkTask>> {
    if !tasks_dir.exists() {
        anyhow::bail!("Tasks directory not found: {}", tasks_dir.display());
    }

    let mut tasks = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(tasks_dir)
        .with_context(|| format!("Failed to read tasks directory: {}", tasks_dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| {
            format!(
                "Failed to enumerate tasks directory: {}",
                tasks_dir.display()
            )
        })?;
    entries.retain(|entry| entry.path().extension().is_some_and(|ext| ext == "json"));

    // Sort entries by filename for deterministic ordering
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read task file: {}", path.display()))?;

        // Support both single task and array of tasks per file
        if content.trim_start().starts_with('[') {
            let batch: Vec<BenchmarkTask> = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse task file: {}", path.display()))?;
            tasks.extend(batch);
        } else {
            let task: BenchmarkTask = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse task file: {}", path.display()))?;
            tasks.push(task);
        }
    }

    // Sort by task ID for deterministic ordering
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tasks)
}

/// Load the corpus manifest from a TOML file.
pub fn load_corpus(corpus_path: &Path) -> Result<CorpusManifest> {
    let content = std::fs::read_to_string(corpus_path)
        .with_context(|| format!("Failed to read corpus manifest: {}", corpus_path.display()))?;
    let manifest: CorpusManifest = toml::from_str(&content)
        .with_context(|| format!("Failed to parse corpus manifest: {}", corpus_path.display()))?;
    Ok(manifest)
}

/// Verify that all corpus repos are cloned at the correct SHAs.
pub fn verify_corpus(manifest: &CorpusManifest, repo_root: &Path) -> Result<Vec<String>> {
    let clone_root = repo_root.join(&manifest.corpus.clone_root);
    let mut issues = Vec::new();

    for repo in &manifest.repos {
        let repo_dir = clone_root.join(&repo.name);
        if !repo_dir.exists() {
            issues.push(format!(
                "Repo '{}' not cloned at {}",
                repo.name,
                repo_dir.display()
            ));
            continue;
        }

        let git_dir = repo_dir.join(".git");
        if !git_dir.exists() {
            issues.push(format!("Repo '{}' exists but is not a git repo", repo.name));
            continue;
        }

        // Check current HEAD SHA
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_dir)
            .output()
            .with_context(|| format!("Failed to run git in {}", repo_dir.display()))?;

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if sha != repo.commit {
            issues.push(format!(
                "Repo '{}' at SHA {} (expected {})",
                repo.name, sha, repo.commit
            ));
        }
    }

    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_load_single_task() {
        let dir = tempfile::tempdir().unwrap();
        let task_json = r#"{
            "id": "test-001",
            "query": "find main function",
            "category": "symbol_lookup",
            "repo": "ripgrep",
            "ground_truth": [
                {"file_path": "crates/core/main.rs", "line_start": 1, "line_end": 50}
            ],
            "description": "Test task"
        }"#;
        fs::write(dir.path().join("test-001.json"), task_json).unwrap();

        let tasks = load_tasks(dir.path()).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "test-001");
        assert_eq!(tasks[0].ground_truth.len(), 1);
    }

    #[test]
    fn test_load_task_array() {
        let dir = tempfile::tempdir().unwrap();
        let tasks_json = r#"[
            {
                "id": "test-001",
                "query": "find main",
                "category": "symbol_lookup",
                "repo": "ripgrep",
                "ground_truth": [
                    {"file_path": "main.rs", "line_start": 1, "line_end": 10}
                ]
            },
            {
                "id": "test-002",
                "query": "error handling",
                "category": "intent",
                "repo": "flask",
                "ground_truth": [
                    {"file_path": "app.py", "line_start": 100, "line_end": 150}
                ]
            }
        ]"#;
        fs::write(dir.path().join("batch.json"), tasks_json).unwrap();

        let tasks = load_tasks(dir.path()).unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_load_tasks_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["z-task", "a-task", "m-task"] {
            let json = format!(
                r#"{{"id":"{id}","query":"q","category":"intent","repo":"r","ground_truth":[]}}"#
            );
            fs::write(dir.path().join(format!("{id}.json")), json).unwrap();
        }

        let tasks = load_tasks(dir.path()).unwrap();
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a-task", "m-task", "z-task"]);
    }

    #[test]
    fn test_load_tasks_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = load_tasks(dir.path()).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_load_tasks_missing_dir() {
        let result = load_tasks(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_corpus() {
        let dir = tempfile::tempdir().unwrap();
        let corpus_toml = r#"
[corpus]
version = 1
description = "Test corpus"
clone_root = ".bench/repos"

[[repos]]
name = "test-repo"
url = "https://github.com/test/repo.git"
commit = "abc123"
language = "Rust"
description = "A test repo"
"#;
        let path = dir.path().join("corpus.toml");
        fs::write(&path, corpus_toml).unwrap();

        let manifest = load_corpus(&path).unwrap();
        assert_eq!(manifest.corpus.version, 1);
        assert_eq!(manifest.repos.len(), 1);
        assert_eq!(manifest.repos[0].name, "test-repo");
        assert!(manifest.semble.is_none());
    }

    #[test]
    fn test_load_corpus_with_semble_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let corpus_toml = r#"
[corpus]
version = 1
description = "Semble corpus"
clone_root = ".bench/repos"

[semble]
version = "0.5.5"
commit = "921849164e2632dd4f0e1c1370f82cfe15ed6d6c"
task_count = 1251
tasks_sha256 = "6d8befaa3c19211ef6c3df7d0aa7bb1711670ed32f9b0258c289909cdb5b58de"

[[repos]]
name = "test-repo"
url = "https://github.com/test/repo.git"
commit = "abc123"
language = "Rust"
description = "A test repo"
"#;
        let path = dir.path().join("corpus.toml");
        fs::write(&path, corpus_toml).unwrap();

        let snapshot = load_corpus(&path).unwrap().semble.unwrap();
        assert_eq!(snapshot.version, "0.5.5");
        assert_eq!(snapshot.task_count, 1251);
    }
}
