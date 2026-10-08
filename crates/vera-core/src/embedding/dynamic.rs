use super::EmbeddingStats;
use crate::config::{InferenceBackend, VeraConfig};
use crate::embedding::local_provider::LocalEmbeddingProvider;
use crate::embedding::model2vec_provider::Model2VecProvider;
use crate::embedding::provider::{
    EmbeddingError, EmbeddingProvider, EmbeddingProviderConfig, OpenAiProvider,
};
use crate::local_models::configured_local_model_name;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub enum DynamicProvider {
    Api(OpenAiProvider),
    Local(LocalEmbeddingProvider),
    Model2Vec(Model2VecProvider),
    /// A variant a unit test can hold.
    ///
    /// Every real variant needs its model on disk, and `LocalEmbeddingProvider`
    /// additionally owns an `ort::Session`, so none of them can be built
    /// without ONNX Runtime and a downloaded model. The arms below would then
    /// be unreachable from a test, and dropping one is silent: the trait's
    /// default hooks return the text unchanged, so `vera index` would embed
    /// every passage unprefixed while queries stayed prefixed, with no error,
    /// no log, and an unchanged `model_identity` to keep the staleness guard
    /// quiet.
    // The stub stays crate-private; the variant exists only under cfg(test),
    // so the visibility mismatch cannot leak into the public API.
    #[cfg(test)]
    #[allow(
        private_interfaces,
        reason = "This variant and its private provider exist only in unit-test builds, so no production API exposes the stub."
    )]
    Stub(tests::StubProvider),
}

impl EmbeddingProvider for DynamicProvider {
    fn stats(&self) -> Option<&EmbeddingStats> {
        match self {
            Self::Api(p) => p.stats(),
            Self::Local(p) => p.stats(),
            Self::Model2Vec(p) => p.stats(),
            #[cfg(test)]
            Self::Stub(p) => p.stats(),
        }
    }

    fn requeue_delay(&self, attempt: u32, error: &EmbeddingError) -> Duration {
        match self {
            Self::Api(p) => p.requeue_delay(attempt, error),
            Self::Local(p) => p.requeue_delay(attempt, error),
            Self::Model2Vec(p) => p.requeue_delay(attempt, error),
            #[cfg(test)]
            Self::Stub(p) => p.requeue_delay(attempt, error),
        }
    }

    fn checkpoints_embeddings(&self) -> bool {
        match self {
            Self::Api(p) => p.checkpoints_embeddings(),
            Self::Local(p) => p.checkpoints_embeddings(),
            Self::Model2Vec(p) => p.checkpoints_embeddings(),
            #[cfg(test)]
            Self::Stub(p) => p.checkpoints_embeddings(),
        }
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        match self {
            Self::Api(p) => p.embed_batch(texts).await,
            Self::Local(p) => p.embed_batch(texts).await,
            Self::Model2Vec(p) => p.embed_batch(texts).await,
            #[cfg(test)]
            Self::Stub(p) => p.embed_batch(texts).await,
        }
    }

    fn expected_dim(&self) -> Option<usize> {
        match self {
            Self::Api(p) => p.expected_dim(),
            Self::Local(p) => p.expected_dim(),
            Self::Model2Vec(p) => p.expected_dim(),
            #[cfg(test)]
            Self::Stub(p) => p.expected_dim(),
        }
    }

    fn prepare_document_text(&self, document: &str) -> String {
        match self {
            Self::Api(p) => p.prepare_document_text(document),
            Self::Local(p) => p.prepare_document_text(document),
            Self::Model2Vec(p) => p.prepare_document_text(document),
            #[cfg(test)]
            Self::Stub(p) => p.prepare_document_text(document),
        }
    }

    fn prepare_query_text(&self, query: &str) -> String {
        match self {
            Self::Api(p) => p.prepare_query_text(query),
            Self::Local(p) => p.prepare_query_text(query),
            Self::Model2Vec(p) => p.prepare_query_text(query),
            #[cfg(test)]
            Self::Stub(p) => p.prepare_query_text(query),
        }
    }

    fn document_prefix_identity(&self) -> String {
        match self {
            Self::Api(p) => p.document_prefix_identity(),
            Self::Local(p) => p.document_prefix_identity(),
            Self::Model2Vec(p) => p.document_prefix_identity(),
            #[cfg(test)]
            Self::Stub(p) => p.document_prefix_identity(),
        }
    }

    fn max_batch_size(&self) -> Option<usize> {
        match self {
            Self::Api(p) => p.max_batch_size(),
            Self::Local(p) => p.max_batch_size(),
            Self::Model2Vec(p) => p.max_batch_size(),
            #[cfg(test)]
            Self::Stub(p) => p.max_batch_size(),
        }
    }

    async fn embed_batch_cancellable(
        &self,
        texts: &[String],
        cancel: &CancellationToken,
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        match self {
            Self::Api(p) => p.embed_batch_cancellable(texts, cancel).await,
            Self::Local(p) => p.embed_batch_cancellable(texts, cancel).await,
            Self::Model2Vec(p) => p.embed_batch_cancellable(texts, cancel).await,
            #[cfg(test)]
            Self::Stub(p) => p.embed_batch_cancellable(texts, cancel).await,
        }
    }
}

pub async fn create_dynamic_provider(
    config: &VeraConfig,
    backend: InferenceBackend,
) -> anyhow::Result<(DynamicProvider, String)> {
    match backend {
        InferenceBackend::OnnxJina(ep) => {
            let gpu_mem_limit_mb = config.embedding.gpu_mem_limit_mb;
            let p = LocalEmbeddingProvider::new_with_ep_and_mem_limit(ep, gpu_mem_limit_mb).await.map_err(|e| {
                anyhow::anyhow!("Failed to initialize local embedding provider: {e}\nHint: check network connection or manually place model at ~/.vera/models/")
            })?;
            Ok((DynamicProvider::Local(p), configured_local_model_name()))
        }
        InferenceBackend::PotionCode => {
            let p = Model2VecProvider::new_potion_code().await.map_err(|e| {
                anyhow::anyhow!("Failed to initialize potion-code provider: {e}\nHint: run `vera repair --potion-code` to fetch missing model assets.")
            })?;
            Ok((
                DynamicProvider::Model2Vec(p),
                crate::local_models::potion_code_model_name(),
            ))
        }
        InferenceBackend::Api => {
            let provider_config = EmbeddingProviderConfig::from_env()
                .map_err(|err| anyhow::anyhow!("embedding API not configured: {err}\nHint: set EMBEDDING_MODEL_BASE_URL, EMBEDDING_MODEL_ID, and EMBEDDING_MODEL_API_KEY environment variables, or use --potion-code for local CPU inference."))?;
            let model_name = provider_config.model_id.clone();
            let mut provider_config = provider_config;
            provider_config.query_prefix = saved_prefix(
                provider_config.query_prefix,
                "EMBEDDING_QUERY_PREFIX",
                config.embedding.query_prefix.as_deref(),
            );
            provider_config.document_prefix = saved_prefix(
                provider_config.document_prefix,
                "EMBEDDING_DOCUMENT_PREFIX",
                config.embedding.document_prefix.as_deref(),
            );
            let provider_config = provider_config
                .with_timeout(Duration::from_secs(config.embedding.timeout_secs))
                .with_max_retries(config.embedding.max_retries);
            let p = OpenAiProvider::new(provider_config)
                .map_err(|err| anyhow::anyhow!("failed to initialize embedding provider: {err}"))?;
            Ok((DynamicProvider::Api(p), model_name))
        }
    }
}

/// A saved `embedding.*_prefix` replaces the model default; a non-empty
/// environment override still wins, and an empty saved value means no prefix.
fn saved_prefix(resolved: Option<String>, env_key: &str, saved: Option<&str>) -> Option<String> {
    let env_override = std::env::var(env_key).is_ok_and(|value| !value.is_empty());
    match saved {
        Some(saved) if !env_override => Some(saved.to_owned()).filter(|s| !s.is_empty()),
        _ => resolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_prefix_overrides_model_default_unless_environment_sets_one() {
        let key = "VERA_TEST_UNSET_EMBEDDING_PREFIX_KEY";
        let default = || Some("model: ".to_string());
        assert_eq!(
            saved_prefix(default(), key, Some("saved: ")).as_deref(),
            Some("saved: ")
        );
        assert_eq!(saved_prefix(default(), key, Some("")), None);
        assert_eq!(
            saved_prefix(default(), key, None).as_deref(),
            Some("model: ")
        );
        let path = std::env::var("PATH").unwrap();
        assert_eq!(
            saved_prefix(Some(path.clone()), "PATH", Some("saved: ")),
            Some(path)
        );
    }

    /// Rewrites both sides, so a dispatch arm that stops forwarding is visible:
    /// the trait defaults hand the text back unchanged.
    pub(super) struct StubProvider;

    impl EmbeddingProvider for StubProvider {
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            Ok(texts.iter().map(|_| vec![0.0]).collect())
        }

        fn expected_dim(&self) -> Option<usize> {
            Some(1)
        }

        fn checkpoints_embeddings(&self) -> bool {
            true
        }

        fn prepare_document_text(&self, document: &str) -> String {
            format!("Document: {document}")
        }

        fn prepare_query_text(&self, query: &str) -> String {
            format!("Query: {query}")
        }
    }

    /// `create_dynamic_provider` is what `vera index` embeds through, so a
    /// document hook that stops at `DynamicProvider` never reaches the model's
    /// configured prefix.
    #[test]
    fn dynamic_provider_forwards_the_document_hook() {
        assert_eq!(
            DynamicProvider::Stub(StubProvider).prepare_document_text("fn main() {}"),
            "Document: fn main() {}"
        );
    }

    #[test]
    fn dynamic_provider_forwards_the_query_hook() {
        assert_eq!(
            DynamicProvider::Stub(StubProvider).prepare_query_text("find main"),
            "Query: find main"
        );
    }

    #[test]
    fn dynamic_provider_forwards_checkpoint_support() {
        assert!(DynamicProvider::Stub(StubProvider).checkpoints_embeddings());
    }
}
