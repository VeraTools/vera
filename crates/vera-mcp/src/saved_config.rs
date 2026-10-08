//! Load saved CLI runtime config for MCP-mode behavior.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct StoredConfig {
    #[serde(default)]
    config_format: Option<u32>,
    #[serde(default)]
    core_config: Option<vera_core::config::VeraConfig>,
}

/// Load the saved runtime config from Vera's home config.json.
///
/// Returns default config on any read or parse failure so MCP stays usable.
pub fn load_saved_runtime_config() -> vera_core::config::VeraConfig {
    let config_path = match vera_core::local_models::vera_home_dir() {
        Ok(dir) => dir.join("config.json"),
        Err(_) => return vera_core::config::VeraConfig::default(),
    };
    load_runtime_config_from_path(&config_path)
}

fn load_runtime_config_from_path(config_path: &Path) -> vera_core::config::VeraConfig {
    let data = match std::fs::read(config_path) {
        Ok(data) => data,
        Err(_) => return vera_core::config::VeraConfig::default(),
    };
    if data.is_empty() {
        return vera_core::config::VeraConfig::default();
    }
    let stored: StoredConfig = match serde_json::from_slice(&data) {
        Ok(stored) => stored,
        Err(_) => return vera_core::config::VeraConfig::default(),
    };
    let mut config = stored.core_config.unwrap_or_default();
    config
        .embedding
        .upgrade_saved_defaults(stored.config_format);
    config
}

#[cfg(test)]
mod tests {
    use super::load_runtime_config_from_path;

    #[test]
    fn load_config_missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let config = load_runtime_config_from_path(&tmp.path().join("config.json"));
        assert_eq!(
            config.indexing.max_chunk_lines,
            vera_core::config::VeraConfig::default()
                .indexing
                .max_chunk_lines
        );
    }

    #[test]
    fn load_config_reads_core_config() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = vera_core::config::VeraConfig::default();
        cfg.indexing.max_chunk_lines = 99;
        cfg.indexing.max_chunk_bytes = 1800;
        cfg.retrieval.default_limit = 17;
        let json = serde_json::json!({ "core_config": cfg });
        std::fs::write(tmp.path().join("config.json"), json.to_string()).unwrap();

        let config = load_runtime_config_from_path(&tmp.path().join("config.json"));
        assert_eq!(config.indexing.max_chunk_lines, 99);
        assert_eq!(config.indexing.max_chunk_bytes, 1800);
        assert_eq!(config.retrieval.default_limit, 17);
    }

    #[test]
    fn load_config_upgrades_pre_v2_pinned_embedding_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = vera_core::config::VeraConfig::default();
        cfg.embedding.max_in_flight_inputs = 16;
        cfg.embedding.timeout_secs = 60;
        let path = tmp.path().join("config.json");
        std::fs::write(&path, serde_json::json!({ "core_config": cfg }).to_string()).unwrap();
        let current = vera_core::config::EmbeddingConfig::default();
        let upgraded = load_runtime_config_from_path(&path).embedding;
        assert_eq!(upgraded.max_in_flight_inputs, current.max_in_flight_inputs);
        assert_eq!(upgraded.timeout_secs, current.timeout_secs);

        let format = vera_core::config::SAVED_CONFIG_FORMAT;
        std::fs::write(
            &path,
            serde_json::json!({ "config_format": format, "core_config": cfg }).to_string(),
        )
        .unwrap();
        let kept = load_runtime_config_from_path(&path).embedding;
        assert_eq!((kept.max_in_flight_inputs, kept.timeout_secs), (16, 60));
    }

    #[test]
    fn load_config_no_core_config_key_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.json"), r#"{"backend":"api"}"#).unwrap();

        let config = load_runtime_config_from_path(&tmp.path().join("config.json"));
        assert_eq!(
            config.indexing.max_chunk_lines,
            vera_core::config::VeraConfig::default()
                .indexing
                .max_chunk_lines
        );
    }
}
