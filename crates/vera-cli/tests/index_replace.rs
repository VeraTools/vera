use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;
use vera_core::indexing::freshness::INDEX_COMPLETE_KEY;
use vera_core::storage::metadata::MetadataStore;

fn scratch_tempdir() -> TempDir {
    if let Some(home) = std::env::var_os("HOME") {
        let scratch = PathBuf::from(home).join(".local/tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        tempfile::tempdir_in(scratch).unwrap()
    } else {
        tempfile::tempdir().unwrap()
    }
}

fn run(repo: &Path, temp: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vera"))
        .current_dir(repo)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", temp.path().join("home"))
        .env("VERA_HOME", temp.path().join("vera-home"))
        .env("TMPDIR", temp.path())
        .env("VERA_NO_UPDATE_CHECK", "1")
        .env("VERA_BACKEND", "api")
        .env("VERA_LOG", "warn")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn existing_usable_index_requires_replace_before_provider_init_and_dry_run_still_runs() {
    let temp = scratch_tempdir();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("main.rs"), "pub fn indexed_marker() {}\n").unwrap();

    let initial = run(&repo, &temp, &["index", ".", "--full-text-only", "--json"]);
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );

    let sentinel = repo.join(".vera/sentinel");
    std::fs::write(&sentinel, "preserve until explicit replace").unwrap();

    let dry_run = run(
        &repo,
        &temp,
        &["index", ".", "--full-text-only", "--dry-run", "--json"],
    );
    assert!(
        dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert!(sentinel.exists());

    // API backend has no EMBEDDING_MODEL_* env vars set here: refusal must
    // fire during preflight before provider creation runs.
    let refused_api = run(&repo, &temp, &["index", ".", "--json"]);
    assert!(!refused_api.status.success());
    assert!(refused_api.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&refused_api.stderr);
    assert!(stderr.contains("vera update"), "{stderr}");
    assert!(stderr.contains("--replace"), "{stderr}");
    assert!(sentinel.exists());

    let refused_ft = run(&repo, &temp, &["index", ".", "--full-text-only", "--json"]);
    assert!(!refused_ft.status.success());
    assert!(refused_ft.stdout.is_empty());
    assert!(sentinel.exists());

    let replaced = run(
        &repo,
        &temp,
        &["index", ".", "--full-text-only", "--replace", "--json"],
    );
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert!(!sentinel.exists());
}

#[test]
fn stray_and_incomplete_indexes_rebuild_without_replace() {
    let temp = scratch_tempdir();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".vera")).unwrap();
    std::fs::write(repo.join(".vera/stray"), "not an index").unwrap();
    std::fs::write(repo.join("main.rs"), "pub fn rebuilt_symbol() {}\n").unwrap();

    let built = run(&repo, &temp, &["index", ".", "--full-text-only", "--json"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(!repo.join(".vera/stray").exists());

    let store = MetadataStore::open(&repo.join(".vera/metadata.db")).unwrap();
    store.set_index_meta(INDEX_COMPLETE_KEY, "0").unwrap();
    drop(store);

    let repaired = run(&repo, &temp, &["index", ".", "--full-text-only", "--json"]);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    let store = MetadataStore::open_existing(&repo.join(".vera/metadata.db")).unwrap();
    assert_eq!(
        store.get_index_meta(INDEX_COMPLETE_KEY).unwrap().as_deref(),
        Some("1")
    );
}

#[test]
fn cli_preflight_recovers_old_index_before_refusing_and_allows_update() {
    let temp = scratch_tempdir();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("main.rs"), "pub fn recoverable_symbol() {}\n").unwrap();

    let built = run(&repo, &temp, &["index", ".", "--full-text-only", "--json"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let idx_dir = repo.join(".vera");
    let old_dir = repo.join(".vera.old");
    std::fs::write(idx_dir.join("sentinel"), "preserve recovered").unwrap();
    std::fs::rename(&idx_dir, &old_dir).unwrap();

    let refused = run(&repo, &temp, &["index", ".", "--full-text-only", "--json"]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("--replace"), "{stderr}");
    assert!(!old_dir.exists());
    assert_eq!(
        std::fs::read_to_string(idx_dir.join("sentinel")).unwrap(),
        "preserve recovered"
    );

    std::fs::write(repo.join("main.rs"), "pub fn updated_after_recovery() {}\n").unwrap();
    let updated = run(&repo, &temp, &["update", ".", "--json"]);
    assert!(
        updated.status.success(),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
}
