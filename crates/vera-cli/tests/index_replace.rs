use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

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
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn existing_index_requires_replace_and_dry_run_does_not_replace() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("main.rs"), "pub fn indexed() {}\n").unwrap();

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

    let refused = run(&repo, &temp, &["index", ".", "--full-text-only", "--json"]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("Refusing to replace it"), "{stderr}");
    assert!(stderr.contains("--replace"), "{stderr}");
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
