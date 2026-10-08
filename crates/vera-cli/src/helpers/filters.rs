//! Search filters, git scopes, and path normalization.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;

use super::index_root::prepare_indexed_repo;

#[derive(Debug, Clone, Default, Args)]
pub struct SearchFilterArgs {
    /// Filter by programming language (case-insensitive).
    #[arg(long)]
    pub lang: Option<String>,
    /// Filter by file path glob pattern (e.g., "src/**/*.rs"). Repeatable;
    /// patterns are combined with OR semantics.
    #[arg(long)]
    pub path: Vec<String>,
    /// Filter by symbol type.
    /// Note: function and method are treated as aliases.
    #[arg(long, rename_all = "snake_case")]
    pub r#type: Option<String>,
    /// Restrict results to a coarse corpus scope.
    #[arg(long, value_parser = ["source", "docs", "runtime", "all"])]
    pub scope: Option<String>,
    /// Include generated or minified files such as dist bundles.
    #[arg(long)]
    pub include_generated: bool,
}

impl SearchFilterArgs {
    pub fn to_filters(&self) -> vera_core::types::SearchFilters {
        vera_core::types::SearchFilters {
            language: self.lang.clone(),
            path_glob: self.path.clone(),
            exact_paths: None,
            symbol_type: self.r#type.clone(),
            scope: self.scope.as_deref().and_then(|value| value.parse().ok()),
            include_generated: Some(self.include_generated),
        }
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct GitScopeFlags {
    /// Limit results to modified, staged, and untracked files.
    #[arg(long, group = "git_scope")]
    pub changed: bool,
    /// Limit results to files changed since the given revision.
    #[arg(long, value_name = "REV", group = "git_scope")]
    pub since: Option<String>,
    /// Limit results to files changed since merge-base(HEAD, REV).
    #[arg(long, value_name = "REV", group = "git_scope")]
    pub base: Option<String>,
}

impl GitScopeFlags {
    pub fn resolve(&self) -> Option<vera_core::git_scope::GitScope> {
        if self.changed {
            Some(vera_core::git_scope::GitScope::Changed)
        } else if let Some(rev) = self.since.as_ref() {
            Some(vera_core::git_scope::GitScope::Since(rev.clone()))
        } else {
            self.base
                .as_ref()
                .map(|rev| vera_core::git_scope::GitScope::Base(rev.clone()))
        }
    }
}

pub fn apply_git_scope(
    cwd: &Path,
    filters: &vera_core::types::SearchFilters,
    git_scope: Option<&vera_core::git_scope::GitScope>,
) -> anyhow::Result<vera_core::types::SearchFilters> {
    let mut filters = filters.clone();
    if let Some(scope) = git_scope {
        filters.exact_paths = Some(Arc::new(vera_core::git_scope::resolve_scope(cwd, scope)?));
    }
    Ok(filters)
}

pub fn prepare_indexed_search(
    indexing_config: &vera_core::config::IndexingConfig,
    filters: &vera_core::types::SearchFilters,
    git_scope: Option<&vera_core::git_scope::GitScope>,
) -> anyhow::Result<(PathBuf, vera_core::types::SearchFilters)> {
    let (repo_root, index_dir) = prepare_indexed_repo(indexing_config)?;
    let mut filters = apply_git_scope(&repo_root, filters, git_scope)?;
    rewrite_absolute_path_filters(&repo_root, &mut filters.path_glob);
    Ok((index_dir, filters))
}

/// What to do with a `--path` filter entry that turns out to be an absolute
/// path.
enum PathFilterRewrite {
    /// Not an absolute path, or absolute but outside the index root: leave it
    /// untouched (`path_filter_hint` covers the all-miss case).
    Keep,
    /// The entry pointed at the index root itself: it admits everything, so
    /// drop it.
    Drop,
    /// The entry pointed inside the index root: replace it with the
    /// root-relative form using forward slashes.
    Rewrite(String),
}

/// Rewrite one absolute `--path` entry against the candidate index roots.
/// Windows-style separators and drive-letter paths are normalized to `/` so
/// the same rule holds whatever produced the path.
fn absolute_path_filter_rewrite(pattern: &str, roots: &[PathBuf]) -> PathFilterRewrite {
    absolute_path_filter_rewrite_inner(pattern, roots, cfg!(windows))
}

/// Comparison core for [`absolute_path_filter_rewrite`]. `case_insensitive`
/// models Windows path semantics where `C:\Repo` and `c:\repo` are the same
/// directory; the rewritten suffix is always sliced from the original pattern
/// so its casing is preserved.
fn absolute_path_filter_rewrite_inner(
    pattern: &str,
    roots: &[PathBuf],
    case_insensitive: bool,
) -> PathFilterRewrite {
    let normalized = pattern.replace('\\', "/");
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
        let root = root.to_string_lossy().replace('\\', "/");
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

/// Rewrite every absolute `--path` entry that resolves under the index root to
/// its root-relative form, since indexed paths are stored relative to it. The
/// root is tried as-is and canonicalized so symlinked invocations work.
fn rewrite_absolute_path_filters(repo_root: &Path, patterns: &mut Vec<String>) {
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
        match absolute_path_filter_rewrite(&pattern, &roots) {
            PathFilterRewrite::Keep => rewritten.push(pattern),
            PathFilterRewrite::Drop => {}
            PathFilterRewrite::Rewrite(rel) => rewritten.push(rel),
        }
    }
    *patterns = rewritten;
}

/// Warn when a `--path` pattern matched none of the indexed files.
///
/// Empty results look the same whether the query found nothing or the filter
/// excluded everything, and the two want different fixes. The common case is a
/// directory pattern carrying a wildcard: `--path 'crates/*/src'` matches no
/// *file*, while the wildcard-free `--path crates/vera-core/src` is treated as
/// a directory prefix and matches everything beneath it.
///
/// Returns `None` when every pattern matched something, so a genuinely empty
/// result set stays quiet.
pub fn path_filter_hint(
    index_dir: &std::path::Path,
    filters: &vera_core::types::SearchFilters,
) -> Option<String> {
    if filters.path_glob.is_empty() {
        return None;
    }

    // Best effort: this runs only on an empty result set, and a hint is not
    // worth failing a command over.
    let store =
        vera_core::storage::metadata::MetadataStore::open_existing(&index_dir.join("metadata.db"))
            .ok()?;
    let files = store.indexed_files().ok()?;
    let unmatched = filters.path_patterns_matching_nothing(&files);
    // `path_glob` is OR-combined, so one working pattern still admits files and
    // the empty result is then a genuine miss rather than the filter's doing.
    if unmatched.is_empty() || unmatched.len() != filters.path_glob.len() {
        return None;
    }

    let quoted: Vec<String> = unmatched.iter().map(|p| format!("`{p}`")).collect();
    let suggestions: Vec<String> = unmatched
        .iter()
        .copied()
        .filter_map(|pattern| {
            if let Some(s) = vera_core::types::directory_pattern_suggestion(pattern) {
                return Some(s);
            }
            // Fallback for a literal file miss where the parent directory exists
            // as a file container, e.g. `src/auth.rs` missing while
            // `src/auth/mod.rs` exists suggests `src/auth/**`. The pure
            // directory-pattern helper only handles wildcarded directory prefixes,
            // so a literal file pattern would otherwise get no suggestion even
            // though its directory variant is actionable.
            let trimmed = pattern.trim_end_matches(['/', '\\']);
            if !trimmed.contains('*') {
                let last = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
                if last.contains('.') {
                    // Strip extension to derive a directory candidate, e.g.
                    // `src/auth.rs` -> `src/auth`, `a/b/c.rs` -> `a/b/c`.
                    let stem = last.rsplit_once('.').map_or(last, |(stem, _)| stem);
                    let dir_candidate = if let Some(slash) = trimmed.rfind(['/', '\\']) {
                        let parent = &trimmed[..slash];
                        if parent.is_empty() {
                            stem.to_string()
                        } else {
                            format!("{parent}/{stem}")
                        }
                    } else {
                        // No directory component, e.g. `auth.rs` -> `auth`
                        stem.to_string()
                    };
                    if !dir_candidate.is_empty() {
                        let parent_pat = format!("{dir_candidate}/**");
                        let probe = vera_core::types::SearchFilters {
                            path_glob: vec![parent_pat.clone()],
                            ..Default::default()
                        };
                        if probe.path_patterns_matching_nothing(&files).is_empty() {
                            return Some(format!("`{dir_candidate}/**`"));
                        }
                    }
                }
            }
            None
        })
        .collect();

    let mut hint = format!(
        "note: no indexed file matches {}; the path filter excluded everything, so this is not necessarily an empty search",
        quoted.join(", ")
    );
    if !suggestions.is_empty() {
        hint.push_str(&format!(
            "\n      try a directory pattern: {}",
            suggestions.join(", ")
        ));
    }
    Some(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_path_filter_rewrite_maps_under_root_and_keeps_the_rest() {
        let roots = vec![PathBuf::from("/repo")];

        // Absolute path under the root becomes root-relative.
        assert!(matches!(
            absolute_path_filter_rewrite("/repo/src/auth", &roots),
            PathFilterRewrite::Rewrite(rel) if rel == "src/auth"
        ));
        // Windows-style separators are normalized to forward slashes.
        assert!(matches!(
            absolute_path_filter_rewrite("/repo\\src\\auth", &roots),
            PathFilterRewrite::Rewrite(rel) if rel == "src/auth"
        ));
        assert!(matches!(
            absolute_path_filter_rewrite("C:\\repo\\src", &[PathBuf::from("C:\\repo")]),
            PathFilterRewrite::Rewrite(rel) if rel == "src"
        ));
        // Windows path comparison is case-insensitive; the rewritten glob keeps
        // the pattern's own casing.
        assert!(matches!(
            absolute_path_filter_rewrite_inner(
                "C:\\Repo\\Src",
                &[PathBuf::from("c:\\repo")],
                true
            ),
            PathFilterRewrite::Rewrite(rel) if rel == "Src"
        ));
        assert!(matches!(
            absolute_path_filter_rewrite_inner(
                "C:\\Repo\\Src",
                &[PathBuf::from("c:\\repo")],
                false
            ),
            PathFilterRewrite::Keep
        ));
        // The root itself becomes an empty filter and is dropped.
        assert!(matches!(
            absolute_path_filter_rewrite("/repo", &roots),
            PathFilterRewrite::Drop
        ));
        assert!(matches!(
            absolute_path_filter_rewrite("/repo/", &roots),
            PathFilterRewrite::Drop
        ));
        // Absolute path outside the root is left for `path_filter_hint`.
        assert!(matches!(
            absolute_path_filter_rewrite("/other/src", &roots),
            PathFilterRewrite::Keep
        ));
        // Relative patterns pass through untouched.
        assert!(matches!(
            absolute_path_filter_rewrite("src/**", &roots),
            PathFilterRewrite::Keep
        ));
    }
}
