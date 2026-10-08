//! Indexing limits and file-discovery configuration.

use serde::{Deserialize, Serialize};

use super::env_usize;

pub(crate) const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 1_000_000;

/// Configuration for the indexing pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingConfig {
    /// Maximum lines for a single chunk before splitting.
    pub max_chunk_lines: u32,
    /// Default path exclusion patterns (in addition to .gitignore).
    pub default_excludes: Vec<String>,
    /// Maximum file size in bytes to index (skip larger files).
    pub max_file_size_bytes: u64,
    /// Extra exclusion globs from CLI `--exclude` flags.
    #[serde(default)]
    pub extra_excludes: Vec<String>,
    /// Disable .gitignore and .veraignore parsing.
    #[serde(default)]
    pub no_ignore: bool,
    /// Disable smart default exclusions.
    #[serde(default)]
    pub no_default_excludes: bool,
    /// Maximum chunk size in bytes for embedding. Chunks exceeding this are
    /// split at line boundaries. 0 disables byte-based splitting.
    /// Default: 24576 (24KB, ~6K-7K tokens). Local embedders see only the
    /// first 512 tokens of a chunk; the size is a retrieval-quality choice
    /// (measured on the Semble suite, see issue #67), not a model limit.
    #[serde(default = "default_max_chunk_bytes")]
    pub max_chunk_bytes: usize,
}

fn default_max_chunk_bytes() -> usize {
    env_usize("VERA_MAX_CHUNK_BYTES", 24_576)
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            max_chunk_lines: 200,
            default_excludes: vec![
                ".git".to_string(),
                ".vera".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
                "build".to_string(),
                "dist".to_string(),
                "__pycache__".to_string(),
                ".venv".to_string(),
            ],
            max_file_size_bytes: DEFAULT_MAX_FILE_SIZE_BYTES, // 1MB
            extra_excludes: Vec::new(),
            no_ignore: false,
            no_default_excludes: false,
            max_chunk_bytes: default_max_chunk_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::run_env_test;

    #[test]
    fn invalid_numeric_environment_value_falls_back_to_default() {
        run_env_test(
            "config::indexing::tests::invalid_numeric_environment_value_falls_back_to_default_probe",
            &[("VERA_MAX_CHUNK_BYTES", Some("24_576"))],
        );
    }

    #[test]
    #[ignore = "driven by invalid_numeric_environment_value_falls_back_to_default"]
    fn invalid_numeric_environment_value_falls_back_to_default_probe() {
        assert_eq!(default_max_chunk_bytes(), 24_576);
    }

    #[test]
    fn default_excludes_contains_common_dirs() {
        let config = IndexingConfig::default();
        assert!(config.default_excludes.contains(&".git".to_string()));
        assert!(
            config
                .default_excludes
                .contains(&"node_modules".to_string())
        );
        assert!(config.default_excludes.contains(&"target".to_string()));
    }
}
