//! Tool dispatch, argument validation, retrieval, and result formatting.

use std::sync::Arc;

use serde_json::Value;
use vera_core::presentation::{CompactResult, truncate_to_budget};

use crate::protocol::ToolCallResult;

use super::runtime::{cached_search_context, ensure_index_and_watcher, search_runtime};

/// Default total output budget for MCP responses (chars).
const MCP_OUTPUT_BUDGET: usize = 20_000;

/// Serialize search results as compact JSON, applying a total character budget.
/// When `signatures_only` is true, function/class bodies are stripped before output.
fn compact_results_json(
    results: &[vera_core::types::SearchResult],
    budget: usize,
    signatures_only: bool,
) -> Result<String, serde_json::Error> {
    use vera_core::parsing::signatures::extract_signature_for_path;

    let signatures: Vec<String> = if signatures_only {
        results
            .iter()
            .map(|r| extract_signature_for_path(&r.content, r.language, &r.file_path))
            .collect()
    } else {
        Vec::new()
    };

    // Build a parallel vec of display content (signature or original).
    let display: Vec<&str> = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            if signatures_only {
                signatures[i].as_str()
            } else {
                r.content.as_str()
            }
        })
        .collect();

    let mut remaining = budget;
    let mut compact: Vec<CompactResult> = Vec::with_capacity(results.len());
    for (i, r) in results.iter().enumerate() {
        if budget > 0 && remaining == 0 {
            break;
        }
        let content = if budget > 0 {
            let c = truncate_to_budget(display[i], remaining);
            remaining = remaining.saturating_sub(c.len());
            c
        } else {
            std::borrow::Cow::Borrowed(display[i])
        };
        compact.push(CompactResult {
            file_path: &r.file_path,
            line_start: r.line_start,
            line_end: r.line_end,
            content,
            symbol_name: r.symbol_name.as_deref(),
            symbol_type: r.symbol_type.as_ref(),
            part_index: r.part_index,
        });
    }
    serde_json::to_string(&compact)
}

/// Wrap serialized results in a tool result, attaching a stale-index notice
/// when the working tree has drifted from the index.
///
/// The notice rides in its own content block, so it stays outside
/// `MCP_OUTPUT_BUDGET` and cannot silently evict results.
fn results_with_staleness(cwd: &std::path::Path, json: String) -> ToolCallResult {
    ToolCallResult::success_with_notice(json, crate::staleness::notice_for_repo(cwd))
}

/// Dispatch a tool call to the appropriate handler.
///
/// Returns a `ToolCallResult` — either success with JSON content or an
/// error with a descriptive message. This function never panics.
pub fn handle_tool_call(name: &str, arguments: &Value) -> ToolCallResult {
    match name {
        "search_code" => handle_search_code(arguments),
        "get_stats" => handle_get_stats(arguments),
        "get_overview" => handle_get_overview(arguments),
        "regex_search" => handle_regex_search(arguments),
        "structural_search" => handle_structural_search(arguments),
        "find_references" => handle_find_references(arguments),
        "explain_path" => handle_explain_path(arguments),
        _ => ToolCallResult::error(format!("Unknown tool: {name}")),
    }
}

fn git_scope_from_args(args: &Value) -> Result<Option<vera_core::git_scope::GitScope>, String> {
    let changed = args
        .get("changed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let since = args
        .get("since")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let base = args
        .get("base")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let selected = changed as u8 + since.is_some() as u8 + base.is_some() as u8;
    if selected > 1 {
        return Err("Only one of 'changed', 'since', or 'base' may be set".to_string());
    }

    Ok(if changed {
        Some(vera_core::git_scope::GitScope::Changed)
    } else if let Some(rev) = since {
        Some(vera_core::git_scope::GitScope::Since(rev))
    } else {
        base.map(vera_core::git_scope::GitScope::Base)
    })
}

fn scope_from_args(args: &Value) -> Result<Option<vera_core::types::SearchScope>, ToolCallResult> {
    match args.get("scope").and_then(|v| v.as_str()) {
        Some(value) => value
            .parse()
            .map(Some)
            .map_err(|()| ToolCallResult::error(format!("Invalid scope: {value}"))),
        None => Ok(None),
    }
}

fn current_working_dir() -> Result<std::path::PathBuf, ToolCallResult> {
    std::env::current_dir()
        .map_err(|e| ToolCallResult::error(format!("Failed to get working directory: {e}")))
}

fn exact_paths_from_args(
    args: &Value,
    cwd: &std::path::Path,
) -> Result<Option<Arc<std::collections::HashSet<String>>>, ToolCallResult> {
    match git_scope_from_args(args) {
        Ok(Some(scope)) => vera_core::git_scope::resolve_scope(cwd, &scope)
            .map(|paths| Some(Arc::new(paths)))
            .map_err(|err| ToolCallResult::error(format!("Failed to resolve git scope: {err}"))),
        Ok(None) => Ok(None),
        Err(err) => Err(ToolCallResult::error(err)),
    }
}

fn apply_git_scope_filters(
    args: &Value,
    cwd: &std::path::Path,
    filters: &mut vera_core::types::SearchFilters,
) -> Result<(), ToolCallResult> {
    filters.exact_paths = exact_paths_from_args(args, cwd)?;
    Ok(())
}

fn existing_index_dir(cwd: &std::path::Path) -> Result<std::path::PathBuf, ToolCallResult> {
    let index_dir = vera_core::indexing::index_dir(cwd);
    if !index_dir.exists() {
        Err(ToolCallResult::error(
            "No index found in current directory. Run search_code first to auto-index.",
        ))
    } else {
        Ok(index_dir)
    }
}

fn search_code_filters(
    args: &Value,
    scope: Option<vera_core::types::SearchScope>,
) -> vera_core::types::SearchFilters {
    let path_glob = match args.get("path") {
        Some(value) if value.is_string() => value
            .as_str()
            .map(|path| vec![path.to_string()])
            .unwrap_or_default(),
        Some(value) => value
            .as_array()
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        None => Vec::new(),
    };

    vera_core::types::SearchFilters {
        language: args.get("lang").and_then(|v| v.as_str()).map(String::from),
        path_glob,
        exact_paths: None,
        symbol_type: args
            .get("symbol_type")
            .and_then(|v| v.as_str())
            .map(String::from),
        scope,
        include_generated: Some(
            args.get("include_generated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        ),
    }
}

fn regex_search_filters(
    args: &Value,
    scope: Option<vera_core::types::SearchScope>,
) -> vera_core::types::SearchFilters {
    // Keep regex_search intentionally small over MCP. search_code already
    // exposes richer corpus filters, so regex_search sticks to the
    // highest-value regex controls.
    vera_core::types::SearchFilters {
        scope,
        exact_paths: None,
        include_generated: Some(
            args.get("include_generated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        ),
        ..Default::default()
    }
}

/// Handle the `search_code` tool.
fn handle_search_code(args: &Value) -> ToolCallResult {
    // Collect queries: support both single `query` and multi `queries`.
    let mut queries: Vec<String> = Vec::new();
    if let Some(q) = args.get("query").and_then(|v| v.as_str()) {
        queries.push(q.to_string());
    }
    if let Some(arr) = args.get("queries").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(q) = item.as_str() {
                queries.push(q.to_string());
            }
        }
    }
    let queries = vera_core::retrieval::normalize_queries(&queries);
    if queries.is_empty() {
        return ToolCallResult::error(
            "Missing required parameter: provide a non-empty 'query' (string) or 'queries' (array)",
        );
    }

    let intent = args.get("intent").and_then(|v| v.as_str());

    let scope = match scope_from_args(args) {
        Ok(scope) => scope,
        Err(err) => return err,
    };

    let mut filters = search_code_filters(args, scope);

    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let backend = vera_core::config::resolve_backend(None);
    let mut config = crate::saved_config::load_saved_runtime_config();
    config.adjust_for_backend(backend);
    let result_limit = limit.unwrap_or(config.retrieval.default_limit);

    let cwd = match current_working_dir() {
        Ok(cwd) => cwd,
        Err(err) => return err,
    };
    if let Err(err) = apply_git_scope_filters(args, &cwd, &mut filters) {
        return err;
    }
    let index_dir = match ensure_index_and_watcher(&cwd) {
        Ok(index_dir) => index_dir,
        Err(err) => return err,
    };
    let rt = match search_runtime() {
        Ok(rt) => rt,
        Err(err) => return err,
    };
    let search_context = match cached_search_context(rt, &config, backend) {
        Ok(context) => context,
        Err(err) => return err,
    };

    // Run each query. A multi-query search over-fetches per query so fusion has
    // candidates to merge; a single query is already at its final width.
    let per_query_limit = if queries.len() > 1 {
        vera_core::retrieval::multi_query_candidate_limit(result_limit)
    } else {
        result_limit
    };

    let mut result_sets: Vec<Vec<vera_core::types::SearchResult>> =
        Vec::with_capacity(queries.len());
    for query in &queries {
        // Pass the raw query plus the optional intent. Core applies the intent
        // only to the semantic (embedding/rerank) side; BM25 gets the raw query
        // so Tantivy does not parse `intent:` as a field. See issue #20.
        match rt.block_on(search_context.search(
            &index_dir,
            query,
            intent,
            &config,
            &filters,
            per_query_limit,
        )) {
            Ok((results, _timings)) => result_sets.push(results),
            Err(e) => return ToolCallResult::error(format!("Search failed: {e}")),
        }
    }

    let all_results = if result_sets.len() == 1 {
        result_sets.remove(0)
    } else {
        match fuse_multi_query_results(
            &index_dir,
            &queries,
            &result_sets,
            &filters,
            config.retrieval.rrf_k,
            result_limit,
        ) {
            Ok(results) => results,
            Err(e) => return ToolCallResult::error(format!("Search failed: {e}")),
        }
    };

    let signatures_only = args
        .get("compact")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    match compact_results_json(&all_results, MCP_OUTPUT_BUDGET, signatures_only) {
        Ok(json) => results_with_staleness(&cwd, json),
        Err(e) => ToolCallResult::error(format!("Failed to serialize results: {e}")),
    }
}

/// Merge per-query result sets the way `vera search` does.
///
/// Reciprocal rank fusion over the full candidate pool, then exact-match
/// augmentation, and only then the cut to `result_limit`. Concatenating instead
/// would hand the whole window to the first query and drop the rest.
fn fuse_multi_query_results(
    index_dir: &std::path::Path,
    queries: &[String],
    result_sets: &[Vec<vera_core::types::SearchResult>],
    filters: &vera_core::types::SearchFilters,
    rrf_k: f64,
    result_limit: usize,
) -> anyhow::Result<Vec<vera_core::types::SearchResult>> {
    vera_core::retrieval::fuse_and_augment_multi_query(
        index_dir,
        queries,
        result_sets,
        filters,
        rrf_k,
        vera_core::retrieval::multi_query_candidate_limit(result_limit),
        result_limit,
    )
}

/// Resolve an optional path argument to a validated directory path.
fn resolve_repo_path(args: &Value) -> Result<std::path::PathBuf, ToolCallResult> {
    let repo_path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => std::path::PathBuf::from(p),
        None => current_working_dir()?,
    };
    if !repo_path.exists() {
        return Err(ToolCallResult::error(format!(
            "Path does not exist: {}",
            repo_path.display()
        )));
    }
    Ok(repo_path)
}

/// Handle the `get_stats` tool.
fn handle_get_stats(args: &Value) -> ToolCallResult {
    let repo_path = match resolve_repo_path(args) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match vera_core::stats::collect_stats(&repo_path) {
        Ok(stats) => match serde_json::to_string_pretty(&stats) {
            Ok(json) => ToolCallResult::success(json),
            Err(e) => ToolCallResult::error(format!("Failed to serialize stats: {e}")),
        },
        Err(e) => ToolCallResult::error(format!("Failed to collect stats: {e}")),
    }
}

/// Handle the `get_overview` tool.
fn handle_get_overview(args: &Value) -> ToolCallResult {
    let repo_path = match resolve_repo_path(args) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let exact_paths = match git_scope_from_args(args) {
        Ok(Some(scope)) => match vera_core::git_scope::resolve_scope(&repo_path, &scope) {
            Ok(paths) => Some(paths),
            Err(err) => {
                return ToolCallResult::error(format!("Failed to resolve git scope: {err}"));
            }
        },
        Ok(None) => None,
        Err(err) => return ToolCallResult::error(err),
    };
    match vera_core::stats::collect_overview_filtered(&repo_path, exact_paths.as_ref()) {
        Ok(overview) => match serde_json::to_string_pretty(&overview) {
            Ok(json) => results_with_staleness(&repo_path, json),
            Err(e) => ToolCallResult::error(format!("Failed to serialize overview: {e}")),
        },
        Err(e) => ToolCallResult::error(format!("Failed to collect overview: {e}")),
    }
}

/// Handle the `regex_search` tool.
fn handle_regex_search(args: &Value) -> ToolCallResult {
    let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ToolCallResult::error("Missing required parameter: pattern"),
    };
    let scope = match scope_from_args(args) {
        Ok(scope) => scope,
        Err(err) => return err,
    };

    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(20);
    let ignore_case = args
        .get("ignore_case")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let context = args
        .get("context")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(2);

    let cwd = match current_working_dir() {
        Ok(cwd) => cwd,
        Err(err) => return err,
    };
    let index_dir = match existing_index_dir(&cwd) {
        Ok(index_dir) => index_dir,
        Err(err) => return err,
    };

    let mut filters = regex_search_filters(args, scope);
    if let Err(err) = apply_git_scope_filters(args, &cwd, &mut filters) {
        return err;
    }

    match vera_core::retrieval::search_regex(
        &index_dir,
        pattern,
        limit,
        ignore_case,
        context,
        &filters,
    ) {
        Ok(results) => {
            let signatures_only = args
                .get("compact")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            match compact_results_json(&results, MCP_OUTPUT_BUDGET, signatures_only) {
                Ok(json) => results_with_staleness(&cwd, json),
                Err(e) => ToolCallResult::error(format!("Failed to serialize results: {e}")),
            }
        }
        Err(e) => ToolCallResult::error(format!("Regex search failed: {e}")),
    }
}

fn handle_structural_search(args: &Value) -> ToolCallResult {
    let kind = match args.get("kind").and_then(|v| v.as_str()) {
        Some(value) => match value.parse::<vera_core::retrieval::StructuralSearchKind>() {
            Ok(kind) => kind,
            Err(()) => {
                return ToolCallResult::error(format!("Invalid structural kind: {value}"));
            }
        },
        None => return ToolCallResult::error("Missing required parameter: kind"),
    };

    let scope = match scope_from_args(args) {
        Ok(scope) => scope,
        Err(err) => return err,
    };
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(20);
    let query = args.get("query").and_then(|v| v.as_str());

    let cwd = match current_working_dir() {
        Ok(cwd) => cwd,
        Err(err) => return err,
    };

    let mut filters = search_code_filters(args, scope);
    if let Err(err) = apply_git_scope_filters(args, &cwd, &mut filters) {
        return err;
    }

    let index_dir = match ensure_index_and_watcher(&cwd) {
        Ok(index_dir) => index_dir,
        Err(err) => return err,
    };

    match vera_core::retrieval::search_structural(&index_dir, kind, query, limit, &filters) {
        Ok(results) => {
            let signatures_only = args
                .get("compact")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            match compact_results_json(&results, MCP_OUTPUT_BUDGET, signatures_only) {
                Ok(json) => results_with_staleness(&cwd, json),
                Err(e) => ToolCallResult::error(format!("Failed to serialize results: {e}")),
            }
        }
        Err(e) => ToolCallResult::error(format!("Structural search failed: {e}")),
    }
}

fn handle_find_references(args: &Value) -> ToolCallResult {
    let symbol = match args.get("symbol").and_then(|v| v.as_str()) {
        Some(symbol) if !symbol.trim().is_empty() => symbol.trim(),
        Some(_) => return ToolCallResult::error("Parameter 'symbol' must not be empty"),
        None => return ToolCallResult::error("Missing required parameter: symbol"),
    };
    let callees = args
        .get("callees")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(20);

    let cwd = match current_working_dir() {
        Ok(cwd) => cwd,
        Err(err) => return err,
    };

    let exact_paths = match exact_paths_from_args(args, &cwd) {
        Ok(paths) => paths,
        Err(err) => return err,
    };
    let index_dir = match ensure_index_and_watcher(&cwd) {
        Ok(index_dir) => index_dir,
        Err(err) => return err,
    };

    if callees {
        match vera_core::stats::find_callees(&cwd, symbol) {
            Ok(mut results) => {
                if let Some(paths) = exact_paths.as_ref() {
                    results.retain(|result| paths.contains(&result.file_path));
                }
                results.truncate(limit);
                match serde_json::to_string(&results) {
                    Ok(json) => results_with_staleness(&cwd, json),
                    Err(err) => {
                        ToolCallResult::error(format!("Failed to serialize references: {err}"))
                    }
                }
            }
            Err(err) => ToolCallResult::error(format!("Reference lookup failed: {err}")),
        }
    } else {
        let filters = vera_core::types::SearchFilters {
            scope: Some(vera_core::types::SearchScope::Source),
            exact_paths,
            include_generated: Some(false),
            ..Default::default()
        };
        match vera_core::retrieval::search_callers(&index_dir, symbol, limit, &filters) {
            Ok(results) => {
                let signatures_only = args
                    .get("compact")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                match compact_results_json(&results, MCP_OUTPUT_BUDGET, signatures_only) {
                    Ok(json) => results_with_staleness(&cwd, json),
                    Err(err) => {
                        ToolCallResult::error(format!("Failed to serialize references: {err}"))
                    }
                }
            }
            Err(err) => ToolCallResult::error(format!("Reference lookup failed: {err}")),
        }
    }
}

fn handle_explain_path(args: &Value) -> ToolCallResult {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(path) => path,
        None => return ToolCallResult::error("Missing required parameter: path"),
    };

    let cwd = match current_working_dir() {
        Ok(cwd) => cwd,
        Err(err) => return err,
    };

    let mut config = crate::saved_config::load_saved_runtime_config();
    config.indexing.no_ignore = args
        .get("no_ignore")
        .and_then(|v| v.as_bool())
        .unwrap_or(config.indexing.no_ignore);
    config.indexing.no_default_excludes = args
        .get("no_default_excludes")
        .and_then(|v| v.as_bool())
        .unwrap_or(config.indexing.no_default_excludes);
    if let Some(items) = args.get("exclude").and_then(|v| v.as_array()) {
        config.indexing.extra_excludes.extend(
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string)),
        );
    }

    match vera_core::discovery::explain_path(&cwd, std::path::Path::new(path), &config.indexing) {
        Ok(explanation) => match serde_json::to_string_pretty(&explanation) {
            Ok(json) => ToolCallResult::success(json),
            Err(e) => ToolCallResult::error(format!("Failed to serialize explanation: {e}")),
        },
        Err(e) => ToolCallResult::error(format!("Failed to explain path: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::repo_indexed_as;

    #[test]
    fn stale_index_result_carries_the_warning_after_the_results() {
        let dir = repo_indexed_as("pub fn current() {}\n", "pub fn previous() {}\n");
        let results = r#"[{"file_path":"src/lib.rs"}]"#;

        let result = results_with_staleness(dir.path(), results.to_string());

        assert!(!result.is_error);
        assert_eq!(
            result.content.len(),
            2,
            "stale index must add a notice block, got {:?}",
            result.content
        );
        assert_eq!(result.content[0].text, results);
        assert_eq!(
            result.content[1].text,
            "warning: index may be stale: 1 modified. \
             Search and grep only cover indexed files. \
             Run `vera update .` or `vera watch .`."
        );
    }

    #[test]
    fn fresh_index_result_is_a_single_results_block() {
        let dir = repo_indexed_as("pub fn current() {}\n", "pub fn current() {}\n");
        let results = r#"[{"file_path":"src/lib.rs"}]"#;
        let result = results_with_staleness(dir.path(), results.to_string());

        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].text, results);
    }

    #[test]
    fn handle_unknown_tool_returns_error() {
        let result = handle_tool_call("nonexistent", &serde_json::json!({}));
        assert!(result.is_error);
        assert!(result.content[0].text.contains("Unknown tool"));
    }

    #[test]
    fn search_code_missing_query_returns_error() {
        let result = handle_tool_call("search_code", &serde_json::json!({}));
        assert!(result.is_error);
        assert!(
            result.content[0]
                .text
                .contains("Missing required parameter")
        );
    }

    #[test]
    fn search_code_accepts_queries_array() {
        // No index and no embedding config, should get past parameter validation.
        let result = handle_tool_call(
            "search_code",
            &serde_json::json!({"queries": ["foo", "bar"]}),
        );
        // Should fail (either auto-index fails or embedding provider fails).
        assert!(result.is_error);
    }

    fn stub_result(name: &str) -> vera_core::types::SearchResult {
        vera_core::types::SearchResult {
            file_path: format!("src/{name}.rs"),
            line_start: 1,
            line_end: 10,
            content: format!("fn {name}() {{}}"),
            language: vera_core::types::Language::Rust,
            score: 1.0,
            symbol_name: Some(name.to_string()),
            symbol_type: None,
            part_index: None,
        }
    }

    #[test]
    fn multi_query_fusion_keeps_hits_the_first_query_buried() {
        // Query 1 returns more hits than the caller's limit, so a concatenating
        // merge fills the window before query 2 is ever read.
        let first: Vec<_> = (1..=10).map(|i| stub_result(&format!("a{i}"))).collect();
        // Query 2's top hit is `a8`, which query 1 ranked 8th, followed by three
        // files query 1 never returned at all.
        let second: Vec<_> = std::iter::once(stub_result("a8"))
            .chain((1..=3).map(|i| stub_result(&format!("b{i}"))))
            .collect();

        let index_dir = tempfile::tempdir().unwrap();
        let queries = vec!["alpha".to_string(), "beta".to_string()];
        let fused = fuse_multi_query_results(
            index_dir.path(),
            &queries,
            &[first, second],
            &vera_core::types::SearchFilters::default(),
            60.0,
            5,
        )
        .unwrap();
        let paths: Vec<&str> = fused.iter().map(|r| r.file_path.as_str()).collect();

        // Ranked by both queries, so it outscores every hit only one query found.
        assert_eq!(paths.first(), Some(&"src/a8.rs"), "fused: {paths:?}");
        // Query 2 only: reachable solely through fusion.
        assert!(paths.contains(&"src/b1.rs"), "fused: {paths:?}");
        // Query 1's 5th hit: inside a concatenated window, outranked after fusion.
        assert!(!paths.contains(&"src/a5.rs"), "fused: {paths:?}");
    }

    #[test]
    fn search_code_filters_include_lang_path_and_symbol_type() {
        let filters = search_code_filters(
            &serde_json::json!({
                "lang": "rust",
                "path": "src/**/*.rs",
                "symbol_type": "function",
                "include_generated": true,
            }),
            Some(vera_core::types::SearchScope::Source),
        );

        assert_eq!(filters.language.as_deref(), Some("rust"));
        assert_eq!(filters.path_glob, vec!["src/**/*.rs"]);
        assert_eq!(filters.symbol_type.as_deref(), Some("function"));
        assert_eq!(filters.scope, Some(vera_core::types::SearchScope::Source));
        assert_eq!(filters.include_generated, Some(true));
    }

    #[test]
    fn search_code_filters_accept_path_array() {
        let filters = search_code_filters(
            &serde_json::json!({
                "path": ["src/**/*.rs", "tests/**/*.py"],
            }),
            None,
        );

        assert_eq!(
            filters.path_glob,
            vec!["src/**/*.rs".to_string(), "tests/**/*.py".to_string()]
        );
    }

    #[test]
    fn search_code_filters_path_or_semantics_match_both_islands() {
        // VAL-FILTER-017/018: multiple --path / MCP path array OR semantics.
        let filters = search_code_filters(
            &serde_json::json!({
                "path": ["src/video", "src/videoplayer/b.ts"],
            }),
            None,
        );
        assert_eq!(
            filters.path_glob,
            vec!["src/video".to_string(), "src/videoplayer/b.ts".to_string()]
        );
        // Verify predicate OR: each pattern matches its own file, not the bulk.
        let video = vera_core::types::SearchResult {
            file_path: "src/video/a.ts".to_string(),
            line_start: 1,
            line_end: 5,
            content: "x".to_string(),
            language: vera_core::types::Language::TypeScript,
            score: 1.0,
            symbol_name: None,
            symbol_type: None,
            part_index: None,
        };
        let videoplayer = vera_core::types::SearchResult {
            file_path: "src/videoplayer/b.ts".to_string(),
            line_start: 1,
            line_end: 5,
            content: "x".to_string(),
            language: vera_core::types::Language::TypeScript,
            score: 1.0,
            symbol_name: None,
            symbol_type: None,
            part_index: None,
        };
        let audio = vera_core::types::SearchResult {
            file_path: "src/audio/c.ts".to_string(),
            line_start: 1,
            line_end: 5,
            content: "x".to_string(),
            language: vera_core::types::Language::TypeScript,
            score: 1.0,
            symbol_name: None,
            symbol_type: None,
            part_index: None,
        };
        assert!(filters.matches(&video));
        assert!(filters.matches(&videoplayer));
        assert!(!filters.matches(&audio));
        // Single-string form also works (CLI --path single value).
        let single = search_code_filters(&serde_json::json!({"path": "src/video"}), None);
        assert!(single.matches(&video));
        assert!(!single.matches(&audio));
    }

    #[test]
    fn search_code_filters_path_string_vs_array_parity() {
        // Ensure string and single-element array produce identical filters.
        let from_string = search_code_filters(&serde_json::json!({"path": "src/video"}), None);
        let from_array = search_code_filters(&serde_json::json!({"path": ["src/video"]}), None);
        assert_eq!(from_string.path_glob, from_array.path_glob);
    }

    #[test]
    fn removed_tools_return_unknown() {
        for tool in &[
            "index_project",
            "update_project",
            "watch_project",
            "find_dead_code",
        ] {
            let result = handle_tool_call(tool, &serde_json::json!({}));
            assert!(result.is_error);
            assert!(result.content[0].text.contains("Unknown tool"));
        }
    }

    #[test]
    fn get_stats_no_index_returns_error() {
        let result = handle_tool_call("get_stats", &serde_json::json!({"path": "/tmp"}));
        assert!(result.is_error);
        // Should mention no index found or similar.
        assert!(!result.content[0].text.is_empty());
    }

    #[test]
    fn compact_results_json_carries_bare_plus_part_index_round_trips() {
        let results = vec![
            vera_core::types::SearchResult {
                file_path: "src/mixer.tsx".to_string(),
                line_start: 1,
                line_end: 50,
                content: "export const MixerConsole: React.FC = () => { part1 }".to_string(),
                language: vera_core::types::Language::TypeScript,
                score: 1.0,
                symbol_name: Some("MixerConsole".to_string()),
                symbol_type: Some(vera_core::types::SymbolType::Function),
                part_index: Some(1),
            },
            vera_core::types::SearchResult {
                file_path: "src/mixer.tsx".to_string(),
                line_start: 51,
                line_end: 100,
                content: "part 2".to_string(),
                language: vera_core::types::Language::TypeScript,
                score: 0.9,
                symbol_name: Some("MixerConsole".to_string()),
                symbol_type: Some(vera_core::types::SymbolType::Function),
                part_index: Some(2),
            },
        ];
        let json = compact_results_json(&results, 10_000, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        for (idx, val) in arr.iter().enumerate() {
            assert_eq!(val["symbol_name"], "MixerConsole");
            assert_eq!(val["part_index"], (idx as u64) + 1);
            // Bare name must round-trip through compact output: feeding it back
            // to a definitions lookup would succeed if an index existed.
            assert!(!val["symbol_name"].as_str().unwrap().contains(" (part "));
        }
        // Text display via same SearchResult keeps part numbers.
        for r in &results {
            let display = r.display_name().unwrap();
            assert!(display.starts_with("MixerConsole (part "));
        }
    }
}
