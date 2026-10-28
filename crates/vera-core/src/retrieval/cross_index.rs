//! Search existing indexes without comparing scores from different corpora.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Result, bail};

use crate::config::VeraConfig;
use crate::types::{SearchFilters, SearchResult};

use super::rag_fusion::{execute_prepared_deep_search, prepare_deep_search};
use super::search_service::{SearchContext, SearchTimings};

pub const MAX_SEARCH_INDEXES: usize = 8;

/// Raw BM25/hybrid scores are not comparable across corpora. Rank fusion over
/// disjoint lists reduces to rank ordering, with a one-position head start for
/// the primary: P1, P2, E1, P3, E2. Ties retain repository order.
pub fn merge_results(lists: Vec<Vec<SearchResult>>, limit: usize) -> Vec<(usize, SearchResult)> {
    let mut ranked: Vec<_> = lists
        .into_iter()
        .enumerate()
        .flat_map(|(repo, results)| {
            results
                .into_iter()
                .enumerate()
                .map(move |(rank, result)| ((rank + usize::from(repo != 0), repo), result))
        })
        .collect();
    ranked.sort_by_key(|(key, _)| *key);
    ranked
        .into_iter()
        .take(limit)
        .map(|((_, repo), result)| (repo, result))
        .collect()
}

/// Roots must already be canonical. Deduplicate before enforcing the bound.
pub fn search_roots(
    primary: &Path,
    extras: impl IntoIterator<Item = PathBuf>,
) -> Result<Vec<PathBuf>> {
    let mut roots = vec![primary.to_path_buf()];
    for root in extras {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    if roots.len() > MAX_SEARCH_INDEXES {
        bail!(
            "At most {MAX_SEARCH_INDEXES} search indexes are supported (primary plus additional repositories)."
        );
    }
    Ok(roots)
}

pub fn validate_git_scope(has_extras: bool, has_scope: bool, argument: &str) -> Result<()> {
    if has_extras && has_scope {
        bail!(
            "--changed/--since/--base cannot be combined with {argument}: git scopes are per repository"
        );
    }
    Ok(())
}

/// Rewrite absolute filters under this repository to indexed relative paths.
/// Outside paths remain unmatched. Canonical roots also support symlinked calls.
pub fn rewrite_absolute_path_filters(repo_root: &Path, patterns: &mut Vec<String>) {
    if patterns.is_empty() {
        return;
    }
    let mut roots = vec![repo_root.to_path_buf()];
    if let Ok(canonical) = repo_root.canonicalize()
        && canonical != repo_root
    {
        roots.push(canonical);
    }
    let mut rewritten = Vec::with_capacity(patterns.len());
    for pattern in patterns.drain(..) {
        match absolute_path_filter_rewrite_inner(&pattern, &roots, cfg!(windows)) {
            PathFilterRewrite::Keep => rewritten.push(pattern),
            // Patterns are ORed, so the index root admits every file.
            PathFilterRewrite::Drop => return,
            PathFilterRewrite::Rewrite(rel) => rewritten.push(rel),
        }
    }
    *patterns = rewritten;
}

enum PathFilterRewrite {
    Keep,
    Drop,
    Rewrite(String),
}

fn absolute_path_filter_rewrite_inner(
    pattern: &str,
    roots: &[PathBuf],
    case_insensitive: bool,
) -> PathFilterRewrite {
    let normalized = without_verbatim_prefix(&pattern.replace('\\', "/"));
    let bytes = normalized.as_bytes();
    let is_absolute = normalized.starts_with('/')
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'/');
    if !is_absolute {
        return PathFilterRewrite::Keep;
    }
    let matches = |a: &str, b: &str| {
        if case_insensitive {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    for root in roots {
        let root = without_verbatim_prefix(&root.to_string_lossy().replace('\\', "/"));
        let root = root.trim_end_matches('/');
        if matches(&normalized, root) {
            return PathFilterRewrite::Drop;
        }
        if normalized.len() > root.len()
            && normalized.as_bytes()[root.len()] == b'/'
            && matches(&normalized[..root.len()], root)
        {
            let rel = &normalized[root.len() + 1..];
            return if rel.is_empty() {
                PathFilterRewrite::Drop
            } else {
                PathFilterRewrite::Rewrite(rel.to_string())
            };
        }
    }
    PathFilterRewrite::Keep
}

/// Windows canonical paths use verbatim forms (`\\?\C:\repo`,
/// `\\?\UNC\server\share`) that ordinary absolute filters never spell out.
fn without_verbatim_prefix(path: &str) -> String {
    match path.strip_prefix("//?/UNC/") {
        Some(rest) => format!("//{rest}"),
        None => path.strip_prefix("//?/").unwrap_or(path).to_string(),
    }
}

pub struct IndexSearch<'a> {
    pub context: &'a SearchContext,
    pub index_dir: &'a Path,
    pub filters: &'a SearchFilters,
}

/// Prepare deep-query expansion once using the primary index, then search the
/// bounded set of indexes concurrently. Each index fuses its own queries before
/// cross-index merging. Errors retain their index slots for caller policy.
pub async fn search_indexes(
    indexes: &[IndexSearch<'_>],
    queries: &[String],
    intent: Option<&str>,
    config: &VeraConfig,
    result_limit: usize,
    deep: bool,
) -> Result<Vec<Result<(Vec<SearchResult>, SearchTimings)>>> {
    if indexes.len() > MAX_SEARCH_INDEXES {
        bail!("At most {MAX_SEARCH_INDEXES} search indexes are supported.");
    }
    let Some(primary) = indexes.first() else {
        return Ok(Vec::new());
    };
    let mut plans = Vec::new();
    if deep {
        for query in queries {
            plans.push(prepare_deep_search(primary.index_dir, query).await?);
        }
    }
    Ok(futures::future::join_all(indexes.iter().map(|index| async {
        let start = Instant::now();
        let per_query_limit = if queries.len() > 1 {
            super::multi_query_candidate_limit(result_limit)
        } else {
            result_limit
        };
        let mut timings = SearchTimings::default();
        let mut lists = Vec::with_capacity(queries.len());
        for (i, query) in queries.iter().enumerate() {
            let (results, stages) = if deep {
                execute_prepared_deep_search(
                    index.context,
                    index.index_dir,
                    &plans[i],
                    intent,
                    config,
                    index.filters,
                    per_query_limit,
                )
                .await?
            } else {
                index
                    .context
                    .search(
                        index.index_dir,
                        query,
                        intent,
                        config,
                        index.filters,
                        per_query_limit,
                    )
                    .await?
            };
            timings.merge(&stages);
            lists.push(results);
        }
        let results = if lists.len() == 1 {
            lists.remove(0)
        } else {
            super::fuse_and_augment_multi_query(
                index.index_dir,
                queries,
                &lists,
                index.filters,
                config.retrieval.rrf_k,
                super::multi_query_candidate_limit(result_limit),
                result_limit,
            )?
        };
        timings.total = Some(start.elapsed());
        Ok((results, timings))
    }))
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Language;

    #[test]
    fn absolute_path_filters_preserve_platform_and_outside_path_semantics() {
        for (pattern, root, insensitive, expected) in [
            ("/repo/src/auth", "/repo", false, Some("src/auth")),
            ("/repo\\src\\auth", "/repo", false, Some("src/auth")),
            ("C:\\repo\\src", "C:\\repo", false, Some("src")),
            ("C:\\Repo\\Src", "c:\\repo", true, Some("Src")),
            ("C:\\Repo\\Src", "c:\\repo", false, None),
            ("C:\\repo\\src", "\\\\?\\C:\\repo", true, Some("src")),
            ("\\\\?\\C:\\repo\\src", "C:\\repo", true, Some("src")),
            (
                "\\\\server\\share\\src",
                "\\\\?\\UNC\\server\\share",
                true,
                Some("src"),
            ),
            ("/other/src", "/repo", false, None),
            ("src/**", "/repo", false, None),
        ] {
            match (
                absolute_path_filter_rewrite_inner(pattern, &[PathBuf::from(root)], insensitive),
                expected,
            ) {
                (PathFilterRewrite::Rewrite(actual), Some(expected)) => {
                    assert_eq!(actual, expected)
                }
                (PathFilterRewrite::Keep, None) => {}
                _ => panic!("unexpected rewrite for {pattern}"),
            }
        }
        for pattern in ["/repo", "/repo/"] {
            assert!(matches!(
                absolute_path_filter_rewrite_inner(pattern, &[PathBuf::from("/repo")], false),
                PathFilterRewrite::Drop
            ));
        }
        let mut filters = vec!["/primary/src".into()];
        rewrite_absolute_path_filters(Path::new("/extra"), &mut filters);
        assert_eq!(filters, ["/primary/src"]);
        rewrite_absolute_path_filters(Path::new("/primary"), &mut filters);
        assert_eq!(filters, ["src"]);
    }

    fn list(names: &[&str]) -> Vec<SearchResult> {
        names
            .iter()
            .map(|name| SearchResult {
                file_path: name.to_string(),
                content: String::new(),
                language: Language::Rust,
                line_start: 1,
                line_end: 1,
                score: 99.0,
                symbol_name: None,
                symbol_type: None,
                part_index: None,
            })
            .collect()
    }

    #[test]
    fn rank_merge_prefers_primary_and_truncates() {
        let merge = |limit| {
            merge_results(vec![list(&["P1", "P2", "P3"]), list(&["E1", "E2"])], limit)
                .into_iter()
                .map(|(_, r)| r.file_path)
                .collect::<Vec<_>>()
        };
        assert_eq!(merge(5), ["P1", "P2", "E1", "P3", "E2"]);
        assert_eq!(merge(3), ["P1", "P2", "E1"]);
        assert!(merge(0).is_empty());
    }

    #[test]
    fn empty_primary_and_extra_ties_keep_repo_order() {
        let results = merge_results(vec![vec![], list(&["A1", "A2"]), list(&["B1", "B2"])], 8);
        assert_eq!(
            results
                .into_iter()
                .map(|(repo, r)| (repo, r.file_path))
                .collect::<Vec<_>>(),
            [
                (1, "A1".into()),
                (2, "B1".into()),
                (1, "A2".into()),
                (2, "B2".into())
            ]
        );
    }

    #[test]
    fn roots_deduplicate_before_enforcing_bound() {
        let primary = Path::new("/primary");
        assert_eq!(
            search_roots(
                primary,
                [
                    primary.to_path_buf(),
                    PathBuf::from("/extra"),
                    PathBuf::from("/extra")
                ]
            )
            .unwrap(),
            [primary.to_path_buf(), PathBuf::from("/extra")]
        );
        assert!(search_roots(primary, (0..8).map(|i| PathBuf::from(format!("/{i}")))).is_err());
        assert!(search_roots(primary, (0..7).map(|i| PathBuf::from(format!("/{i}")))).is_ok());
    }

    #[test]
    fn git_scope_cannot_span_indexes() {
        assert!(
            validate_git_scope(true, true, "--also")
                .unwrap_err()
                .to_string()
                .contains("git scopes are per repository")
        );
        assert!(validate_git_scope(false, true, "--also").is_ok());
        assert!(validate_git_scope(true, false, "repos").is_ok());
    }
}
