//! `vera search <query>` — Search the indexed codebase.

use anyhow::bail;
use std::path::{Path, PathBuf};
use std::time::Instant;
use vera_core::config::InferenceBackend;
use vera_core::retrieval::cross_index::{
    IndexSearch, merge_results, search_indexes, validate_git_scope,
};
use vera_core::retrieval::search_service::{SearchContext, SearchTimings};
use vera_core::types::SearchFilters;

use crate::helpers::{
    check_index_format, output_cross_index_search_results, output_search_results,
    prepare_indexed_search, resolve_additional_roots, rewrite_absolute_path_filters,
    should_offer_auto_index, timing, warn_if_index_stale_at,
};
use crate::state;

/// Run the `vera search <query>` command.
#[allow(
    clippy::too_many_arguments,
    reason = "Search retains the existing CLI query, backend, filter and output dispatch interface"
)]
pub fn run(
    queries: &[String],
    also: &[PathBuf],
    intent: Option<&str>,
    limit: Option<usize>,
    filters: &vera_core::types::SearchFilters,
    json_output: bool,
    rerank_status: bool,
    raw: bool,
    timing: bool,
    deep: bool,
    git_scope: Option<vera_core::git_scope::GitScope>,
    compact: bool,
    backend: InferenceBackend,
) -> anyhow::Result<()> {
    validate_git_scope(!also.is_empty(), git_scope.is_some(), "--also")?;
    let mut config = state::load_runtime_config()?;
    config.adjust_for_backend(backend);
    let result_limit = limit.unwrap_or(config.retrieval.default_limit);
    let queries = vera_core::retrieval::normalize_queries(queries);

    if queries.is_empty() {
        bail!(
            "search query is empty.\n\
             Hint: pass at least one non-empty quoted query."
        );
    }

    let cwd = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("failed to get current directory: {e}"))?;
    if crate::helpers::find_index_root(&cwd).is_none()
        && should_offer_auto_index(json_output, crate::helpers::prompts_available())
        && cliclack::confirm("No index found. Index the current directory now?")
            .initial_value(true)
            .interact()?
    {
        crate::commands::index::execute(
            cwd.to_string_lossy().as_ref(),
            false,
            crate::commands::index::CommandOptions::new(backend),
        )?;
    }

    let (index_dir, primary_filters) =
        prepare_indexed_search(&config.indexing, filters, git_scope.as_ref())?;
    let primary_root = index_dir
        .parent()
        .expect("index directory has a repository parent");
    let roots = resolve_additional_roots(primary_root, also)?;
    let mut prepared = vec![(roots[0].clone(), index_dir, primary_filters)];
    for root in roots.into_iter().skip(1) {
        match timing::timed("index lookup", || check_index_format(&root)) {
            Ok((_, index_dir)) => {
                timing::timed("freshness", || {
                    warn_if_index_stale_at(&root, &config.indexing, true)
                });
                let mut repo_filters = filters.clone();
                rewrite_absolute_path_filters(&root, &mut repo_filters.path_glob);
                prepared.push((root, index_dir, repo_filters));
            }
            Err(err) => eprintln!("warning: skipped {}: {err}", root.display()),
        }
    }
    let (rt, modes, embedded, prepared) = timing::timed("setup", || -> anyhow::Result<_> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("failed to create async runtime: {e}"))?;
        let mut modes = Vec::new();
        let mut ready = Vec::new();
        for (i, (root, index, repo_filters)) in prepared.into_iter().enumerate() {
            match vera_core::indexing::is_full_text_only(&index) {
                Ok(mode) => {
                    modes.push(mode);
                    ready.push((root, index, repo_filters));
                }
                Err(err) if i == 0 => return Err(err),
                Err(err) => eprintln!("warning: skipped {}: {err}", root.display()),
            }
        }
        let embedded = if modes.contains(&false) {
            Some(rt.block_on(SearchContext::new(&config, backend)))
        } else {
            None
        };
        Ok((rt, modes, embedded, ready))
    })?;
    let keyword = SearchContext::bm25_only();
    let indexes: Vec<_> = prepared
        .iter()
        .zip(&modes)
        .map(|((_, index_dir, filters), keyword_only)| IndexSearch {
            context: if *keyword_only {
                &keyword
            } else {
                embedded
                    .as_ref()
                    .expect("embedded context initialized for semantic indexes")
            },
            index_dir,
            filters,
        })
        .collect();
    let search_start = Instant::now();
    let outcomes = rt.block_on(search_indexes(
        &indexes,
        &queries,
        intent,
        &config,
        result_limit,
        deep,
    ))?;
    let mut timings = SearchTimings::default();
    let mut lists = Vec::with_capacity(outcomes.len());
    let mut searched = Vec::new();
    for (i, outcome) in outcomes.into_iter().enumerate() {
        match outcome {
            Ok((results, stages)) => {
                // Stage durations sum work across indexes; total is wall-clock time.
                timings.merge(&stages);
                searched.push(i);
                lists.push(results);
            }
            Err(err) if i == 0 => return Err(err),
            Err(err) => {
                eprintln!("warning: skipped {}: {err}", prepared[i].0.display());
                lists.push(Vec::new());
            }
        }
    }
    timings.total = Some(search_start.elapsed());
    let cross_mode = searched.len() > 1;
    let merged = merge_results(lists, result_limit);
    let roots: Vec<_> = prepared.iter().map(|(root, _, _)| root.clone()).collect();
    let (repo_indexes, results): (Vec<_>, Vec<_>) = merged.into_iter().unzip();

    timing::record_stages(&[
        ("index open", timings.index_open),
        ("embedding", timings.embedding),
        ("bm25", timings.bm25),
        ("vector", timings.vector),
        ("fusion", timings.fusion),
        ("reranking", timings.reranking),
        ("augmentation", timings.augmentation),
        ("search", timings.total),
    ]);
    timing::timed("output", || {
        let status = rerank_status.then(|| {
            (
                &timings.rerank_outcome,
                vera_core::retrieval::reranker_kind(&config, backend),
            )
        });
        if cross_mode {
            output_cross_index_search_results(
                &results,
                &repo_indexes,
                &roots,
                json_output,
                raw,
                compact,
                config.retrieval.max_output_chars,
                status,
            );
        } else {
            output_search_results(
                &results,
                json_output,
                raw,
                compact,
                config.retrieval.max_output_chars,
                status,
            );
        }
    });

    if results.is_empty() && !json_output {
        for i in searched {
            if cross_mode {
                eprintln!("note: path filters for {}", roots[i].display());
            }
            print_path_glob_hint(&prepared[i].2, &prepared[i].1);
        }
    }

    if timing {
        timing::print_timings();
    }

    Ok(())
}

/// Issue #215: a wildcarded `--path` pattern that names only directories
/// matches no files under strict glob semantics, silently. When the search
/// came back empty and such a pattern would have acted as a directory filter,
/// say so once on stderr so the empty result is explainable.
fn print_path_glob_hint(filters: &SearchFilters, index_dir: &Path) {
    if filters.path_glob.is_empty() {
        return;
    }
    let Ok(store) =
        vera_core::storage::metadata::MetadataStore::open_existing(&index_dir.join("metadata.db"))
    else {
        return;
    };
    let Ok(indexed_files) = store.indexed_files() else {
        return;
    };
    let misses = vera_core::types::directory_prefix_near_misses(&filters.path_glob, &indexed_files);
    if misses.is_empty() {
        return;
    }
    eprintln!(
        "Hint: '{}' matched no files directly: wildcarded directory patterns do not get prefix matching, but appending '/**' would select everything beneath them.",
        misses.join("', '")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn also_is_repeatable_and_rejects_git_scopes_before_index_lookup() {
        let cli = crate::cli::Cli::try_parse_from([
            "vera", "search", "marker", "--also", "one", "--also", "two",
        ])
        .unwrap();
        let crate::cli::Commands::Search { also, .. } = cli.command else {
            panic!("expected search")
        };
        assert_eq!(also, [PathBuf::from("one"), PathBuf::from("two")]);
        let err = run(
            &["marker".into()],
            &also,
            None,
            None,
            &SearchFilters::default(),
            true,
            false,
            false,
            false,
            false,
            Some(vera_core::git_scope::GitScope::Changed),
            false,
            InferenceBackend::Api,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("--changed/--since/--base cannot be combined with --also")
        );
    }
}
