//! Sample-based estimates for indexing a repository.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use serde::Serialize;

use crate::config::VeraConfig;
use crate::discovery::{self, DiscoveredFile};
use crate::parsing;
use crate::types::Chunk;

/// Estimate statistics and the sampled chunks used for optional provider benchmarks.
#[derive(Debug)]
pub struct IndexEstimateSample {
    pub estimate: IndexEstimate,
    pub chunks: Vec<Chunk>,
}

impl IndexEstimateSample {
    /// Select a reproducible random subset of sampled chunks for embedding benchmarks.
    pub fn benchmark_chunks(&self, limit: usize) -> Vec<Chunk> {
        let mut chunks = self.chunks.iter().collect::<Vec<_>>();
        chunks.sort_by(|left, right| left.id.cmp(&right.id));
        let mut rng = StdRng::seed_from_u64(self.estimate.seed ^ 0xd1b54a32d192ed03);
        chunks.shuffle(&mut rng);
        chunks.into_iter().take(limit).cloned().collect()
    }
}

/// Workload estimate derived from a stratified sample of eligible files.
#[derive(Debug, Clone, Serialize)]
pub struct IndexEstimate {
    pub total_files: usize,
    pub total_bytes: u64,
    pub sampled_files: usize,
    pub sampled_bytes: u64,
    pub sampled_chunks: usize,
    pub estimated_chunks: f64,
    pub chunk_count_95_interval: (f64, f64),
    pub chunk_count_interval_method: String,
    pub discovery_secs: f64,
    pub sampled_parse_secs: f64,
    pub estimated_parse_secs: f64,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Stratum {
    language: String,
    size_bucket: u32,
}

/// Discover eligible files, sample within language/size strata, and parse only
/// the sample with the same parser and chunker used by indexing.
pub fn sample_index_work(
    repo_path: &Path,
    config: &VeraConfig,
    sample_file_limit: usize,
    seed: Option<u64>,
) -> Result<IndexEstimateSample> {
    if sample_file_limit == 0 {
        bail!("sample file limit must be greater than zero");
    }

    let root = repo_path
        .canonicalize()
        .with_context(|| format!("failed to resolve path: {}", repo_path.display()))?;
    if !root.is_dir() {
        bail!("path is not a directory: {}", root.display());
    }

    let discovery_start = Instant::now();
    let discovery =
        discovery::discover_files(&root, &config.indexing).context("file discovery failed")?;
    let discovery_secs = discovery_start.elapsed().as_secs_f64();

    let total_files = discovery.files.len();
    let total_bytes = discovery.files.iter().map(|file| file.size).sum();
    if total_files == 0 {
        return Ok(IndexEstimateSample {
            estimate: IndexEstimate {
                total_files: 0,
                total_bytes: 0,
                sampled_files: 0,
                sampled_bytes: 0,
                sampled_chunks: 0,
                estimated_chunks: 0.0,
                chunk_count_95_interval: (0.0, 0.0),
                chunk_count_interval_method: "census".to_string(),
                discovery_secs,
                sampled_parse_secs: 0.0,
                estimated_parse_secs: 0.0,
                seed: seed.unwrap_or_else(random_seed),
            },
            chunks: Vec::new(),
        });
    }

    let seed = seed.unwrap_or_else(random_seed);
    let mut strata: BTreeMap<Stratum, Vec<&DiscoveredFile>> = BTreeMap::new();
    for file in &discovery.files {
        let language = crate::indexing::update::detect_language_for_path(&file.relative_path);
        strata
            .entry(Stratum {
                language: language.to_string(),
                size_bucket: size_bucket(file.size),
            })
            .or_default()
            .push(file);
    }

    let allocations = allocate_sample_counts(
        strata.values().map(Vec::len).collect(),
        sample_file_limit.min(total_files),
    );

    let mut rng = StdRng::seed_from_u64(seed);
    let parse_start = Instant::now();
    let mut sampled_files = 0usize;
    let mut sampled_bytes = 0u64;
    let mut sampled_chunks = Vec::new();
    let mut estimated_chunks = 0.0;
    let mut stratum_variances = Vec::with_capacity(strata.len());
    let mut estimated_parse_secs = 0.0;

    for ((stratum, files), sample_count) in strata.iter().zip(allocations) {
        let mut selected = files.clone();
        selected.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        selected.shuffle(&mut rng);
        selected.truncate(sample_count);
        selected.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        let stratum_bytes: u64 = files.iter().map(|file| file.size).sum();
        let mut sample_byte_total = 0u64;
        let mut sample_chunk_total = 0usize;
        let mut file_observations = Vec::with_capacity(selected.len());
        let stratum_parse_start = Instant::now();

        for file in selected {
            let source = discovery::read_source_lossy_at(
                &discovery.root_dir,
                Path::new(&file.relative_path),
            )
            .with_context(|| format!("failed to read sampled file {}", file.relative_path))?;
            let language = crate::indexing::update::detect_language_for_path(&file.relative_path);
            debug_assert_eq!(language.to_string(), stratum.language);
            let (chunks, _, _) = parsing::parse_file_with_diagnostics(
                &source,
                &file.relative_path,
                language,
                &config.indexing,
            )
            .with_context(|| format!("failed to parse sampled file {}", file.relative_path))?;
            let chunk_count = chunks.len();
            sample_byte_total += file.size;
            sample_chunk_total += chunk_count;
            sampled_files += 1;
            sampled_bytes += file.size;
            file_observations.push((file.size as f64, chunk_count as f64));
            sampled_chunks.extend(chunks);
        }

        let sample_parse_secs = stratum_parse_start.elapsed().as_secs_f64();
        if sample_byte_total > 0 {
            estimated_parse_secs +=
                sample_parse_secs * stratum_bytes as f64 / sample_byte_total as f64;
        }

        let ratio = sample_chunk_total as f64 / sample_byte_total as f64;
        let estimated_stratum_chunks = stratum_bytes as f64 * ratio;
        estimated_chunks += estimated_stratum_chunks;

        let population_count = files.len();
        let sample_count = file_observations.len();
        let residual_variance = if sample_count >= 2 {
            let residuals: Vec<f64> = file_observations
                .iter()
                .map(|(bytes, chunks)| chunks - ratio * bytes)
                .collect();
            let residual_mean = residuals.iter().sum::<f64>() / sample_count as f64;
            Some(
                residuals
                    .iter()
                    .map(|residual| (residual - residual_mean).powi(2))
                    .sum::<f64>()
                    / (sample_count - 1) as f64,
            )
        } else {
            None
        };
        stratum_variances.push((
            stratum_bytes as f64,
            sample_byte_total as f64,
            sample_count,
            population_count,
            residual_variance,
        ));
    }

    let sampled_parse_secs = parse_start.elapsed().as_secs_f64();
    let needs_variance_borrowing =
        stratum_variances
            .iter()
            .any(|(_, _, sample_count, population_count, variance)| {
                sample_count < population_count
                    && (*sample_count < 2 || variance.is_none_or(|variance| variance == 0.0))
            });
    let pooled_residual_variance = {
        let (sum_squares, degrees_of_freedom) = stratum_variances
            .iter()
            .filter_map(|(_, _, sample_count, _, variance)| {
                variance.map(|variance| (variance * (sample_count - 1) as f64, sample_count - 1))
            })
            .fold((0.0, 0usize), |(sum, df), (squares, added_df)| {
                (sum + squares, df + added_df)
            });
        (degrees_of_freedom > 0).then(|| sum_squares / degrees_of_freedom as f64)
    };
    let needs_poisson_floor =
        needs_variance_borrowing && pooled_residual_variance.is_none_or(|variance| variance == 0.0);
    let fallback_variance = if needs_poisson_floor {
        (sampled_chunks.len() as f64 / sampled_files.max(1) as f64).max(1.0)
    } else {
        pooled_residual_variance.unwrap_or_default()
    };
    let mut estimated_variance = 0.0;
    for (stratum_bytes, sample_byte_total, sample_count, population_count, residual_variance) in
        &stratum_variances
    {
        if sample_count == population_count {
            continue;
        }
        let variance = residual_variance
            .filter(|variance| *variance > 0.0)
            .unwrap_or(fallback_variance);
        let mean_bytes = sample_byte_total / *sample_count as f64;
        let sampling_fraction = *sample_count as f64 / *population_count as f64;
        estimated_variance += stratum_bytes.powi(2) * (1.0 - sampling_fraction) * variance
            / (*sample_count as f64 * mean_bytes.powi(2));
    }
    let margin = 1.96 * estimated_variance.sqrt();
    let chunk_count_95_interval = (
        (estimated_chunks - margin).max(0.0),
        estimated_chunks + margin,
    );
    let chunk_count_interval_method = if needs_poisson_floor {
        "Poisson count-variance floor".to_string()
    } else if needs_variance_borrowing {
        "pooled residual variance".to_string()
    } else if stratum_variances
        .iter()
        .all(|(_, _, sample_count, population_count, _)| sample_count == population_count)
    {
        "census".to_string()
    } else {
        "stratified residual variance".to_string()
    };

    Ok(IndexEstimateSample {
        estimate: IndexEstimate {
            total_files,
            total_bytes,
            sampled_files,
            sampled_bytes,
            sampled_chunks: sampled_chunks.len(),
            estimated_chunks,
            chunk_count_95_interval,
            chunk_count_interval_method,
            discovery_secs,
            sampled_parse_secs,
            estimated_parse_secs,
            seed,
        },
        chunks: sampled_chunks,
    })
}

fn size_bucket(size: u64) -> u32 {
    u64::BITS - size.max(1).leading_zeros() - 1
}

fn allocate_sample_counts(populations: Vec<usize>, requested: usize) -> Vec<usize> {
    let mut allocated: Vec<usize> = populations
        .iter()
        .map(|population| usize::from(*population > 0))
        .collect();
    let minimum = allocated.iter().sum::<usize>();
    let target = requested.max(minimum);
    let mut remaining = target.saturating_sub(minimum);

    while remaining > 0 {
        let next = populations
            .iter()
            .enumerate()
            .filter(|(index, population)| allocated[*index] < **population)
            .max_by(
                |(left_index, left_population), (right_index, right_population)| {
                    let left_share = **left_population as f64 / (allocated[*left_index] + 1) as f64;
                    let right_share =
                        **right_population as f64 / (allocated[*right_index] + 1) as f64;
                    left_share.total_cmp(&right_share)
                },
            )
            .map(|(index, _)| index);
        let Some(index) = next else {
            break;
        };
        allocated[index] += 1;
        remaining -= 1;
    }
    allocated
}

fn random_seed() -> u64 {
    rand::random()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_allocations_respect_stratum_sizes_and_budget() {
        let allocations = allocate_sample_counts(vec![100, 20, 1], 12);
        assert_eq!(allocations.iter().sum::<usize>(), 12);
        assert!(allocations[0] <= 100);
        assert!(allocations[1] <= 20);
        assert_eq!(allocations[2], 1);
        assert!(allocations[0] >= allocations[1]);
    }

    #[test]
    fn sample_allocations_cover_each_nonempty_stratum() {
        let allocations = allocate_sample_counts(vec![3, 0, 7], 1);
        assert_eq!(allocations, vec![1, 0, 1]);
    }

    #[test]
    fn size_buckets_are_logarithmic() {
        assert_eq!(size_bucket(1), 0);
        assert_eq!(size_bucket(2), 1);
        assert_eq!(size_bucket(3), 1);
        assert_eq!(size_bucket(4), 2);
    }

    #[test]
    fn file_sample_is_reproducible_unique_and_seed_dependent() {
        let repo = tempfile::tempdir().unwrap();
        for index in 0..32 {
            std::fs::write(
                repo.path().join(format!("doc-{index:02}.md")),
                "# Heading\n\nA short section with the same size.\n",
            )
            .unwrap();
        }

        let sample_paths = |seed| {
            sample_index_work(repo.path(), &VeraConfig::default(), 8, Some(seed))
                .unwrap()
                .chunks
                .iter()
                .map(|chunk| chunk.file_path.clone())
                .collect::<Vec<_>>()
        };
        let first = sample_paths(42);
        assert_eq!(first, sample_paths(42));
        assert_eq!(first.len(), 8);
        assert_eq!(
            first.iter().collect::<std::collections::HashSet<_>>().len(),
            8
        );
        assert_ne!(first, sample_paths(43));
    }

    #[test]
    fn empty_repository_has_zero_estimate() {
        let repo = tempfile::tempdir().unwrap();
        let estimate = sample_index_work(repo.path(), &VeraConfig::default(), 10, Some(1)).unwrap();
        assert_eq!(estimate.estimate.total_files, 0);
        assert_eq!(estimate.estimate.estimated_chunks, 0.0);
        assert_eq!(estimate.estimate.chunk_count_95_interval, (0.0, 0.0));
        assert_eq!(estimate.estimate.chunk_count_interval_method, "census");
    }

    #[test]
    fn census_sample_reports_exact_chunk_total() {
        let repo = tempfile::tempdir().unwrap();
        for index in 0..4 {
            std::fs::write(
                repo.path().join(format!("doc-{index}.md")),
                "# Heading\n\nA short section.\n",
            )
            .unwrap();
        }

        let sample = sample_index_work(repo.path(), &VeraConfig::default(), 4, Some(42)).unwrap();
        assert_eq!(sample.estimate.total_files, 4);
        assert_eq!(sample.estimate.sampled_files, 4);
        assert_eq!(
            sample.estimate.estimated_chunks,
            sample.estimate.sampled_chunks as f64
        );
        assert_eq!(
            sample.estimate.chunk_count_95_interval,
            (
                sample.estimate.sampled_chunks as f64,
                sample.estimate.sampled_chunks as f64
            )
        );
        assert_eq!(sample.estimate.chunk_count_interval_method, "census");
    }

    #[test]
    fn non_census_sample_always_reports_a_model_based_interval() {
        let repo = tempfile::tempdir().unwrap();
        for index in 0..8 {
            std::fs::write(
                repo.path().join(format!("doc-{index}.md")),
                format!("# Heading {index}\n\nA short section.\n"),
            )
            .unwrap();
        }

        let sample = sample_index_work(repo.path(), &VeraConfig::default(), 2, Some(42)).unwrap();
        let (lower, upper) = sample.estimate.chunk_count_95_interval;
        assert!(lower <= sample.estimate.estimated_chunks);
        assert!(sample.estimate.estimated_chunks <= upper);
        assert!(upper > lower);
        assert_eq!(
            sample.estimate.chunk_count_interval_method,
            "Poisson count-variance floor"
        );
    }
}
