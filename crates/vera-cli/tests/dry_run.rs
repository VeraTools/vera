use std::process::Command;

#[test]
fn dry_run_full_text_only_reports_json_without_writing_an_index() {
    let scratch = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
    std::fs::create_dir_all(&scratch).unwrap();
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let exclude = repo.join(".git/info/exclude");
    std::fs::write(&exclude, "# unchanged by dry run\n").unwrap();
    std::fs::write(repo.join("lib.rs"), "pub fn dry_run_target() {}\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_vera"))
        .current_dir(&repo)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", dir.path().join("home"))
        .env("VERA_HOME", dir.path().join("vera-home"))
        .env("TMPDIR", dir.path())
        .env("VERA_NO_UPDATE_CHECK", "1")
        .env("VERA_BACKEND", "api")
        .env("VERA_LOG", "warn")
        .env("EMBEDDING_MODEL_BASE_URL", "http://127.0.0.1:0/v1")
        .args(["index", ".", "--dry-run", "--full-text-only", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let estimate: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(estimate["path"], ".");
    assert_eq!(estimate["total_files"], 1);
    assert!(estimate["estimated_chunks"].as_u64().unwrap() > 0);
    assert_eq!(
        estimate["estimated_embedding_secs"],
        serde_json::Value::Null
    );
    assert!(std::fs::read_dir(&repo).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".vera")
    }));
    assert_eq!(
        std::fs::read_to_string(exclude).unwrap(),
        "# unchanged by dry run\n"
    );
}
