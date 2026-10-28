//! Tool dispatch, argument validation, retrieval, and result formatting.

use std::sync::Arc;

use serde_json::Value;
use vera_core::presentation::{CompactResult, truncate_to_budget};

use crate::protocol::ToolCallResult;

use super::runtime::{
    cached_search_context, ensure_index_and_watcher, keyword_search_context, search_runtime,
};

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
    results_with_notices(cwd, json, Vec::new())
}

/// Like [`results_with_staleness`], with `notices` ahead of any stale-index
/// notice in the same block.
fn results_with_notices(
    cwd: &std::path::Path,
    json: String,
    mut notices: Vec<String>,
) -> ToolCallResult {
    notices.extend(crate::staleness::notice_for_repo(cwd));
    ToolCallResult::success_with_notice(json, (!notices.is_empty()).then(|| notices.join("\n")))
}

/// Notices for results from a degraded pipeline. The CLI reports these on
/// stderr, which an MCP agent never sees, so without them it cannot tell
/// keyword-only or unreranked results from a full hybrid search. Merged
/// multi-query timings keep the first fallback, which may cover only one query.
fn degraded_search_notices(
    timings: &vera_core::retrieval::search_service::SearchTimings,
    multi_query: bool,
) -> Vec<String> {
    let (keyword_only, unreranked) = if multi_query {
        (
            "At least one query ran keyword-only (BM25)",
            "At least one query was not reranked",
        )
    } else {
        (
            "These results are keyword-only (BM25)",
            "These results are not reranked",
        )
    };
    let mut notices = Vec::new();
    if let Some(reason) = &timings.semantic_fallback {
        notices.push(format!(
            "warning: semantic search unavailable ({reason}). {keyword_only}, so prefer exact identifiers in queries."
        ));
    }
    if let vera_core::retrieval::RerankOutcome::Fallback(reason) = &timings.rerank_outcome {
        notices.push(format!(
            "warning: reranker unavailable ({reason}). {unreranked}."
        ));
    }
    notices
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

struct Repository {
    root: std::path::PathBuf,
    is_server_repo: bool,
}

impl Repository {
    fn search_index_dir(&self) -> Result<std::path::PathBuf, ToolCallResult> {
        if self.is_server_repo {
            ensure_index_and_watcher(&self.root)
        } else {
            // The resolver requires an existing index for secondary repositories.
            existing_index_dir(&self.root)
        }
    }
}

/// Resolve `repo`, falling back to the legacy `path` argument when `repo` is absent or null.
fn resolve_repository_or_path(args: &Value) -> Result<Repository, ToolCallResult> {
    resolve_repository(
        args.get("repo")
            .filter(|value| !value.is_null())
            .or_else(|| args.get("path")),
    )
}

/// Resolve a repository without changing the server's process-wide directory.
///
/// Both the server's own directory and an explicit `repo` resolve to the
/// nearest indexed ancestor, as the CLI does, so a server started in (or a
/// `repo` naming) a subdirectory uses the existing index instead of
/// creating a nested one. Only the server's directory may lack an index: it is
/// auto-indexed on first search.
fn resolve_repository(repo: Option<&Value>) -> Result<Repository, ToolCallResult> {
    let cwd = std::env::current_dir()
        .and_then(|cwd| cwd.canonicalize())
        .map_err(|e| ToolCallResult::error(format!("Failed to get working directory: {e}")))?;
    resolve_repository_from(&cwd, repo)
}

/// [`resolve_repository`] for a server whose canonical working directory is `cwd`.
fn resolve_repository_from(
    cwd: &std::path::Path,
    repo: Option<&Value>,
) -> Result<Repository, ToolCallResult> {
    let server_root =
        vera_core::indexing::find_index_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let root = match repo.filter(|value| !value.is_null()) {
        Some(value) => {
            let path = value
                .as_str()
                .ok_or_else(|| ToolCallResult::error("Repository path must be a string"))?;
            let requested = cwd.join(path).canonicalize().map_err(|err| {
                ToolCallResult::error(format!("Cannot resolve repository {path}: {err}"))
            })?;
            if !requested.is_dir() {
                return Err(ToolCallResult::error(format!(
                    "Repository is not a directory: {}",
                    requested.display()
                )));
            }
            match vera_core::indexing::find_index_root(&requested) {
                Some(root) => root,
                None if requested == server_root => requested,
                None => {
                    return Err(ToolCallResult::error(format!(
                        "No index found at {} or any parent directory. Run `vera index {}` first.",
                        requested.display(),
                        requested.display()
                    )));
                }
            }
        }
        None => server_root.clone(),
    };
    Ok(Repository {
        is_server_repo: root == server_root,
        root,
    })
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

    let repo = match resolve_repository(args.get("repo")) {
        Ok(repo) => repo,
        Err(err) => return err,
    };
    if let Err(err) = apply_git_scope_filters(args, &repo.root, &mut filters) {
        return err;
    }
    let index_dir = match repo.search_index_dir() {
        Ok(index_dir) => index_dir,
        Err(err) => return err,
    };
    let rt = match search_runtime() {
        Ok(rt) => rt,
        Err(err) => return err,
    };
    // A mode read error falls through to the embedded path, which degrades to BM25 itself.
    let search_context = if matches!(vera_core::indexing::is_full_text_only(&index_dir), Ok(true)) {
        keyword_search_context()
    } else {
        match cached_search_context(rt, &config, backend) {
            Ok(context) => context,
            Err(err) => return err,
        }
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
    let mut timings = vera_core::retrieval::search_service::SearchTimings::default();
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
            Ok((results, query_timings)) => {
                timings.merge(&query_timings);
                result_sets.push(results);
            }
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
        Ok(json) => results_with_notices(
            &repo.root,
            json,
            degraded_search_notices(&timings, queries.len() > 1),
        ),
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

/// Handle the `get_stats` tool.
fn handle_get_stats(args: &Value) -> ToolCallResult {
    let repo = match resolve_repository_or_path(args) {
        Ok(repo) => repo,
        Err(e) => return e,
    };
    match vera_core::stats::collect_stats(&repo.root) {
        Ok(stats) => match serde_json::to_string_pretty(&stats) {
            Ok(json) => ToolCallResult::success(json),
            Err(e) => ToolCallResult::error(format!("Failed to serialize stats: {e}")),
        },
        Err(e) => ToolCallResult::error(format!("Failed to collect stats: {e}")),
    }
}

/// Handle the `get_overview` tool.
fn handle_get_overview(args: &Value) -> ToolCallResult {
    let repo = match resolve_repository_or_path(args) {
        Ok(repo) => repo,
        Err(e) => return e,
    };
    let exact_paths = match git_scope_from_args(args) {
        Ok(Some(scope)) => match vera_core::git_scope::resolve_scope(&repo.root, &scope) {
            Ok(paths) => Some(paths),
            Err(err) => {
                return ToolCallResult::error(format!("Failed to resolve git scope: {err}"));
            }
        },
        Ok(None) => None,
        Err(err) => return ToolCallResult::error(err),
    };
    match vera_core::stats::collect_overview_filtered(&repo.root, exact_paths.as_ref()) {
        Ok(overview) => match serde_json::to_string_pretty(&overview) {
            Ok(json) => results_with_staleness(&repo.root, json),
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

    let repo = match resolve_repository(args.get("repo")) {
        Ok(repo) => repo,
        Err(err) => return err,
    };
    let index_dir = match existing_index_dir(&repo.root) {
        Ok(index_dir) => index_dir,
        Err(err) => return err,
    };

    let mut filters = regex_search_filters(args, scope);
    if let Err(err) = apply_git_scope_filters(args, &repo.root, &mut filters) {
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
                Ok(json) => results_with_staleness(&repo.root, json),
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

    let repo = match resolve_repository(args.get("repo")) {
        Ok(repo) => repo,
        Err(err) => return err,
    };

    let mut filters = search_code_filters(args, scope);
    if let Err(err) = apply_git_scope_filters(args, &repo.root, &mut filters) {
        return err;
    }

    let index_dir = match repo.search_index_dir() {
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
                Ok(json) => results_with_staleness(&repo.root, json),
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

    let repo = match resolve_repository(args.get("repo")) {
        Ok(repo) => repo,
        Err(err) => return err,
    };

    let exact_paths = match exact_paths_from_args(args, &repo.root) {
        Ok(paths) => paths,
        Err(err) => return err,
    };
    let index_dir = match repo.search_index_dir() {
        Ok(index_dir) => index_dir,
        Err(err) => return err,
    };

    if callees {
        match vera_core::stats::find_callees(&repo.root, symbol) {
            Ok(mut results) => {
                if let Some(paths) = exact_paths.as_ref() {
                    results.retain(|result| paths.contains(&result.file_path));
                }
                results.truncate(limit);
                match serde_json::to_string(&results) {
                    Ok(json) => results_with_staleness(&repo.root, json),
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
                    Ok(json) => results_with_staleness(&repo.root, json),
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

    let repo = match resolve_repository(args.get("repo")) {
        Ok(repo) => repo,
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

    match vera_core::discovery::explain_path(
        &repo.root,
        std::path::Path::new(path),
        &config.indexing,
    ) {
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

    fn temporary_repo() -> tempfile::TempDir {
        let scratch =
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        tempfile::tempdir_in(scratch).unwrap()
    }

    fn keyword_repo() -> tempfile::TempDir {
        let dir = temporary_repo();
        crate::test_support::write_file(
            dir.path(),
            "src/dependency.rs",
            "pub fn dependency_marker() {}\npub fn dependency_caller() { dependency_marker(); }\n",
        );
        search_runtime()
            .unwrap()
            .block_on(
                vera_core::indexing::pipeline::index_repository_with_progress_and_cancellation(
                    dir.path(),
                    None::<&vera_core::embedding::DynamicProvider>,
                    &vera_core::config::VeraConfig::default(),
                    "",
                    |_| {},
                    &vera_core::CancellationToken::new(),
                ),
            )
            .unwrap();
        dir
    }

    #[test]
    fn search_tools_query_a_secondary_keyword_index() {
        let dir = keyword_repo();
        for (tool, mut args) in [
            (
                "regex_search",
                serde_json::json!({"pattern": "dependency_marker"}),
            ),
            (
                "search_code",
                serde_json::json!({"query": "dependency_marker"}),
            ),
            (
                "structural_search",
                serde_json::json!({"kind": "definitions", "query": "dependency_marker"}),
            ),
            (
                "find_references",
                serde_json::json!({"symbol": "dependency_marker"}),
            ),
        ] {
            args["repo"] = serde_json::json!(dir.path());
            let result = handle_tool_call(tool, &args);
            assert!(!result.is_error, "{tool}: {:?}", result.content);
            let hits: Value = serde_json::from_str(&result.content[0].text).unwrap();
            let hits = hits.as_array().unwrap();
            assert!(!hits.is_empty(), "{tool}: no hits");
            assert!(
                hits.iter()
                    .all(|hit| hit["file_path"] == "src/dependency.rs")
            );
            assert!(hits.iter().any(|hit| {
                hit["content"]
                    .as_str()
                    .unwrap()
                    .contains("dependency_marker")
            }));
            assert_eq!(result.content.len(), 1, "{tool}: {:?}", result.content);
        }
    }

    #[test]
    fn secondary_repo_metadata_and_path_tools_use_its_root() {
        let dir = keyword_repo();
        for tool in ["get_stats", "get_overview", "explain_path"] {
            let result = handle_tool_call(
                tool,
                &serde_json::json!({"repo": dir.path(), "path": "src/dependency.rs"}),
            );
            assert!(!result.is_error, "{tool}: {:?}", result.content);
        }
        for tool in ["get_stats", "get_overview"] {
            for args in [
                serde_json::json!({"path": dir.path()}),
                serde_json::json!({"repo": null, "path": dir.path()}),
                serde_json::json!({"repo": dir.path(), "path": null}),
            ] {
                let result = handle_tool_call(tool, &args);
                assert!(!result.is_error, "{tool}: {:?}", result.content);
            }
            let absent = handle_tool_call(tool, &serde_json::json!({}));
            let null = handle_tool_call(tool, &serde_json::json!({"repo": null, "path": null}));
            assert_eq!(null.is_error, absent.is_error);
            assert_eq!(null.content[0].text, absent.content[0].text);
        }
        let result = handle_tool_call(
            "find_references",
            &serde_json::json!({"repo": dir.path(), "symbol": "dependency_caller", "callees": true}),
        );
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("dependency_marker"));
    }

    #[test]
    fn unindexed_secondary_repo_is_never_created() {
        let dir = temporary_repo();
        let root = dir.path().canonicalize().unwrap();
        let args = serde_json::json!({
            "repo": dir.path(),
            "query": "dependency_marker",
            "pattern": "dependency_marker",
            "kind": "definitions",
            "symbol": "dependency_marker",
            "path": "src/dependency.rs",
        });
        for empty_index_dir in [false, true] {
            if empty_index_dir {
                std::fs::create_dir(root.join(".vera")).unwrap();
            }
            for tool in [
                "search_code",
                "get_stats",
                "get_overview",
                "regex_search",
                "structural_search",
                "find_references",
                "explain_path",
            ] {
                let result = handle_tool_call(tool, &args);
                assert!(result.is_error, "{tool}");
                assert_eq!(
                    result.content[0].text,
                    format!(
                        "No index found at {} or any parent directory. Run `vera index {}` first.",
                        root.display(),
                        root.display()
                    ),
                    "{tool}"
                );
                assert_eq!(root.join(".vera").exists(), empty_index_dir, "{tool}");
                assert_eq!(
                    std::fs::read_dir(&root).unwrap().count(),
                    usize::from(empty_index_dir)
                );
                if empty_index_dir {
                    assert_eq!(
                        std::fs::read_dir(root.join(".vera")).unwrap().count(),
                        0,
                        "{tool}"
                    );
                }
            }
        }
    }

    #[test]
    fn repository_resolution_recognizes_the_server_root() {
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        // A checkout with its own index resolves the test directory to it.
        let server_root = vera_core::indexing::find_index_root(&cwd).unwrap_or_else(|| cwd.clone());
        for value in [
            None,
            Some(Value::Null),
            Some(serde_json::json!(".")),
            Some(serde_json::json!(cwd)),
        ] {
            let repo = resolve_repository(value.as_ref()).unwrap();
            assert!(repo.is_server_repo);
            assert_eq!(repo.root, server_root);
        }
    }

    #[test]
    fn repository_resolution_uses_the_index_above_a_subdirectory() {
        let dir = keyword_repo();
        let root = dir.path().canonicalize().unwrap();
        let repo = resolve_repository(Some(&serde_json::json!(root.join("src")))).unwrap();
        assert_eq!(repo.root, root);
        assert!(!repo.is_server_repo);
        assert!(!root.join("src/.vera").exists());

        let result = handle_tool_call(
            "search_code",
            &serde_json::json!({"repo": root.join("src"), "query": "dependency_marker"}),
        );
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("src/dependency.rs"));
    }

    #[test]
    fn server_started_in_a_subdirectory_uses_the_ancestor_index() {
        let dir = keyword_repo();
        let root = dir.path().canonicalize().unwrap();
        let subdir = root.join("src");
        for value in [
            None,
            Some(serde_json::json!(".")),
            Some(serde_json::json!(root)),
        ] {
            let repo = resolve_repository_from(&subdir, value.as_ref()).unwrap();
            assert_eq!(repo.root, root);
            assert!(repo.is_server_repo);
        }
        let index_dir = super::super::runtime::ensure_index_and_watcher(&root).unwrap();
        assert_eq!(index_dir, root.join(".vera"));
        assert!(!subdir.join(".vera").exists());
    }

    #[test]
    fn auto_indexing_refuses_broad_roots() {
        for root in [Some(std::path::PathBuf::from("/")), dirs_home()]
            .into_iter()
            .flatten()
        {
            if vera_core::indexing::index_exists(&root) {
                continue;
            }
            let error = super::super::runtime::ensure_index_and_watcher(&root).unwrap_err();
            assert!(error.is_error);
            assert!(
                error.content[0].text.contains("Refusing to auto-index"),
                "{:?}",
                error.content
            );
            assert!(!vera_core::indexing::index_exists(&root));
        }
    }

    fn dirs_home() -> Option<std::path::PathBuf> {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }

    #[test]
    fn repository_resolution_rejects_files_and_invalid_arguments() {
        let dir = temporary_repo();
        crate::test_support::write_file(dir.path(), "src/dependency.rs", "");
        for value in [
            serde_json::json!(dir.path().join("missing")),
            serde_json::json!(dir.path().join("src/dependency.rs")),
            serde_json::json!(42),
        ] {
            assert!(resolve_repository(Some(&value)).is_err(), "{value}");
        }
    }

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
            format!(
                "warning: index may be stale: 1 modified. \
             Search and grep only cover indexed files. \
             Run `vera -C {root} update .` or `vera -C {root} watch .`.",
                root = dir.path().canonicalize().unwrap().display()
            )
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
    fn degraded_search_notices_precede_the_stale_index_notice() {
        let dir = repo_indexed_as("pub fn current() {}\n", "pub fn previous() {}\n");
        let timings = vera_core::retrieval::search_service::SearchTimings {
            semantic_fallback: Some("vector search failed: 401".to_string()),
            rerank_outcome: vera_core::retrieval::RerankOutcome::Fallback("timeout".to_string()),
            ..Default::default()
        };

        let result = results_with_notices(
            dir.path(),
            "[]".to_string(),
            degraded_search_notices(&timings, false),
        );

        assert_eq!(result.content.len(), 2);
        let lines: Vec<&str> = result.content[1].text.lines().collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].starts_with("warning: semantic search unavailable (vector search failed: 401). These results are keyword-only (BM25)"));
        assert_eq!(
            lines[1],
            "warning: reranker unavailable (timeout). These results are not reranked."
        );
        assert!(lines[2].starts_with("warning: index may be stale"));
    }

    #[test]
    fn full_hybrid_search_adds_no_notice() {
        let timings = vera_core::retrieval::search_service::SearchTimings {
            rerank_outcome: vera_core::retrieval::RerankOutcome::Reranked,
            ..Default::default()
        };
        assert!(degraded_search_notices(&timings, false).is_empty());
    }

    #[test]
    fn multi_query_notice_does_not_claim_every_result_degraded() {
        let timings = vera_core::retrieval::search_service::SearchTimings {
            semantic_fallback: Some("vector search failed: 429".to_string()),
            ..Default::default()
        };
        let notices = degraded_search_notices(&timings, true);
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0].contains("At least one query ran keyword-only (BM25)"),
            "{}",
            notices[0]
        );
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
        // An unindexed secondary repository fails only after the queries
        // passed parameter validation.
        let dir = temporary_repo();
        let result = handle_tool_call(
            "search_code",
            &serde_json::json!({"queries": ["foo", "bar"], "repo": dir.path()}),
        );
        assert!(result.is_error);
        assert!(
            result.content[0].text.contains("No index found"),
            "{:?}",
            result.content
        );
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
