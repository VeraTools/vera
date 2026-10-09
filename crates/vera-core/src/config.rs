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

/// Environment variables applied by [`VeraConfig::with_env_overrides`].
pub const CONFIG_ENV_OVERRIDE_KEYS: &[&str] = &[
    "VERA_MAX_OUTPUT_CHARS",
    "VERA_MAX_RERANK_BATCH",
    "VERA_MAX_RERANK_DOC_CHARS",
    "VERA_RERANK_TIMEOUT_SECS",
    "VERA_RERANK_MAX_RETRIES",
    "VERA_RERANK_RATE_LIMIT_WAIT_SECS",
    "VERA_RANKING_FILENAME_STEM_BOOST",
    "VERA_RANKING_FILENAME_STEM_MIN_RATIO",
    "VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES",
    "VERA_RANKING_DEFINITION_BOOST",
    "VERA_RANKING_RECALL_POOL_EXPANSION",
    "VERA_VECTOR_FILTER_DURING_SCAN",
    "VERA_MAX_IN_FLIGHT_INPUTS",
    "VERA_MAX_CHUNK_BYTES",
];

impl VeraConfig {
    /// Apply valid environment overrides for runtime use, never for saving.
    pub fn with_env_overrides(mut self) -> Self {
        let retrieval = &mut self.retrieval;
        retrieval.max_output_chars =
            env_parse("VERA_MAX_OUTPUT_CHARS").unwrap_or(retrieval.max_output_chars);
        retrieval.max_rerank_batch =
            env_parse("VERA_MAX_RERANK_BATCH").unwrap_or(retrieval.max_rerank_batch);
        retrieval.reranker_max_doc_chars =
            env_parse("VERA_MAX_RERANK_DOC_CHARS").unwrap_or(retrieval.reranker_max_doc_chars);
        retrieval.reranker_timeout_secs =
            env_parse("VERA_RERANK_TIMEOUT_SECS").unwrap_or(retrieval.reranker_timeout_secs);
        retrieval.reranker_max_retries =
            env_parse("VERA_RERANK_MAX_RETRIES").unwrap_or(retrieval.reranker_max_retries);
        if let Some(value) = env_parse("VERA_RERANK_RATE_LIMIT_WAIT_SECS") {
            retrieval.reranker_rate_limit_wait_secs = Some(value).filter(|value| *value != 0);
        }
        retrieval.ranking_filename_stem_boost = retrieval.ranking_filename_stem_boost_enabled();
        retrieval.ranking_filename_stem_min_ratio =
            retrieval.ranking_filename_stem_min_ratio_effective();
        retrieval.ranking_filename_stem_skip_symbol_queries =
            retrieval.ranking_filename_stem_skip_symbol_queries_enabled();
        retrieval.ranking_definition_boost = retrieval.ranking_definition_boost_enabled();
        retrieval.ranking_recall_pool_expansion = retrieval.ranking_recall_pool_expansion_enabled();
        retrieval.vector_filter_during_scan = retrieval.vector_filter_during_scan_enabled();
        if let Some(value) = env_parse::<usize>("VERA_MAX_IN_FLIGHT_INPUTS") {
            self.embedding.max_in_flight_inputs = value.max(1);
        }
        self.indexing.max_chunk_bytes =
            env_parse("VERA_MAX_CHUNK_BYTES").unwrap_or(self.indexing.max_chunk_bytes);
        self
    }
}

fn env_value(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => {
            tracing::warn!(
                key,
                error = %error,
                "could not read environment override; retaining configured value"
            );
            None
        }
    }
}

/// Read a numeric override. Unset or invalid values leave the config unchanged.
fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T>
where
    T::Err: std::fmt::Display,
{
    let value = env_value(key)?;
    match value.parse() {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            tracing::warn!(
                key,
                value = %value,
                error = %error,
                "invalid numeric environment override; retaining configured value"
            );
            None
        }
    }
}

/// Recognizes `1`/`0`, `true`/`false`, `yes`/`no`, `on`/`off` case-insensitively.
fn env_bool(key: &str) -> Option<bool> {
    let value = env_value(key)?;
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => {
            tracing::warn!(
                key,
                value = %value,
                "invalid boolean environment override; retaining configured value"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::run_env_test;

    // Config path, built-in default, saved value, override, in key-list order.
    const OVERRIDES: &[(&str, &str, &str, &str)] = &[
        ("retrieval.max_output_chars", "0", "111", "777"),
        ("retrieval.max_rerank_batch", "20", "8", "7"),
        ("retrieval.reranker_max_doc_chars", "4800", "1200", "9999"),
        ("retrieval.reranker_timeout_secs", "30", "42", "9"),
        ("retrieval.reranker_max_retries", "2", "5", "3"),
        (
            "retrieval.reranker_rate_limit_wait_secs",
            "null",
            "10",
            "99",
        ),
        (
            "retrieval.ranking_filename_stem_boost",
            "true",
            "false",
            "true",
        ),
        (
            "retrieval.ranking_filename_stem_min_ratio",
            "0.05",
            "0.5",
            "0.75",
        ),
        (
            "retrieval.ranking_filename_stem_skip_symbol_queries",
            "false",
            "true",
            "false",
        ),
        (
            "retrieval.ranking_definition_boost",
            "true",
            "false",
            "true",
        ),
        (
            "retrieval.ranking_recall_pool_expansion",
            "true",
            "false",
            "true",
        ),
        (
            "retrieval.vector_filter_during_scan",
            "true",
            "false",
            "true",
        ),
        ("embedding.max_in_flight_inputs", "256", "64", "32"),
        ("indexing.max_chunk_bytes", "24576", "1000", "2000"),
    ];

    #[test]
    fn environment_override_precedence() {
        assert_eq!(CONFIG_ENV_OVERRIDE_KEYS.len(), OVERRIDES.len());
        for mode in ["valid", "unset", "invalid", "zero"] {
            let mut vars: Vec<_> = CONFIG_ENV_OVERRIDE_KEYS
                .iter()
                .zip(OVERRIDES)
                .map(|(&key, &(_, _, _, value))| {
                    let value = match mode {
                        "valid" => Some(value),
                        "invalid" => Some("invalid"),
                        "zero"
                            if matches!(
                                key,
                                "VERA_RERANK_RATE_LIMIT_WAIT_SECS" | "VERA_MAX_IN_FLIGHT_INPUTS"
                            ) =>
                        {
                            Some("0")
                        }
                        _ => None,
                    };
                    (key, value)
                })
                .collect();
            vars.push(("VERA_TEST_OVERRIDE_MODE", Some(mode)));
            run_env_test(
                "config::tests::environment_override_precedence_probe",
                &vars,
            );
        }
    }

    #[test]
    #[ignore = "driven by environment_override_precedence"]
    fn environment_override_precedence_probe() {
        let mode = std::env::var("VERA_TEST_OVERRIDE_MODE").unwrap();
        let defaults = serde_json::to_value(VeraConfig::default()).unwrap();
        let mut legacy = defaults.clone();
        let mut saved = defaults.clone();
        for &(path, default, value, _) in OVERRIDES {
            let (section, field) = path.split_once('.').unwrap();
            assert_eq!(
                defaults[section][field],
                serde_json::from_str::<serde_json::Value>(default).unwrap(),
                "{path}"
            );
            legacy[section].as_object_mut().unwrap().remove(field);
            saved[section][field] = serde_json::from_str(value).unwrap();
        }
        let legacy: VeraConfig = serde_json::from_value(legacy).unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap(), defaults);
        let config: VeraConfig = serde_json::from_value(saved.clone()).unwrap();
        let runtime = serde_json::to_value(config.with_env_overrides()).unwrap();
        for (&key, &(path, _, value, override_value)) in
            CONFIG_ENV_OVERRIDE_KEYS.iter().zip(OVERRIDES)
        {
            let (section, field) = path.split_once('.').unwrap();
            let expected = match mode.as_str() {
                "valid" => override_value,
                "zero" if key == "VERA_RERANK_RATE_LIMIT_WAIT_SECS" => "null",
                "zero" if key == "VERA_MAX_IN_FLIGHT_INPUTS" => "1",
                _ => value,
            };
            assert_eq!(
                runtime[section][field],
                serde_json::from_str::<serde_json::Value>(expected).unwrap(),
                "{path}: {mode}"
            );
            assert_eq!(
                saved[section][field],
                serde_json::from_str::<serde_json::Value>(value).unwrap()
            );
        }
    }

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
                assert_eq!(env_bool(key), Some(true));
            }
            Ok("0" | "false" | "FALSE" | "no" | "off") => {
                assert_eq!(env_bool(key), Some(false));
            }
            Ok("maybe") | Err(_) => {
                assert_eq!(env_bool(key), None);
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
