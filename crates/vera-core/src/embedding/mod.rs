//! Embedding generation via external API providers.
//!
//! This module provides:
//! - [`EmbeddingProvider`] trait for abstracting embedding API calls
//! - [`OpenAiProvider`] for OpenAI-compatible embedding endpoints
//! - Batched embedding generation with configurable batch size
//! - Credential management (read from environment, never log)
//! - Error handling (auth failures, connection errors, rate limits)

use std::sync::atomic::{AtomicU64, Ordering};

/// Thread-safe request counters shared by a provider and the embedding queue.
#[derive(Debug, Default)]
pub struct EmbeddingStats {
    pub(crate) requests: AtomicU64,
    pub(crate) retries: AtomicU64,
    pub(crate) timeouts: AtomicU64,
    pub(crate) failed_batches: AtomicU64,
}

/// Request counters captured at the end of an indexing operation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct EmbeddingRequestStats {
    /// HTTP requests sent to the embedding API, counting every attempt.
    pub requests: u64,
    /// requests that repeated a failed request (immediate retries plus requeued resends). `requests - retries` is the number of batches sent.
    pub retries: u64,
    /// requests that timed out.
    pub timeouts: u64,
    /// times a batch exhausted its immediate retries and was requeued or failed the run.
    pub failed_batches: u64,
}

impl EmbeddingRequestStats {
    /// Counters belonging to this run, excluding earlier work by a reused provider.
    pub fn since(self, before: Self) -> Self {
        Self {
            requests: self.requests.saturating_sub(before.requests),
            retries: self.retries.saturating_sub(before.retries),
            timeouts: self.timeouts.saturating_sub(before.timeouts),
            failed_batches: self.failed_batches.saturating_sub(before.failed_batches),
        }
    }
}

impl EmbeddingStats {
    pub fn snapshot(&self) -> EmbeddingRequestStats {
        EmbeddingRequestStats {
            requests: self.requests.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            failed_batches: self.failed_batches.load(Ordering::Relaxed),
        }
    }
}

mod provider;

pub(crate) use provider::embed_chunks_concurrent_with_progress_and_cancellation;
pub use provider::{
    CachedEmbeddingProvider, EmbeddingError, EmbeddingProvider, EmbeddingProviderConfig,
    OpenAiProvider, embed_chunks_concurrent,
};

pub mod dynamic;
pub use dynamic::{DynamicProvider, create_dynamic_provider};

pub mod local_provider;

pub use local_provider::LocalEmbeddingProvider;

pub mod model2vec_provider;
pub use model2vec_provider::Model2VecProvider;

/// Test helpers for creating mock embedding providers.
#[cfg(test)]
pub(crate) mod test_helpers {
    pub use super::provider::test_helpers::MockProvider;

    use super::EmbeddingError;
    use super::provider::EmbeddingProvider;
    use crate::storage::vector::VectorStore;
    use crate::types::Chunk;
    use std::sync::Mutex;

    /// Records requests and fails one chosen batch, without retryable API errors.
    pub(crate) struct CheckpointProvider {
        requests: Mutex<Vec<Vec<String>>>,
        fail_on_call: Option<usize>,
        pub fail_status: u16,
        /// Batches larger than this fail with a context-size error.
        pub max_batch: usize,
        pub prefix: &'static str,
    }

    impl CheckpointProvider {
        pub(crate) fn new(fail_on_call: Option<usize>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                fail_on_call,
                fail_status: 400,
                max_batch: usize::MAX,
                prefix: "passage: ",
            }
        }

        pub(crate) fn inputs(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .cloned()
                .collect()
        }

        pub(crate) fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl EmbeddingProvider for CheckpointProvider {
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            let call = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(texts.to_vec());
                requests.len()
            };
            if texts.len() > self.max_batch {
                return Err(EmbeddingError::ApiError {
                    status: 400,
                    message: "max allowed tokens per submitted batch is 8192".to_string(),
                });
            }
            if self.fail_on_call == Some(call) {
                return Err(EmbeddingError::ApiError {
                    status: self.fail_status,
                    message: "checkpoint test failure".to_string(),
                });
            }
            MockProvider::new(4).embed_batch(texts).await
        }

        fn expected_dim(&self) -> Option<usize> {
            Some(4)
        }
        fn checkpoints_embeddings(&self) -> bool {
            true
        }
        fn prepare_document_text(&self, text: &str) -> String {
            format!("{}{text}", self.prefix)
        }
        fn document_prefix_identity(&self) -> String {
            self.prefix.trim().to_string()
        }
    }

    /// Embed chunks with the provider and insert the vectors into the store.
    pub(crate) async fn embed_and_insert_vectors(
        store: &VectorStore,
        provider: &impl EmbeddingProvider,
        chunks: &[Chunk],
    ) {
        let embeddings =
            super::embed_chunks_concurrent(provider, chunks, chunks.len().max(1), 4, 0)
                .await
                .unwrap();
        let batch: Vec<(&str, &[f32])> = embeddings
            .iter()
            .map(|(id, vec)| (id.as_str(), vec.as_slice()))
            .collect();
        store.insert_batch(&batch).unwrap();
    }
}

#[cfg(test)]
mod tests;
