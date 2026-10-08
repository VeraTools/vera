use std::time::Duration;

/// Cumulative busy time per stage, rounded to milliseconds.
///
/// Parsing, embedding, and storage overlap in a full build, so their sum is
/// not wall-clock elapsed time. Stage time includes I/O and
/// embedding retry waits, but excludes idle time between windows on worker queues.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PhaseSecs {
    pub discovery: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<f64>,
    pub parse: f64,
    pub embed: f64,
    pub store: f64,
}

pub(crate) fn rounded_secs(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_seconds_round_to_milliseconds() {
        assert_eq!(rounded_secs(Duration::from_micros(123_456)), 0.123);
        assert_eq!(rounded_secs(Duration::from_micros(123_789)), 0.124);
    }

    #[tokio::test]
    async fn summaries_count_only_this_run_and_noop_and_local_runs_have_zero_requests() {
        use crate::embedding::test_helpers::MockProvider;
        use crate::embedding::{EmbeddingError, EmbeddingProvider, EmbeddingStats};
        use crate::indexing::{index_repository, update_repository};
        use std::sync::atomic::Ordering;

        struct CountingProvider(EmbeddingStats);
        impl EmbeddingProvider for CountingProvider {
            fn stats(&self) -> Option<&EmbeddingStats> {
                Some(&self.0)
            }
            fn expected_dim(&self) -> Option<usize> {
                Some(8)
            }
            async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
                self.0.requests.fetch_add(1, Ordering::Relaxed);
                MockProvider::new(8).embed_batch(texts).await
            }
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("test.rs"), "pub fn example() {}\n").unwrap();
        let config = crate::config::VeraConfig::default();
        let provider = CountingProvider(EmbeddingStats::default());
        provider.0.requests.store(42, Ordering::Relaxed);
        provider.0.retries.store(7, Ordering::Relaxed);
        let summary = index_repository(root.path(), &provider, &config, "mock")
            .await
            .unwrap();
        assert_eq!(summary.embedding_requests, 1);
        assert_eq!(summary.embedding_retries, 0);
        assert_eq!(summary.embedding_timeouts, 0);
        assert_eq!(summary.embedding_failed_batches, 0);
        let update = update_repository(root.path(), &provider, &config, "mock")
            .await
            .unwrap();
        assert_eq!(update.embedding_requests, 0);
        assert_eq!(update.embedding_retries, 0);
        assert_eq!(update.phase_secs.embed, 0.0);
        assert!(update.phase_secs.classification.is_some());
        let local = index_repository(root.path(), &MockProvider::new(8), &config, "mock")
            .await
            .unwrap();
        assert_eq!(local.embedding_requests, 0);
        assert_eq!(local.embedding_retries, 0);
        assert_eq!(local.embedding_timeouts, 0);
        assert_eq!(local.embedding_failed_batches, 0);
    }
}
