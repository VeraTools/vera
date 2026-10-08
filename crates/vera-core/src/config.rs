//! Configuration types and defaults for Vera's pipeline.

mod backend;
mod embedding;
mod hardware;
mod indexing;
mod retrieval;

pub use backend::{InferenceBackend, OnnxExecutionProvider, is_local_mode, resolve_backend};
pub use embedding::{EmbeddingConfig, SAVED_CONFIG_FORMAT, model_names_match_with_aliases};
pub use hardware::{GpuInfo, detect_gpu_info};
pub(crate) use indexing::DEFAULT_MAX_FILE_SIZE_BYTES;
pub use indexing::IndexingConfig;
pub use retrieval::{RerankerProtocol, RetrievalConfig};

use serde::{Deserialize, Serialize};

/// Top-level configuration for Vera.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VeraConfig {
    /// Indexing configuration.
    pub indexing: IndexingConfig,
    /// Retrieval configuration.
    pub retrieval: RetrievalConfig,
    /// Embedding configuration.
    pub embedding: EmbeddingConfig,
}

/// Read a `usize` config override from an environment variable, falling back
/// to `default` when unset or unparseable. Invalid values are reported so a
/// typo cannot silently change runtime behavior.
fn env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Ok(value) => match value.parse() {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(
                    key,
                    value = %value,
                    default,
                    error = %error,
                    "invalid numeric environment override; using default"
                );
                default
            }
        },
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => {
            tracing::warn!(
                key,
                default,
                error = %error,
                "could not read numeric environment override; using default"
            );
            default
        }
    }
}

/// Read an `f64` config override from an environment variable, falling back
/// to `default` when unset or unparseable.
fn env_f64(key: &str, default: f64) -> f64 {
    match std::env::var(key) {
        Ok(value) => match value.parse::<f64>() {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(
                    key,
                    value = %value,
                    default,
                    error = %error,
                    "invalid float environment override; using default"
                );
                default
            }
        },
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => {
            tracing::warn!(
                key,
                default,
                error = %error,
                "could not read float environment override; using default"
            );
            default
        }
    }
}

/// Read a `bool` config override from an environment variable, falling back
/// to `default` when unset or unparseable. Recognizes `1`/`0`, `true`/`false`,
/// `yes`/`no`, `on`/`off` case-insensitively.
fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => {
                tracing::warn!(
                    key,
                    value = %value,
                    default,
                    "invalid boolean environment override; using default"
                );
                default
            }
        },
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => {
            tracing::warn!(
                key,
                default,
                error = %error,
                "could not read boolean environment override; using default"
            );
            default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::run_env_test;
    #[test]
    fn default_config_is_valid() {
        let config = VeraConfig::default();
        assert!(config.indexing.max_chunk_lines > 0);
        assert!(config.retrieval.default_limit > 0);
        assert!(config.retrieval.rrf_k > 0.0);
        assert!(config.embedding.batch_size > 0);
        assert!(config.embedding.max_in_flight_inputs > 0);
        let (batch_size, concurrency) = config.embedding.bounded_parallelism();
        assert!(
            batch_size * concurrency <= config.embedding.max_in_flight_inputs,
            "default embedding parallelism must respect its in-flight bound"
        );
    }

    #[test]
    fn config_serialization_round_trip() {
        let config = VeraConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: VeraConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.indexing.max_chunk_lines,
            config.indexing.max_chunk_lines
        );
        assert_eq!(
            deserialized.retrieval.default_limit,
            config.retrieval.default_limit
        );
    }

    #[test]
    fn env_bool_parses_variants() {
        for value in [
            "1", "0", "true", "TRUE", "false", "FALSE", "yes", "no", "on", "off", "maybe",
        ] {
            run_env_test(
                "config::tests::env_bool_parses_variants_probe",
                &[("VERA_TEST_BOOL_VARIANTS", Some(value))],
            );
        }
        run_env_test(
            "config::tests::env_bool_parses_variants_probe",
            &[("VERA_TEST_BOOL_VARIANTS", None)],
        );
    }

    #[test]
    #[ignore = "driven by env_bool_parses_variants"]
    fn env_bool_parses_variants_probe() {
        let key = "VERA_TEST_BOOL_VARIANTS";
        match std::env::var(key).as_deref() {
            Ok("1" | "true" | "TRUE" | "yes" | "on") => {
                assert!(env_bool(key, false));
                assert!(env_bool(key, true));
            }
            Ok("0" | "false" | "FALSE" | "no" | "off") => {
                assert!(!env_bool(key, false));
                assert!(!env_bool(key, true));
            }
            Ok("maybe") | Err(_) => {
                assert!(env_bool(key, true));
                assert!(!env_bool(key, false));
            }
            Ok(value) => panic!("unexpected boolean test value: {value}"),
        }
    }

    #[test]
    fn legacy_experiment_settings_load_without_being_serialized() {
        let mut legacy = serde_json::to_value(VeraConfig::default()).unwrap();
        legacy["indexing"]["chunk_max_chars"] = serde_json::json!(750);
        legacy["indexing"]["max_chunk_chars"] = serde_json::json!(750);
        legacy["indexing"]["max_chunk_characters"] = serde_json::json!(750);
        for key in [
            "ranking_multiplicative_path_penalty",
            "ranking_path_penalty",
            "ranking_multiplicative_penalty",
            "ranking_candidate_pool_multiplier",
            "ranking_candidate_pool_size_multiplier",
            "ranking_pool_multiplier",
        ] {
            legacy["retrieval"][key] = serde_json::json!(true);
        }
        let config: VeraConfig = serde_json::from_value(legacy).unwrap();
        let serialized = serde_json::to_value(config).unwrap();
        assert_eq!(
            serialized,
            serde_json::to_value(VeraConfig::default()).unwrap()
        );
    }
}
