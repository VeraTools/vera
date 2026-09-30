//! Embedding limits and compatible model identities.

use serde::{Deserialize, Serialize};

use super::{env_usize, is_local_mode};

/// Configuration for the embedding provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Batch size for embedding API calls.
    pub batch_size: usize,
    /// Maximum number of concurrent embedding API requests.
    pub max_concurrent_requests: usize,
    /// Hard limit on the total number of embedding inputs that may be active
    /// across concurrent API requests. This bounds abandoned backend work when
    /// an indexing client disconnects.
    #[serde(default = "default_max_in_flight_inputs")]
    pub max_in_flight_inputs: usize,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum retries on transient errors.
    pub max_retries: u32,
    /// Maximum stored vector dimensionality.
    ///
    /// If the embedding model produces vectors larger than this, they
    /// are truncated to this dimensionality before storage. Qwen3 models
    /// support Matryoshka-style truncation, so lower dimensions still
    /// yield good retrieval quality while dramatically reducing index size.
    /// Set to 0 to store full-dimensionality vectors.
    pub max_stored_dim: usize,
    /// GPU memory limit in MB for ONNX CUDA sessions.
    /// 0 means no limit (ORT default: use all available VRAM).
    #[serde(default)]
    pub gpu_mem_limit_mb: u64,
    /// When true, forces conservative GPU settings (batch_size=1, low mem limit).
    #[serde(default)]
    pub low_vram: bool,
    /// Optional API query prefix override.
    #[serde(default)]
    pub query_prefix: Option<String>,
    /// Optional API document prefix override.
    #[serde(default)]
    pub document_prefix: Option<String>,
    /// Equivalent embedding model names.
    ///
    /// OpenAI-compatible providers sometimes expose a deployment alias while the
    /// embedding response, stored index metadata, or another compatible gateway
    /// reports the canonical upstream model name. Each inner list is one
    /// equivalence class. When two model names normalize into the same list, Vera
    /// treats them as index-compatible after the existing dimension check passes.
    /// Only alias models you have verified produce compatible embeddings.
    ///
    /// Can also be supplied with `VERA_EMBEDDING_MODEL_ALIASES`, using
    /// semicolon-separated groups of comma-separated aliases:
    /// `text-embedding-3-large,text-embedding-3-large-2;model-a,model-a-prod`
    #[serde(default)]
    pub model_aliases: Vec<Vec<String>>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        let is_local = is_local_mode();
        Self {
            batch_size: if is_local { 4 } else { 128 },
            max_concurrent_requests: if is_local { 1 } else { 8 },
            max_in_flight_inputs: default_max_in_flight_inputs(),
            timeout_secs: 60,
            max_retries: 3,
            max_stored_dim: 1024,
            gpu_mem_limit_mb: 0,
            low_vram: false,
            query_prefix: None,
            document_prefix: None,
            model_aliases: Vec::new(),
        }
    }
}

fn default_max_in_flight_inputs() -> usize {
    env_usize("VERA_MAX_IN_FLIGHT_INPUTS", 16).max(1)
}

impl EmbeddingConfig {
    /// Clamp configured batching so the product of batch size and concurrency
    /// never exceeds `max_in_flight_inputs`.
    pub fn bounded_parallelism(&self) -> (usize, usize) {
        let max_in_flight = self.max_in_flight_inputs.max(1);
        let batch_size = self.batch_size.max(1).min(max_in_flight);
        let max_concurrent_requests = self
            .max_concurrent_requests
            .max(1)
            .min((max_in_flight / batch_size).max(1));
        (batch_size, max_concurrent_requests)
    }
}

/// Check whether two model names refer to the same model, using configured
/// alias groups plus aliases supplied by `VERA_EMBEDDING_MODEL_ALIASES`.
///
/// Model names may differ only by an org/repo prefix (e.g.
/// `"jinaai/jina-embeddings-v5-text-nano-retrieval"` vs
/// `"jina-embeddings-v5-text-nano-retrieval"`). Both names are normalised by
/// stripping everything up to and including the last `/` and then compared
/// case-insensitively.
pub fn model_names_match_with_aliases(a: &str, b: &str, aliases: &[Vec<String>]) -> bool {
    let a = normalize_model_name(a);
    let b = normalize_model_name(b);
    a == b || aliases_match(&a, &b, aliases) || aliases_match_env(&a, &b)
}

fn normalize_model_name(s: &str) -> String {
    s.rsplit('/')
        .next()
        .unwrap_or(s)
        .trim()
        .to_ascii_lowercase()
}

fn aliases_match(a: &str, b: &str, aliases: &[Vec<String>]) -> bool {
    aliases.iter().any(|group| {
        let mut has_a = false;
        let mut has_b = false;
        for alias in group {
            let normalized = normalize_model_name(alias);
            has_a |= normalized == a;
            has_b |= normalized == b;
        }
        has_a && has_b
    })
}

fn aliases_match_env(a: &str, b: &str) -> bool {
    std::env::var("VERA_EMBEDDING_MODEL_ALIASES")
        .ok()
        .map(|value| aliases_match(a, b, &parse_model_alias_groups(&value)))
        .unwrap_or(false)
}

fn parse_model_alias_groups(value: &str) -> Vec<Vec<String>> {
    value
        .split(';')
        .filter_map(|group| {
            let aliases = group
                .split(',')
                .map(str::trim)
                .filter(|alias| !alias.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            (aliases.len() >= 2).then_some(aliases)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::run_env_test;

    #[test]
    fn embedding_parallelism_clamps_batch_and_concurrency() {
        let config = EmbeddingConfig {
            batch_size: 128,
            max_concurrent_requests: 8,
            max_in_flight_inputs: 16,
            ..EmbeddingConfig::default()
        };

        assert_eq!(config.bounded_parallelism(), (16, 1));
    }

    #[test]
    fn embedding_parallelism_normalizes_zero_values_to_one() {
        let config = EmbeddingConfig {
            batch_size: 0,
            max_concurrent_requests: 0,
            max_in_flight_inputs: 0,
            ..EmbeddingConfig::default()
        };

        assert_eq!(config.bounded_parallelism(), (1, 1));
    }

    #[test]
    fn max_in_flight_environment_value_normalizes_zero_to_one() {
        run_env_test(
            "config::embedding::tests::max_in_flight_environment_value_normalizes_zero_to_one_probe",
            &[("VERA_MAX_IN_FLIGHT_INPUTS", Some("0"))],
        );
    }

    #[test]
    #[ignore = "driven by max_in_flight_environment_value_normalizes_zero_to_one"]
    fn max_in_flight_environment_value_normalizes_zero_to_one_probe() {
        assert_eq!(default_max_in_flight_inputs(), 1);
    }

    /// Shorthand for matching without configured alias groups.
    fn model_names_match(a: &str, b: &str) -> bool {
        model_names_match_with_aliases(a, b, &[])
    }

    #[test]
    fn model_names_match_exact() {
        assert!(model_names_match(
            "jina-embeddings-v5",
            "jina-embeddings-v5"
        ));
    }

    #[test]
    fn model_names_match_with_org_prefix() {
        assert!(model_names_match(
            "jinaai/jina-embeddings-v5-text-nano-retrieval",
            "jina-embeddings-v5-text-nano-retrieval"
        ));
    }

    #[test]
    fn model_names_match_case_insensitive() {
        assert!(model_names_match(
            "Jina-Embeddings-V5",
            "jina-embeddings-v5"
        ));
    }

    #[test]
    fn model_names_match_different_models() {
        assert!(!model_names_match("jina-embeddings-v5", "jina-reranker-v2"));
    }

    #[test]
    fn model_names_match_configured_alias_group() {
        let aliases = vec![vec![
            "text-embedding-3-large".to_string(),
            "text-embedding-3-large-2".to_string(),
        ]];

        assert!(model_names_match_with_aliases(
            "text-embedding-3-large",
            "text-embedding-3-large-2",
            &aliases
        ));
        assert!(!model_names_match_with_aliases(
            "text-embedding-3-large",
            "text-embedding-3-small",
            &aliases
        ));
    }

    #[test]
    fn model_names_match_env_alias_group() {
        run_env_test(
            "config::embedding::tests::model_names_match_env_alias_group_probe",
            &[(
                "VERA_EMBEDDING_MODEL_ALIASES",
                Some("text-embedding-3-large,text-embedding-3-large-2;other,other-prod"),
            )],
        );
    }

    #[test]
    #[ignore = "driven by model_names_match_env_alias_group"]
    fn model_names_match_env_alias_group_probe() {
        assert!(model_names_match(
            "text-embedding-3-large",
            "text-embedding-3-large-2"
        ));
        assert!(model_names_match("other", "other-prod"));
        assert!(!model_names_match(
            "text-embedding-3-large",
            "text-embedding-3-small"
        ));
    }

    #[test]
    fn model_alias_groups_ignore_single_entry_groups() {
        assert!(parse_model_alias_groups("solo;").is_empty());
        assert_eq!(
            parse_model_alias_groups("a,b; c , d").len(),
            2,
            "whitespace-tolerant groups parse"
        );
    }
}
