//! CLI handlers for `vera references` and `vera dead-code`.

use anyhow::Result;

use crate::helpers::{
    SearchFilterArgs, apply_git_scope, output_results, prepare_indexed_repo,
    rewrite_absolute_path_filters,
};
use crate::state;

/// Run the `vera references <symbol>` command.
#[allow(
    clippy::too_many_arguments,
    reason = "References accepts the existing command dispatch parameters for filters and output"
)]
pub fn run(
    symbol: &str,
    callees: bool,
    receiver: Option<&str>,
    path: Vec<String>,
    limit: Option<usize>,
    git_scope: Option<vera_core::git_scope::GitScope>,
    json: bool,
    raw: bool,
    compact: bool,
) -> Result<()> {
    let config = state::load_runtime_config()?;
    let result_limit = limit.unwrap_or(20);
    let (cwd, index_dir) = prepare_indexed_repo(&config.indexing)?;
    let mut filters = apply_git_scope(
        &cwd,
        &SearchFilterArgs {
            path,
            scope: Some("source".into()),
            ..Default::default()
        }
        .to_filters(),
        git_scope.as_ref(),
    )?;
    rewrite_absolute_path_filters(&cwd, &mut filters.path_glob);

    if callees {
        let mut results = vera_core::stats::find_callees(&cwd, symbol)?;
        results.retain(|result| {
            filters.matches_file(&result.file_path, vera_core::types::Language::Unknown)
        });
        results.truncate(result_limit);
        if json {
            println!("{}", serde_json::to_string(&results)?);
        } else if results.is_empty() {
            println!("No callees found for '{symbol}'.");
        } else {
            println!(
                "Symbols called by '{symbol}' ({} results):\n",
                results.len()
            );
            for r in &results {
                println!("  {}:{} → {}", r.file_path, r.line, r.callee);
            }
        }
    } else {
        let results = vera_core::retrieval::search_callers_through(
            &index_dir,
            symbol,
            receiver,
            result_limit,
            &filters,
        )?;
        if results.is_empty() && !json && !raw {
            println!("No callers found for '{symbol}'.");
        } else {
            output_results(
                &results,
                json,
                raw,
                compact,
                config.retrieval.max_output_chars,
            );
            if receiver.is_none() && !json {
                print_receiver_hint(&index_dir, symbol)?;
            }
        }
    }
    Ok(())
}

/// Report the receivers these call sites went through when more than one is
/// present.
///
/// Callers are matched by name, so `state.add_url_rule()` and
/// `app.add_url_rule()` land in the same answer even though they reach
/// different definitions. Naming the receivers makes that ambiguity visible
/// and points at `--receiver` for narrowing it.
fn print_receiver_hint(index_dir: &std::path::Path, symbol: &str) -> Result<()> {
    let store = vera_core::storage::metadata::MetadataStore::open(&index_dir.join("metadata.db"))?;
    let receivers = store.caller_qualifiers(symbol)?;
    if receivers.len() < 2 {
        return Ok(());
    }
    let listed: Vec<String> = receivers
        .iter()
        .take(5)
        .map(|(name, count)| format!("{name} ({count})"))
        .collect();
    println!(
        "\nCalled through {} receivers: {}. Narrow with --receiver <name>.",
        receivers.len(),
        listed.join(", ")
    );
    Ok(())
}

/// Run the `vera dead-code` command.
pub fn run_dead_code(json: bool) -> Result<()> {
    let config = state::load_runtime_config()?;
    let (cwd, _) = prepare_indexed_repo(&config.indexing)?;
    let results = vera_core::stats::find_dead_symbols(&cwd)?;

    if json {
        println!("{}", serde_json::to_string(&results)?);
    } else if results.is_empty() {
        println!("No dead code found.");
    } else {
        println!("Potentially unused symbols ({} results):\n", results.len());
        for r in &results {
            let stype = r.symbol_type.as_deref().unwrap_or("symbol");
            println!("  {}:{} {} {}", r.file_path, r.line, stype, r.symbol_name);
        }
    }
    Ok(())
}
