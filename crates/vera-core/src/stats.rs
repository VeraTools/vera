//! Index statistics collection.
//!
//! Provides functions to collect and report statistics about the Vera index,
//! including file count, chunk count, index size on disk, and language breakdown.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::indexing::index_dir;
use crate::storage::metadata::{IndexHealth, MetadataStore, is_entry_point_path};

/// Architecture overview of an indexed repository.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectOverview {
    /// Total files in the index.
    pub file_count: u64,
    /// Total chunks (symbols/blocks).
    pub chunk_count: u64,
    /// Approximate total lines of code.
    pub total_lines: u64,
    /// Index size on disk.
    pub index_size_human: String,
    /// Languages with file counts, sorted by file count descending.
    pub languages: Vec<LanguageOverview>,
    /// Top-level directories with file counts.
    pub top_directories: Vec<DirectoryStat>,
    /// Symbol type breakdown (function, struct, class, etc.).
    pub symbol_types: Vec<SymbolTypeStat>,
    /// Likely entry point files (main.*, index.*, app.*, etc.).
    pub entry_points: Vec<String>,
    /// Files with the most chunks (complexity hotspots).
    pub hotspots: Vec<HotspotFile>,
    /// Detected project conventions (frameworks, patterns, config files).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conventions: Vec<String>,
}

/// Language info for the overview.
#[derive(Debug, Clone, Serialize)]
pub struct LanguageOverview {
    pub language: String,
    pub files: u64,
    pub chunks: u64,
}

/// A top-level directory with its file count.
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryStat {
    pub directory: String,
    pub files: u64,
}

/// Symbol type with count.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolTypeStat {
    pub symbol_type: String,
    pub count: u64,
}

/// A file identified as a complexity hotspot.
#[derive(Debug, Clone, Serialize)]
pub struct HotspotFile {
    pub file_path: String,
    pub chunks: u64,
}

/// Complete statistics about an indexed repository.
#[derive(Debug, Clone, Serialize)]
pub struct IndexStats {
    /// Number of distinct source files in the index.
    pub file_count: u64,
    /// Total number of chunks (symbols/blocks) in the index.
    pub chunk_count: u64,
    /// Total index size on disk in bytes.
    pub index_size_bytes: u64,
    /// Human-readable index size (e.g., "12.3 MB").
    pub index_size_human: String,
    /// Language breakdown: language name -> chunk count.
    pub languages: Vec<LanguageStat>,
    /// Persisted index health from file-level parse diagnostics.
    pub index_health: IndexHealth,
}

/// Statistics for a single programming language.
#[derive(Debug, Clone, Serialize)]
pub struct LanguageStat {
    /// Language name (e.g., "rust", "python").
    pub language: String,
    /// Number of chunks in this language.
    pub chunk_count: u64,
    /// Percentage of total chunks (0.0–100.0).
    pub percentage: f64,
}

/// Collect statistics about the index stored at the given repository path.
///
/// # Arguments
/// - `repo_path` — Path to the repository root (index lives in `.vera/`).
///
/// # Errors
/// Returns an error if the index doesn't exist or can't be read.
pub fn collect_stats(repo_path: &Path) -> Result<IndexStats> {
    let idx_dir = index_dir(repo_path);

    if !idx_dir.exists() {
        anyhow::bail!(
            "no index found at: {}\nRun `vera index <path>` first to create an index.",
            idx_dir.display()
        );
    }

    // Open metadata store. Read-only: a missing database must be an error,
    // never an empty database fabricated by the read itself.
    let metadata_path = idx_dir.join("metadata.db");
    let metadata_store =
        MetadataStore::open_existing(&metadata_path).context("failed to open metadata store")?;
    crate::indexing::freshness::ensure_index_complete(&idx_dir)?;

    // Collect counts.
    let file_count = metadata_store
        .file_count()
        .context("failed to get file count")?;
    let chunk_count = metadata_store
        .chunk_count()
        .context("failed to get chunk count")?;

    // Compute index size on disk.
    let index_size_bytes = compute_dir_size(&idx_dir).context("failed to compute index size")?;
    let index_size_human = format_bytes(index_size_bytes);

    // Collect language breakdown.
    let raw_lang_stats = metadata_store
        .language_stats()
        .context("failed to get language stats")?;

    let languages: Vec<LanguageStat> = raw_lang_stats
        .into_iter()
        .map(|(language, count)| {
            let percentage = if chunk_count > 0 {
                (count as f64 / chunk_count as f64) * 100.0
            } else {
                0.0
            };
            LanguageStat {
                language,
                chunk_count: count,
                percentage,
            }
        })
        .collect();
    let index_health = metadata_store
        .index_health()
        .context("failed to get index health")?;

    Ok(IndexStats {
        file_count,
        chunk_count,
        index_size_bytes,
        index_size_human,
        languages,
        index_health,
    })
}

/// Collect an architecture overview of the indexed repository, optionally
/// filtered to an exact set of file paths.
///
/// Returns a high-level summary: languages, directories, entry points,
/// symbol types, and complexity hotspots. Designed for agent onboarding.
pub fn collect_overview_filtered(
    repo_path: &Path,
    exact_paths: Option<&HashSet<String>>,
) -> Result<ProjectOverview> {
    let idx_dir = index_dir(repo_path);

    if !idx_dir.exists() {
        anyhow::bail!(
            "no index found at: {}\nRun `vera index <path>` first to create an index.",
            idx_dir.display()
        );
    }

    let metadata_path = idx_dir.join("metadata.db");
    let store =
        MetadataStore::open_existing(&metadata_path).context("failed to open metadata store")?;

    let index_size_bytes = compute_dir_size(&idx_dir)?;
    let index_size_human = format_bytes(index_size_bytes);

    if exact_paths.is_none() {
        return collect_full_overview(&store, index_size_human);
    }

    let mut files = store.indexed_files()?;
    if let Some(exact_paths) = exact_paths {
        files.retain(|path| exact_paths.contains(path));
    }

    if files.is_empty() {
        return Ok(ProjectOverview {
            file_count: 0,
            chunk_count: 0,
            total_lines: 0,
            index_size_human,
            languages: Vec::new(),
            top_directories: Vec::new(),
            symbol_types: Vec::new(),
            entry_points: Vec::new(),
            hotspots: Vec::new(),
            conventions: Vec::new(),
        });
    }

    // Two grouped queries replace one content-fetching query per file: the
    // overview only needs counts, line spans, languages, and symbol types.
    let summaries = store.file_chunk_summaries(&files)?;
    let symbol_type_totals = store.symbol_type_counts(&files)?;

    let mut language_files: BTreeMap<String, u64> = BTreeMap::new();
    let mut language_chunks: BTreeMap<String, u64> = BTreeMap::new();
    let mut top_directories: HashMap<String, u64> = HashMap::new();
    let mut hotspots: Vec<(String, u64)> = Vec::with_capacity(files.len());
    let mut entry_points = Vec::new();
    let mut total_lines = 0u64;
    let mut chunk_count = 0u64;

    for file in &files {
        // Files retained by the filter but absent from the index contribute
        // nothing, exactly as when their chunk fetch used to come back empty.
        let Some(summary) = summaries.get(file) else {
            continue;
        };

        chunk_count += summary.chunk_count;
        hotspots.push((file.clone(), summary.chunk_count));

        if is_entry_point_path(file) {
            entry_points.push(file.clone());
        }

        let top_dir = file
            .split('/')
            .next()
            .filter(|dir| !dir.is_empty())
            .unwrap_or(".")
            .to_string();
        *top_directories.entry(top_dir).or_default() += 1;

        *language_files.entry(summary.language.clone()).or_default() += 1;
        *language_chunks.entry(summary.language.clone()).or_default() += summary.chunk_count;

        total_lines += summary.max_line_end as u64;
    }

    let mut languages: Vec<LanguageOverview> = language_chunks
        .into_iter()
        .map(|(language, chunks)| LanguageOverview {
            files: language_files.get(&language).copied().unwrap_or(0),
            language,
            chunks,
        })
        .collect();
    languages.sort_by(|left, right| {
        right
            .files
            .cmp(&left.files)
            .then(right.chunks.cmp(&left.chunks))
            .then(left.language.cmp(&right.language))
    });

    let mut top_directories: Vec<DirectoryStat> = top_directories
        .into_iter()
        .map(|(directory, files)| DirectoryStat { directory, files })
        .collect();
    top_directories.sort_by(|left, right| {
        right
            .files
            .cmp(&left.files)
            .then(left.directory.cmp(&right.directory))
    });
    top_directories.truncate(15);

    let mut symbol_types: Vec<SymbolTypeStat> = symbol_type_totals
        .into_iter()
        .map(|(symbol_type, count)| SymbolTypeStat { symbol_type, count })
        .collect();
    symbol_types.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then(left.symbol_type.cmp(&right.symbol_type))
    });

    hotspots.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    hotspots.truncate(10);
    let hotspots = hotspots
        .into_iter()
        .map(|(file_path, chunks)| HotspotFile { file_path, chunks })
        .collect();

    entry_points.sort();
    let conventions = detect_conventions_from_files(&files);

    Ok(ProjectOverview {
        file_count: files.len() as u64,
        chunk_count,
        total_lines,
        index_size_human,
        languages,
        top_directories,
        symbol_types,
        entry_points,
        hotspots,
        conventions,
    })
}

fn collect_full_overview(
    store: &MetadataStore,
    index_size_human: String,
) -> Result<ProjectOverview> {
    let file_count = store.file_count()?;
    let chunk_count = store.chunk_count()?;
    let total_lines = store.total_lines()?;

    let chunk_stats = store.language_stats()?;
    let file_stats = store.language_file_counts()?;
    let file_map: HashMap<&str, u64> = file_stats
        .iter()
        .map(|(lang, count)| (lang.as_str(), *count))
        .collect();
    let languages = chunk_stats
        .iter()
        .map(|(lang, chunks)| LanguageOverview {
            language: lang.clone(),
            files: file_map.get(lang.as_str()).copied().unwrap_or(0),
            chunks: *chunks,
        })
        .collect();

    let top_directories = store
        .top_directories(15)?
        .into_iter()
        .map(|(directory, files)| DirectoryStat { directory, files })
        .collect();

    let symbol_types = store
        .symbol_type_stats()?
        .into_iter()
        .map(|(symbol_type, count)| SymbolTypeStat { symbol_type, count })
        .collect();

    let entry_points = store.entry_points()?;
    let hotspots = store
        .hotspot_files(10)?
        .into_iter()
        .map(|(file_path, chunks)| HotspotFile { file_path, chunks })
        .collect();
    let conventions = detect_conventions_from_files(&store.indexed_files()?);

    Ok(ProjectOverview {
        file_count,
        chunk_count,
        total_lines,
        index_size_human,
        languages,
        top_directories,
        symbol_types,
        entry_points,
        hotspots,
        conventions,
    })
}

fn detect_conventions_from_files(files: &[String]) -> Vec<String> {
    // Lowercase each path once and split it into components up front; matching
    // then compares whole components, so `src/myterraform/mod.rs` can never
    // satisfy a `terraform` pattern and nothing allocates per pattern x file.
    let lowered_components: Vec<Vec<String>> = files
        .iter()
        .map(|file| {
            file.split(['/', '\\'])
                .filter(|component| !component.is_empty())
                .map(str::to_ascii_lowercase)
                .collect()
        })
        .collect();

    indicators()
        .iter()
        .filter(|(patterns, _label)| {
            patterns
                .iter()
                .any(|pattern| matches_convention_pattern(pattern, &lowered_components))
        })
        .map(|(_patterns, label)| (*label).to_string())
        .collect()
}

/// Whether any indexed path satisfies a convention pattern.
///
/// Single-segment patterns must equal a whole path component; dot-prefixed
/// patterns may instead match a filename suffix (`.proto` matches
/// `schema.proto`). Slash-separated patterns must appear as consecutive
/// components, so `.github/workflows` neither fires on `.github/workflows-old`
/// nor on a bare `.github` directory. Components are pre-lowercased by the
/// caller; comparisons are ASCII case-insensitive so mixed-case pattern
/// literals (`Cargo.toml`, `Dockerfile`) match without allocating.
fn matches_convention_pattern(pattern: &str, files: &[Vec<String>]) -> bool {
    if let Some((head, tail)) = pattern.split_once('/') {
        let parts: Vec<&str> = std::iter::once(head).chain(tail.split('/')).collect();
        return files.iter().any(|components| {
            components.windows(parts.len()).any(|window| {
                window
                    .iter()
                    .zip(&parts)
                    .all(|(component, part)| component.eq_ignore_ascii_case(part))
            })
        });
    }
    files.iter().any(|components| {
        components.iter().any(|component| {
            if pattern.starts_with('.') && component.len() >= pattern.len() {
                let suffix = &component[component.len() - pattern.len()..];
                suffix.eq_ignore_ascii_case(pattern)
            } else {
                component.eq_ignore_ascii_case(pattern)
            }
        })
    })
}

const fn indicators() -> &'static [(&'static [&'static str], &'static str)] {
    &[
        (&["Cargo.toml"], "Rust/Cargo project"),
        (&["package.json"], "Node.js/npm project"),
        (
            &["pyproject.toml", "setup.py", "setup.cfg"],
            "Python project",
        ),
        (&["go.mod"], "Go module"),
        (
            &["pom.xml", "build.gradle", "build.gradle.kts"],
            "Java/JVM project",
        ),
        (&["Gemfile"], "Ruby/Bundler project"),
        (
            &["Dockerfile", "docker-compose.yml", "docker-compose.yaml"],
            "Docker containerization",
        ),
        (&[".github/workflows"], "GitHub Actions CI"),
        (&[".gitlab-ci.yml"], "GitLab CI"),
        (&["Makefile"], "Make build system"),
        (&["tsconfig.json"], "TypeScript project"),
        (
            &[
                ".eslintrc",
                ".eslintrc.json",
                ".eslintrc.js",
                "eslint.config",
            ],
            "ESLint linting",
        ),
        (&[".prettierrc", "prettier.config"], "Prettier formatting"),
        (&["jest.config", "vitest.config"], "JS test framework"),
        (&["next.config"], "Next.js framework"),
        (&["nuxt.config"], "Nuxt.js framework"),
        (&["vite.config"], "Vite build tool"),
        (&["webpack.config"], "Webpack bundler"),
        (&["tailwind.config"], "Tailwind CSS"),
        (&[".env", ".env.example"], "Environment variable config"),
        (&["terraform"], "Terraform infrastructure"),
        (&["k8s", "kubernetes", "helm"], "Kubernetes deployment"),
        (&["proto", ".proto"], "Protocol Buffers"),
        (&["openapi", "swagger"], "OpenAPI/Swagger spec"),
        (&["migrations"], "Database migrations"),
        (&["prisma"], "Prisma ORM"),
        (&[".storybook"], "Storybook UI"),
    ]
}

// ── Call graph queries ───────────────────────────────────────────────

pub use crate::storage::metadata::{CalleeRef, CallerRef, DeadSymbol};

/// Open the metadata store for a repo, or error if no index exists.
fn open_metadata(repo_path: &Path) -> Result<MetadataStore> {
    let idx_dir = index_dir(repo_path);
    if !idx_dir.exists() {
        anyhow::bail!(
            "no index found at: {}\nRun `vera index <path>` first.",
            idx_dir.display()
        );
    }
    MetadataStore::open_existing(&idx_dir.join("metadata.db"))
        .context("failed to open metadata store")
}

/// Find all call sites that reference a given symbol name.
pub fn find_callers(repo_path: &Path, symbol: &str) -> Result<Vec<CallerRef>> {
    open_metadata(repo_path)?.find_callers(symbol)
}

/// Find all symbols called by a given symbol.
pub fn find_callees(repo_path: &Path, symbol: &str) -> Result<Vec<CalleeRef>> {
    open_metadata(repo_path)?.find_callees(symbol)
}

/// Find defined symbols with zero callers (potential dead code).
pub fn find_dead_symbols(repo_path: &Path) -> Result<Vec<DeadSymbol>> {
    open_metadata(repo_path)?.find_dead_symbols()
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Recursively compute the total size of files in a directory.
fn compute_dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0u64;
    if dir.is_file() {
        return Ok(dir.metadata().map(|m| m.len()).unwrap_or(0));
    }
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory: {}", dir.display()))?;
    for entry in entries {
        let entry = entry.context("failed to read directory entry")?;
        let path = entry.path();
        if path.is_dir() {
            total += compute_dir_size(&path)?;
        } else {
            total += path.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(total)
}

/// Format a byte count into a human-readable string.
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VeraConfig;
    use crate::embedding::test_helpers::MockProvider;
    use crate::indexing::index_repository;
    use std::collections::HashSet;

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
    }

    #[test]
    fn collect_stats_missing_index() {
        let dir = std::env::temp_dir().join("vera-stats-test-missing");
        let result = collect_stats(&dir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no index found"), "error was: {err}");
    }

    #[test]
    fn collect_stats_missing_metadata_db_errors_without_creating_it() {
        // A `.vera/` directory without `metadata.db` means a crashed or
        // truncated index: the read must fail with the re-index hint and must
        // not fabricate the database (issue #155).
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".vera")).unwrap();

        let result = collect_stats(dir.path());
        assert!(result.is_err(), "all-zero success for a missing index");
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("no index metadata found"), "error was: {err}");
        assert!(
            !dir.path().join(".vera/metadata.db").exists(),
            "the read created metadata.db"
        );
    }

    #[test]
    fn entry_point_predicate_matches_final_components() {
        assert!(is_entry_point_path("src/main.rs"));
        assert!(is_entry_point_path("server.ts"));
        assert!(!is_entry_point_path("src/main"));
        assert!(!is_entry_point_path("src/domain.rs"));
    }

    #[test]
    fn conventions_match_whole_components_not_substrings() {
        let detect = |files: &[&str]| {
            detect_conventions_from_files(
                &files
                    .iter()
                    .map(|file| file.to_string())
                    .collect::<Vec<_>>(),
            )
        };
        let has_label = |labels: &Vec<String>, label: &str| labels.iter().any(|l| l == label);

        // Real markers still fire, case-insensitively.
        assert!(has_label(
            &detect(&["Cargo.toml", "src/lib.rs"]),
            "Rust/Cargo project"
        ));
        assert!(has_label(
            &detect(&["infra/terraform/main.tf"]),
            "Terraform infrastructure"
        ));
        assert!(has_label(
            &detect(&["deploy/k8s/app.yaml"]),
            "Kubernetes deployment"
        ));
        assert!(has_label(
            &detect(&["src/api/schema.proto"]),
            "Protocol Buffers"
        ));

        // Substring hits must not fire (issue #173).
        assert!(!has_label(
            &detect(&["src/myterraform/mod.rs"]),
            "Terraform infrastructure"
        ));
        assert!(!has_label(
            &detect(&["src/kubernetes_client.go"]),
            "Kubernetes deployment"
        ));
        assert!(!has_label(
            &detect(&["src/protocol.rs"]),
            "Protocol Buffers"
        ));
        assert!(!has_label(
            &detect(&["docs/contrib.md"]),
            "Protocol Buffers"
        ));

        // Multi-segment patterns match consecutive components only: a bare
        // `.github` directory and a `.github/workflows-old` near-miss stay
        // silent, while real workflow files are detected.
        assert!(has_label(
            &detect(&[".github/workflows/ci.yml"]),
            "GitHub Actions CI"
        ));
        assert!(!has_label(
            &detect(&[".github/dependabot.yml"]),
            "GitHub Actions CI"
        ));
        assert!(!has_label(
            &detect(&[".github/workflows-old/ci.yml"]),
            "GitHub Actions CI"
        ));

        // Windows separators split into components like Unix ones.
        assert!(has_label(
            &detect(&["packages\\app\\Dockerfile"]),
            "Docker containerization"
        ));
        assert!(has_label(
            &detect(&["src\\proto\\client.ts"]),
            "Protocol Buffers"
        ));
    }

    #[tokio::test]
    async fn filtered_overview_aggregates_match_per_file_numbers() {
        // The exact-path set must cover every indexed file so the filtered
        // path runs both aggregate queries; asserting against per-file reads
        // keeps the test independent of parser chunking details.
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src").join("main.rs"),
            "fn main() {}\nstruct Config { name: String }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src").join("lib.py"),
            "def hello():\n    pass\n",
        )
        .unwrap();

        let provider = MockProvider::new(8);
        let config = VeraConfig::default();
        index_repository(dir.path(), &provider, &config, "mock-model")
            .await
            .unwrap();

        let store = MetadataStore::open_existing(&dir.path().join(".vera/metadata.db")).unwrap();
        let files = [String::from("src/main.rs"), String::from("src/lib.py")];
        let expected_chunks: u64 = files
            .iter()
            .map(|file| store.get_chunks_by_file(file).unwrap().len() as u64)
            .sum();
        let expected_lines: u64 = files
            .iter()
            .map(|file| {
                store
                    .get_chunks_by_file(file)
                    .unwrap()
                    .iter()
                    .map(|chunk| u64::from(chunk.line_end))
                    .max()
                    .unwrap_or(0)
            })
            .sum();
        let expected_functions: u64 = files
            .iter()
            .map(|file| {
                store
                    .get_chunks_by_file(file)
                    .unwrap()
                    .iter()
                    .filter(|chunk| chunk.symbol_type == Some(crate::types::SymbolType::Function))
                    .count() as u64
            })
            .sum();
        drop(store);

        let exact_paths: HashSet<String> = files.iter().cloned().collect();
        let overview = collect_overview_filtered(dir.path(), Some(&exact_paths)).unwrap();
        assert_eq!(overview.file_count, 2);
        assert_eq!(overview.chunk_count, expected_chunks);
        assert_eq!(overview.total_lines, expected_lines);
        assert_eq!(overview.languages.len(), 2);
        for language in &overview.languages {
            assert_eq!(language.files, 1);
            assert!(language.chunks > 0, "{} has no chunks", language.language);
        }
        let function_total = overview
            .symbol_types
            .iter()
            .find(|stat| stat.symbol_type == "function")
            .map(|stat| stat.count)
            .unwrap_or(0);
        assert_eq!(function_total, expected_functions);
        assert!(overview.entry_points.contains(&String::from("src/main.rs")));
    }
}
