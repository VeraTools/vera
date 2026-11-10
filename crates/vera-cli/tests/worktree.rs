use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[test]
fn worktree_cli_search_and_lookup_bootstrap_from_indexed_main() {
    if !Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("skipping worktree CLI test: git is unavailable");
        return;
    }
    let scratch = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
    std::fs::create_dir_all(&scratch).unwrap();
    let dir = tempfile::tempdir_in(scratch).unwrap();
    let main = dir.path().join("main");
    let lane = dir.path().join("lane");
    std::fs::create_dir(&main).unwrap();
    let git = |root: &Path, args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&main, &["init", "-q"]);
    std::fs::write(main.join("lib.rs"), "pub fn main_version() {}\n").unwrap();
    std::fs::write(main.join("deleted.rs"), "pub fn main_only() {}\n").unwrap();
    git(&main, &["add", "."]);
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
    );
    let run = |root: &Path, args: &[&str]| -> Output {
        Command::new(env!("CARGO_BIN_EXE_vera"))
            .current_dir(root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir.path().join("home"))
            .env("VERA_HOME", dir.path().join("vera-home"))
            .env("TMPDIR", dir.path())
            .env("VERA_BACKEND", "api")
            .env("VERA_NO_UPDATE_CHECK", "1")
            .args(args)
            .output()
            .unwrap()
    };
    let vera = |root: &Path, args: &[&str]| -> Output {
        let output = run(root, args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    vera(&main, &["index", ".", "--full-text-only", "--json"]);
    git(
        &main,
        &["worktree", "add", "-qb", "lane", lane.to_str().unwrap()],
    );
    std::fs::write(lane.join("lib.rs"), "pub fn lane_version() {}\n").unwrap();
    std::fs::write(lane.join("added.rs"), "pub fn lane_only() {}\n").unwrap();
    std::fs::remove_file(lane.join("deleted.rs")).unwrap();
    let nested = lane.join("src/nested");
    std::fs::create_dir_all(&nested).unwrap();
    let output = vera(&nested, &["search", "lane_only", "--json"]);
    let results: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(results.to_string().contains("lane_only"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("seeded this worktree"));
    assert!(lane.join(".vera/metadata.db").is_file());
    for query in ["main_only", "main_version"] {
        let output = vera(&nested, &["search", query, "--json"]);
        let results: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(results, serde_json::json!([]));
    }
    let output = vera(
        &nested,
        &["structural", "definitions", "lane_version", "--json"],
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("lane_version"));

    // A seed made by `vera update` honors that command's own exclusions.
    let private_lane = dir.path().join("private-lane");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-qb",
            "private",
            private_lane.to_str().unwrap(),
        ],
    );
    std::fs::write(
        private_lane.join("private.rs"),
        "pub fn private_only() {}\n",
    )
    .unwrap();
    // Two excluded additions against one excluded inherited file: a seed that
    // ignored the exclusions would report two changes, not one.
    std::fs::write(private_lane.join("extra.rs"), "pub fn extra_only() {}\n").unwrap();
    let capped = run(&private_lane, &["update", ".", "--max-files", "1"]);
    assert!(!capped.status.success());
    assert!(String::from_utf8_lossy(&capped.stderr).contains("--max-files cannot limit"));
    assert!(!private_lane.join(".vera").exists());
    let output = vera(
        &private_lane,
        &[
            "update",
            ".",
            "--exclude",
            "private.rs",
            "--exclude",
            "lib.rs",
            "--exclude",
            "extra.rs",
        ],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("updated 1 changed files"), "{stderr}");
    // Neither a new file nor one inherited from the main index survives exclusion.
    for query in ["private_only", "main_version"] {
        let output = vera(&private_lane, &["search", query, "--json"]);
        let results: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(results, serde_json::json!([]));
    }
}
