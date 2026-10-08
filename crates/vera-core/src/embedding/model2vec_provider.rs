use std::path::PathBuf;
use std::sync::Arc;

use model2vec_rs::model::StaticModel;
use rayon::prelude::*;
use tokio::task;

use crate::embedding::provider::{EmbeddingError, EmbeddingProvider};
use crate::local_models::{POTION_CODE_DIM, POTION_CODE_MAX_LENGTH};

const CANCELLABLE_BATCH_SIZE: usize = 64;

pub struct Model2VecProvider {
    model: Arc<StaticModel>,
    dim: usize,
    max_length: usize,
    batch_size: usize,
}

impl Model2VecProvider {
    pub async fn new_potion_code() -> anyhow::Result<Self> {
        let model_dir = crate::local_models::ensure_potion_code_assets().await?;
        Self::from_cached_potion_code_dir(model_dir).await
    }

    pub async fn from_cached_potion_code_dir(model_dir: PathBuf) -> anyhow::Result<Self> {
        let load_dir = model_dir.clone();
        let model = task::spawn_blocking(move || {
            StaticModel::from_pretrained(load_dir, None, Some(true), None)
        })
        .await
        .map_err(|err| anyhow::anyhow!("failed to join potion-code loader: {err}"))??;

        Ok(Self {
            model: Arc::new(model),
            dim: POTION_CODE_DIM,
            max_length: POTION_CODE_MAX_LENGTH,
            batch_size: 1024,
        })
    }

    pub fn probe_inference() -> anyhow::Result<()> {
        let model_dir = crate::local_models::potion_code_model_dir()?;
        for asset in crate::local_models::inspect_potion_code_model_files()? {
            if !asset.exists {
                anyhow::bail!("missing {} at {}", asset.name, asset.path.display());
            }
        }

        let model = StaticModel::from_pretrained(model_dir, None, Some(true), None)?;
        let embeddings = model.encode(&["vera doctor probe".to_string()]);
        let Some(vector) = embeddings.first() else {
            anyhow::bail!("potion-code returned no embeddings");
        };
        if vector.len() != POTION_CODE_DIM || !vector.iter().all(|value| value.is_finite()) {
            anyhow::bail!(
                "potion-code returned invalid embedding: dim={}, expected={}",
                vector.len(),
                POTION_CODE_DIM
            );
        }
        Ok(())
    }

    async fn encode_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let model = Arc::clone(&self.model);
        let texts = texts.to_vec();
        let input_len = texts.len();
        let max_length = self.max_length;
        let expected_dim = self.dim;

        let vectors = task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // The tokenizer pads each batch to its longest text and the
                // pad tokens are pooled, so a vector would depend on its batch
                // neighbors. One-text batches match how queries are embedded.
                texts
                    .par_iter()
                    .map(|text| {
                        model
                            .encode_with_args(std::slice::from_ref(text), Some(max_length), 1)
                            .pop()
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
            }))
            .map_err(|_| EmbeddingError::ResponseError {
                message: "potion-code tokenization failed".to_string(),
            })
        })
        .await
        .map_err(|err| EmbeddingError::ConnectionError {
            message: format!("potion-code worker failed: {err}"),
        })??;

        if vectors.len() != input_len {
            return Err(EmbeddingError::ResponseError {
                message: format!(
                    "potion-code returned {} vectors for {} inputs",
                    vectors.len(),
                    input_len
                ),
            });
        }

        if let Some(bad_dim) = vectors
            .iter()
            .map(Vec::len)
            .find(|&dim| dim != expected_dim)
        {
            return Err(EmbeddingError::ResponseError {
                message: format!(
                    "potion-code returned {bad_dim}-dim vectors, expected {expected_dim}"
                ),
            });
        }

        Ok(vectors)
    }
}

impl EmbeddingProvider for Model2VecProvider {
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.encode_batch(texts).await
    }

    async fn embed_batch_cancellable(
        &self,
        texts: &[String],
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let batch_size = self.batch_size.clamp(1, CANCELLABLE_BATCH_SIZE);
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(batch_size) {
            if cancel.is_cancelled() {
                return Err(EmbeddingError::Cancelled);
            }
            vectors.extend(self.encode_batch(batch).await?);
        }
        if cancel.is_cancelled() {
            return Err(EmbeddingError::Cancelled);
        }

        Ok(vectors)
    }

    fn expected_dim(&self) -> Option<usize> {
        Some(self.dim)
    }

    fn max_batch_size(&self) -> Option<usize> {
        Some(self.batch_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires the cached Potion Code model"]
    async fn vectors_do_not_depend_on_batch_neighbors() {
        let model_dir = crate::local_models::potion_code_model_dir().unwrap();
        let provider = Model2VecProvider::from_cached_potion_code_dir(model_dir)
            .await
            .unwrap();
        let short = "fn add(a: i32) -> i32".to_string();
        let long = "pub fn parse_configuration_file(path: &Path) -> Result<Config> { ".repeat(20);
        let alone = provider
            .embed_batch(std::slice::from_ref(&short))
            .await
            .unwrap();
        let batched = provider.embed_batch(&[long, short]).await.unwrap();
        assert_eq!(alone[0], batched[1]);
    }
}
