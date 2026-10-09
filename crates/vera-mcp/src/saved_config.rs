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
/// Returns defaults plus runtime environment overrides on any read or parse
/// failure so MCP stays usable.
pub fn load_saved_runtime_config() -> vera_core::config::VeraConfig {
    let config_path = match vera_core::local_models::vera_home_dir() {
        Ok(dir) => dir.join("config.json"),
        Err(_) => return vera_core::config::VeraConfig::default().with_env_overrides(),
    };
    load_runtime_config_from_path(&config_path)
}

fn load_runtime_config_from_path(config_path: &Path) -> vera_core::config::VeraConfig {
    let data = match std::fs::read(config_path) {
        Ok(data) => data,
        Err(_) => return vera_core::config::VeraConfig::default().with_env_overrides(),
    };
    if data.is_empty() {
        return vera_core::config::VeraConfig::default().with_env_overrides();
    }
    let stored: StoredConfig = match serde_json::from_slice(&data) {
        Ok(stored) => stored,
        Err(_) => return vera_core::config::VeraConfig::default().with_env_overrides(),
    };
    let mut config = stored.core_config.unwrap_or_default();
    config
        .embedding
        .upgrade_saved_defaults(stored.config_format);
    config.with_env_overrides()
}

#[cfg(test)]
mod tests {
    use super::load_runtime_config_from_path;

    #[test]
    fn runtime_environment_precedence_and_fallbacks() {
        for value in [Some("777"), None, Some("invalid")] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args([
                "saved_config::tests::runtime_environment_precedence_and_fallbacks_probe",
                "--exact",
                "--ignored",
                "--nocapture",
            ]);
            match value {
                Some(value) => {
                    command.env("VERA_MAX_OUTPUT_CHARS", value);
                }
                None => {
                    command.env_remove("VERA_MAX_OUTPUT_CHARS");
                }
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "child failed: {}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    #[ignore = "driven by runtime_environment_precedence_and_fallbacks"]
    fn runtime_environment_precedence_and_fallbacks_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        let valid = std::env::var("VERA_MAX_OUTPUT_CHARS").as_deref() == Ok("777");
        assert_eq!(
            load_runtime_config_from_path(&path)
                .retrieval
                .max_output_chars,
            if valid { 777 } else { 0 }
        );
        for contents in ["", "not json", "{}"] {
            std::fs::write(&path, contents).unwrap();
            assert_eq!(
                load_runtime_config_from_path(&path)
                    .retrieval
                    .max_output_chars,
                if valid { 777 } else { 0 }
            );
        }
        let mut saved = vera_core::config::VeraConfig::default();
        saved.retrieval.max_output_chars = 321;
        let contents = serde_json::json!({"core_config": saved}).to_string();
        std::fs::write(&path, &contents).unwrap();
        assert_eq!(
            load_runtime_config_from_path(&path)
                .retrieval
                .max_output_chars,
            if valid { 777 } else { 321 }
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
    }

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

        let expected = cfg.with_env_overrides();
        let config = load_runtime_config_from_path(&tmp.path().join("config.json"));
        assert_eq!(config.indexing.max_chunk_lines, 99);
        assert_eq!(
            config.indexing.max_chunk_bytes,
            expected.indexing.max_chunk_bytes
        );
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
        let current = vera_core::config::VeraConfig::default()
            .with_env_overrides()
            .embedding;
        let upgraded = load_runtime_config_from_path(&path).embedding;
        assert_eq!(upgraded.max_in_flight_inputs, current.max_in_flight_inputs);
        assert_eq!(upgraded.timeout_secs, current.timeout_secs);

        let format = vera_core::config::SAVED_CONFIG_FORMAT;
        std::fs::write(
            &path,
            serde_json::json!({ "config_format": format, "core_config": cfg }).to_string(),
        )
        .unwrap();
        let expected = cfg.with_env_overrides().embedding;
        let kept = load_runtime_config_from_path(&path).embedding;
        assert_eq!(
            (kept.max_in_flight_inputs, kept.timeout_secs),
            (expected.max_in_flight_inputs, expected.timeout_secs)
        );
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
