//! Read-only estimates from sampled parsing and embedding work.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::CancellationToken;
use crate::config::VeraConfig;
use crate::discovery::discover_files_with_cancellation;
use crate::embedding::{EmbeddingProvider, embed_chunks_concurrent_with_progress_and_cancellation};

use super::pipeline::parse_discovered_files_parallel;

const SAMPLE_FILE_LIMIT: usize = 1000;
const EMBED_SAMPLE_CHUNKS: usize = 256;

#[derive(Debug, Clone, Serialize)]
pub struct IndexEstimate {
    pub total_files: usize,
    pub total_bytes: u64,
    pub binary_skipped: usize,
    pub large_skipped: usize,
    pub sampled_files: usize,
    pub estimated_chunks: u64,
    pub estimated_parse_secs: f64,
    pub embedded_sample_chunks: usize,
    pub estimated_embedding_secs: Option<f64>,
}

/// Estimate a fresh index without locks, checkpoints, or index writes.
/// A provider, when supplied, embeds a small sample to measure its throughput.
pub async fn estimate_index<P: EmbeddingProvider>(
    repo_path: &Path,
    provider: Option<&P>,
    config: &VeraConfig,
) -> Result<IndexEstimate> {
    if !repo_path.exists() {
        bail!("path does not exist: {}", repo_path.display());
    }
    if !repo_path.is_dir() {
        bail!("path is not a directory: {}", repo_path.display());
    }
    let repo_root = repo_path
        .canonicalize()
        .with_context(|| format!("failed to resolve path: {}", repo_path.display()))?;
    let parse_config = config.clone();
    let (mut estimate, chunks) = tokio::task::spawn_blocking(move || -> Result<_> {
        let cancellation = CancellationToken::new();
        let discovery =
            discover_files_with_cancellation(&repo_root, &parse_config.indexing, &cancellation)
                .context("file discovery failed")?;
        let files = stride_sample(&discovery.files, SAMPLE_FILE_LIMIT);
        let total_bytes: u64 = discovery.files.iter().map(|file| file.size).sum();
        let sampled_bytes: u64 = files.iter().map(|file| file.size).sum();
        let scale = if sampled_bytes == 0 {
            1.0
        } else {
            total_bytes as f64 / sampled_bytes as f64
        };
        let start = Instant::now();
        let (chunks, ..) = parse_discovered_files_parallel(
            &discovery,
            &files,
            &repo_root,
            &parse_config,
            &cancellation,
        )?;
        let estimated_parse_secs = start.elapsed().as_secs_f64() * scale;
        let estimate = IndexEstimate {
            total_files: discovery.files.len(),
            total_bytes,
            binary_skipped: discovery.binary_skipped,
            large_skipped: discovery.large_skipped,
            sampled_files: files.len(),
            estimated_chunks: (chunks.len() as f64 * scale).round() as u64,
            estimated_parse_secs,
            embedded_sample_chunks: 0,
            estimated_embedding_secs: None,
        };
        Ok((estimate, chunks))
    })
    .await
    .context("estimate parsing task failed")??;

    if let Some(provider) = provider.filter(|_| !chunks.is_empty()) {
        let sample = stride_sample(&chunks, EMBED_SAMPLE_CHUNKS);
        let (batch_size, max_concurrent) = config.embedding.bounded_parallelism();
        let cancellation = CancellationToken::new();
        let start = Instant::now();
        let vectors = embed_chunks_concurrent_with_progress_and_cancellation(
            provider,
            &sample,
            batch_size,
            max_concurrent,
            config.indexing.max_chunk_bytes,
            cancellation.as_async_token(),
            None,
            |_, _| {},
        )
        .await
        .context("embedding estimate failed")?;
        estimate.embedded_sample_chunks = vectors.len();
        estimate.estimated_embedding_secs = Some(
            start.elapsed().as_secs_f64() * estimate.estimated_chunks as f64 / vectors.len() as f64,
        );
    }
    Ok(estimate)
}

fn stride_sample<T: Clone>(items: &[T], limit: usize) -> Vec<T> {
    items
        .iter()
        .step_by(items.len().div_ceil(limit).max(1))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::test_helpers::MockProvider;

    #[tokio::test]
    async fn estimate_totals_and_timings_without_writes() {
        let scratch =
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        let dir = tempfile::tempdir_in(scratch).unwrap();
        let source = "pub fn sampled_function() {}\n";
        for name in ["one.rs", "two.rs", "three.rs"] {
            std::fs::write(dir.path().join(name), source).unwrap();
        }
        std::fs::write(dir.path().join("binary.bin"), [0, 1, 2, 3]).unwrap();
        std::fs::write(dir.path().join("large.rs"), "x".repeat(4096)).unwrap();
        let mut config = VeraConfig::default();
        config.indexing.no_ignore = true;
        config.indexing.max_file_size_bytes = 512;
        let provider = MockProvider::new(4);

        for provider in [Some(&provider), None] {
            let estimate = estimate_index(dir.path(), provider, &config).await.unwrap();
            assert_eq!(estimate.total_files, 3);
            assert_eq!(estimate.total_bytes, (3 * source.len()) as u64);
            assert_eq!(estimate.sampled_files, 3);
            assert_eq!(estimate.binary_skipped, 1);
            assert_eq!(estimate.large_skipped, 1);
            assert!(estimate.estimated_chunks > 0);
            assert!(estimate.estimated_parse_secs.is_finite());
            assert_eq!(
                estimate.estimated_embedding_secs.is_some(),
                provider.is_some()
            );
            assert_eq!(estimate.embedded_sample_chunks > 0, provider.is_some());
            assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".vera")
            }));
        }
    }

    #[tokio::test]
    async fn estimate_empty_and_invalid_paths() {
        let scratch =
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        let dir = tempfile::tempdir_in(scratch).unwrap();
        let config = VeraConfig::default();
        let provider = MockProvider::new(4);
        let estimate = estimate_index(dir.path(), Some(&provider), &config)
            .await
            .unwrap();
        assert_eq!(estimate.total_files, 0);
        assert_eq!(estimate.total_bytes, 0);
        assert_eq!(estimate.estimated_chunks, 0);
        assert_eq!(estimate.embedded_sample_chunks, 0);
        assert_eq!(estimate.estimated_embedding_secs, None);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        let missing = dir.path().join("missing");
        assert!(
            estimate_index(&missing, Some(&provider), &config)
                .await
                .is_err()
        );
        std::fs::write(&missing, "not a directory").unwrap();
        assert!(
            estimate_index(&missing, Some(&provider), &config)
                .await
                .is_err()
        );
    }

    #[test]
    fn estimate_samples_are_evenly_strided_and_bounded() {
        for (count, limit) in [
            (0_usize, SAMPLE_FILE_LIMIT),
            (3, SAMPLE_FILE_LIMIT),
            (2001, SAMPLE_FILE_LIMIT),
            (600, EMBED_SAMPLE_CHUNKS),
        ] {
            let items: Vec<_> = (0..count).collect();
            let sample = stride_sample(&items, limit);
            let step = count.div_ceil(limit).max(1);
            assert!(sample.len() <= limit);
            assert_eq!(sample, (0..count).step_by(step).collect::<Vec<_>>());
        }
    }
}
