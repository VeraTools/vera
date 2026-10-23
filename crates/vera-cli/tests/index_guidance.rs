use std::path::Path;
use std::process::{Command, Output};

use vera_core::config::VeraConfig;
use vera_core::embedding::{EmbeddingError, EmbeddingProvider};

fn vera(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vera"))
        .current_dir(root)
        .env("VERA_HOME", root.join("vera-home"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn commands_share_no_index_guidance_and_resume_hint() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::create_dir(root.join(".vera.resume")).unwrap();
    std::fs::write(root.join(".vera.resume/embeddings.db"), []).unwrap();
    for args in [
        vec!["search", "foo"],
        vec!["grep", "foo"],
        vec!["structural", "definitions", "foo"],
        vec!["references", "foo"],
        vec!["overview"],
        vec!["update", "."],
        vec!["stats"],
    ] {
        let output = vera(&root, &args);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(
            stderr.contains(&vera_core::indexing::missing_index_message(&root)),
            "{args:?}: {stderr}"
        );
    }
    assert!(vera(&root, &["explain-path", "foo.rs"]).status.success());
}

struct TestEmbeddings;

impl EmbeddingProvider for TestEmbeddings {
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        Ok(vec![vec![0.5; 8]; texts.len()])
    }

    fn expected_dim(&self) -> Option<usize> {
        Some(8)
    }
}

#[tokio::test]
async fn references_filter_callers_and_callees_with_search_path_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for path in ["src/first.rs", "other/second.rs", "tests/third.rs"] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "fn caller() { target(); }\nfn target() {}\n").unwrap();
    }
    vera_core::indexing::index_repository(root, &TestEmbeddings, &VeraConfig::default(), "test")
        .await
        .unwrap();
    for callees in [false, true] {
        for patterns in [
            vec!["src/**/*.rs".to_string(), "tests".to_string()],
            vec![
                root.join("src").to_string_lossy().into_owned(),
                "tests".to_string(),
            ],
        ] {
            let mut args = vec![
                "references",
                if callees { "caller" } else { "target" },
                "--json",
                "--limit",
                "2",
            ];
            if callees {
                args.push("--callees");
            }
            for pattern in &patterns {
                args.extend(["--path", pattern.as_str()]);
            }
            let output = vera(root, &args);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let results: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
            let mut paths: Vec<_> = results
                .iter()
                .map(|result| result["file_path"].as_str().unwrap())
                .collect();
            paths.sort();
            assert_eq!(paths, ["src/first.rs", "tests/third.rs"]);
        }
    }
    let output = vera(&root.join("src"), &["overview", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let overview: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(overview["file_count"], 3);
}
