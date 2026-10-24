//! `vera watch` command: watch a project directory and auto-update the index.

use anyhow::Result;

/// Run the watch command. Blocks until the process is killed (Ctrl-C).
pub fn run(path: &str, json: bool) -> Result<()> {
    let start = std::path::Path::new(path)
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Failed to resolve path: {e}"))?;
    // Watch the index that covers `path`, like `vera update`, so `vera watch .`
    // from a subdirectory does not watch a directory without its own index.
    let repo_path = crate::helpers::resolve_index_root(&start)?;

    let _handle = vera_mcp::watcher::start_watching_with_progress(&repo_path)
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
