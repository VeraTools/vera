use std::process::Command;

#[test]
fn full_text_only_commands_need_no_embedding_configuration() {
    let scratch = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
    std::fs::create_dir_all(&scratch).unwrap();
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("lib.rs"),
        "pub fn keyword_target() {}\npub fn caller() { keyword_target(); }\n",
    )
    .unwrap();
    let run_from = |cwd: &std::path::Path, args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_vera"))
            .current_dir(cwd)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir.path().join("home"))
            .env("VERA_HOME", dir.path().join("vera-home"))
            .env("TMPDIR", dir.path())
            .env("VERA_NO_UPDATE_CHECK", "1")
            .env("VERA_BACKEND", "api")
            .env("VERA_LOG", "warn")
            // Missing model and key make provider creation fail, not just requests.
            .env("EMBEDDING_MODEL_BASE_URL", "http://127.0.0.1:0/v1")
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {stderr}",
            String::from_utf8_lossy(&output.stdout),
        );
        assert!(!stderr.contains("BM25-only"), "{stderr}");
        assert!(!stderr.contains("embedding provider"), "{stderr}");
        output
    };
    let run = |args: &[&str]| run_from(&repo, args);
    let output = run(&["index", ".", "--full-text-only", "--json"]);
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["embeddings_generated"], 0);
    assert_eq!(summary["embedding_requests"], 0);
    for args in [
        vec!["-C", repo.to_str().unwrap(), "grep", "keyword_target"],
        vec!["-C", "repo", "grep", "keyword_target", "-C", "1"],
        vec!["--directory", "repo", "grep", "keyword_target", "-C", "3"],
    ] {
        let output = run_from(dir.path(), &args);
        assert!(String::from_utf8_lossy(&output.stdout).contains("keyword_target"));
    }
    assert!(
        String::from_utf8_lossy(&run(&["grep", "keyword_target", "-C", "0"]).stdout)
            .contains("keyword_target")
    );
    assert!(
        String::from_utf8_lossy(&run(&["search", "keyword_target"]).stdout)
            .contains("keyword_target")
    );
    for args in [
        vec!["grep", "keyword_target"],
        vec!["structural", "definitions", "keyword_target"],
        vec!["references", "keyword_target"],
        vec!["overview"],
    ] {
        assert!(!run(&args).stdout.is_empty(), "{args:?}");
    }
    std::fs::write(repo.join("lib.rs"), "pub fn changed_keyword() {}\n").unwrap();
    std::fs::write(repo.join("new.rs"), "pub fn added_keyword() {}\n").unwrap();
    let output = run(&["update", ".", "--json"]);
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["files_modified"], 1);
    assert_eq!(summary["files_added"], 1);
    assert_eq!(summary["embedding_requests"], 0);
    for query in ["changed_keyword", "added_keyword"] {
        assert!(String::from_utf8_lossy(&run(&["search", query]).stdout).contains(query));
    }
    assert!(!repo.join(".vera/vectors.db").exists());
    let stats = run(&["stats"]);
    assert!(String::from_utf8_lossy(&stats.stdout).contains("full-text only"));
    let summary = run(&["index", ".", "--full-text-only"]);
    let summary = String::from_utf8_lossy(&summary.stdout);
    assert!(summary.contains("full-text only"));
    assert!(!summary.contains("Embeddings generated"));
}
