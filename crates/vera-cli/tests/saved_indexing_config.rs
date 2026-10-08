//! Saved indexing settings survive CLI runs that do not pass the matching flags.

use std::path::Path;
use std::process::Command;

fn vera(root: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_vera"))
        .current_dir(root.join("repo"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", root.join("home"))
        .env("VERA_HOME", root.join("vera-home"))
        .env("VERA_NO_UPDATE_CHECK", "1")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null)
}

fn decision(root: &Path, path: &str, extra: &[&str]) -> String {
    let mut args = vec!["explain-path", path, "--json"];
    args.extend_from_slice(extra);
    vera(root, &args)["decision"].as_str().unwrap().to_string()
}

#[test]
fn saved_ignore_settings_apply_without_flags_and_exclude_flags_add_to_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("thirdparty")).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    std::fs::write(repo.join(".gitignore"), "ignored.rs\n").unwrap();
    std::fs::write(repo.join("ignored.rs"), "fn ignored() {}\n").unwrap();
    std::fs::write(repo.join("thirdparty/lib.rs"), "fn vendored() {}\n").unwrap();
    std::fs::write(repo.join("extra.rs"), "fn extra() {}\n").unwrap();
    assert_eq!(decision(root, "ignored.rs", &[]), "excluded");

    vera(root, &["config", "set", "indexing.no_ignore", "true"]);
    vera(
        root,
        &[
            "config",
            "set",
            "indexing.extra_excludes",
            r#"["thirdparty/**"]"#,
        ],
    );

    assert_eq!(decision(root, "ignored.rs", &[]), "indexed");
    assert_eq!(decision(root, "thirdparty/lib.rs", &[]), "excluded");
    assert_eq!(
        decision(root, "thirdparty/lib.rs", &["--exclude", "extra.rs"]),
        "excluded"
    );
    assert_eq!(
        decision(root, "extra.rs", &["--exclude", "extra.rs"]),
        "excluded"
    );
}
