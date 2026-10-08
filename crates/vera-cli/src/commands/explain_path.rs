//! `vera explain-path <path>` — Explain why a path is or is not indexed.

use std::path::Path;

use crate::state;

/// Run the `vera explain-path <path>` command.
pub fn run(
    path: &str,
    json_output: bool,
    exclude: Vec<String>,
    no_ignore: bool,
    no_default_excludes: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("failed to get current directory: {e}"))?;

    let mut config = state::load_runtime_config()?;
    config.indexing.extra_excludes.extend(exclude);
    config.indexing.no_ignore |= no_ignore;
    config.indexing.no_default_excludes |= no_default_excludes;

    let explanation = vera_core::discovery::explain_path(&cwd, Path::new(path), &config.indexing)?;

    if json_output {
        let json = serde_json::to_string_pretty(&explanation)
            .map_err(|e| anyhow::anyhow!("failed to serialize explanation: {e}"))?;
        println!("{json}");
    } else {
        print_human_explanation(&explanation);
    }

    Ok(())
}

fn print_human_explanation(explanation: &vera_core::discovery::PathExplanation) {
    println!("Path Explanation");
    println!();
    println!("  Input:     {}", explanation.input_path);
    println!("  Resolved:  {}", explanation.absolute_path);
    if let Some(relative) = &explanation.relative_path {
        println!("  Relative:  {relative}");
    }
    println!("  Decision:  {}", explanation.decision);
    println!("  Reason:    {}", explanation.reason);
    if let Some(source) = &explanation.source {
        println!("  Source:    {source}");
    }
    if let Some(pattern) = &explanation.pattern {
        println!("  Pattern:   {pattern}");
    }
    if let Some(details) = &explanation.details {
        println!();
        println!("  {details}");
    }
}
