//! `vera watch` command: watch a project directory and auto-update the index.

use anyhow::Result;
use std::path::Path;

/// Run the watch command. Blocks until the process is killed (Ctrl-C).
pub fn run(path: &str, json: bool, index_dir: Option<&Path>) -> Result<()> {
    let repo_path = std::path::Path::new(path)
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Failed to resolve path: {e}"))?;

    let _handle = if let Some(index_dir) = index_dir {
        vera_mcp::watcher::start_watching_at_index_dir(&repo_path, index_dir, true)
    } else {
        vera_mcp::watcher::start_watching_with_progress(&repo_path)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": "watching",
                "path": repo_path.display().to_string(),
                "message": "Watching for file changes. Index will auto-update. Press Ctrl-C to stop."
            })
        );
    } else {
        eprintln!(
            "Watching {} for file changes. Index will auto-update. Press Ctrl-C to stop.",
            repo_path.display()
        );
    }

    // Block forever; the watcher runs in background threads.
    // The process exits on Ctrl-C (SIGINT).
    loop {
        std::thread::park();
    }
}
