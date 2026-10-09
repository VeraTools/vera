//! Persistent CLI state for agent-friendly setup and installs.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_format: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<vera_core::config::InferenceBackend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_api: Option<ApiEndpointConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reranker_api: Option<ApiEndpointConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_config: Option<vera_core::config::VeraConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_embedding_model: Option<vera_core::local_models::LocalEmbeddingModelConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiEndpointConfig {
    pub base_url: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredSecrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reranker_api_key: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstallProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApiSetupInput {
    pub base_url: String,
    pub model_id: String,
    pub api_key: String,
}

/// Load `config.json` verbatim. Every save helper re-reads through here and
/// writes the result back, so anything this function changes is persisted as a
/// side effect of unrelated commands. In particular a repaired `pooling` would
/// be written to disk and then refuse to parse under an older Vera on the same
/// machine, whose `FromStr` only knows `mean` and `cls`. Repairs therefore
/// belong at the points of use, not here. The one exception is moving pre-2.0
/// pinned embedding defaults forward: the result is plain numbers every Vera
/// version parses, and it must persist so later saves do not pin them again.
pub fn load_saved_config() -> Result<StoredConfig> {
    let mut config: StoredConfig = load_json_file(&config_path()?)?;
    if let Some(core) = config.core_config.as_mut() {
        core.embedding.upgrade_saved_defaults(config.config_format);
    }
    Ok(config)
}

pub fn load_saved_secrets() -> Result<StoredSecrets> {
    load_json_file(&credentials_path()?)
}

pub fn load_install_provenance() -> Result<InstallProvenance> {
    load_json_file(&install_path()?)
}

pub fn save_backend(backend: vera_core::config::InferenceBackend) -> Result<()> {
    let mut config = load_saved_config()?;
    config.backend = Some(backend);
    config.local_mode = Some(backend.is_local());
    save_config(&config)
}

pub fn save_local_embedding_model(
    model: &vera_core::local_models::LocalEmbeddingModelConfig,
) -> Result<()> {
    let mut config = load_saved_config()?;
    config.local_embedding_model = Some(model.clone());
    save_config(&config)
}

/// The single point where a stored model config becomes a runtime one.
///
/// Every runtime reader of `local_embedding_model` goes through here, so the
/// repair cannot be forgotten by a future one and silently reinstate the
/// mean-pooled jina config. Nothing here reaches disk; see `load_saved_config`
/// for why the repair must stay out of the load path.
fn repaired_local_embedding_model(
    stored: Option<vera_core::local_models::LocalEmbeddingModelConfig>,
) -> Option<vera_core::local_models::LocalEmbeddingModelConfig> {
    stored.map(vera_core::local_models::LocalEmbeddingModelConfig::repair_stored_defaults)
}

pub fn saved_backend() -> Result<Option<vera_core::config::InferenceBackend>> {
    use vera_core::config::{InferenceBackend, OnnxExecutionProvider};

    let config = load_saved_config()?;
    Ok(config.backend.or(match config.local_mode {
        Some(true) => Some(InferenceBackend::OnnxJina(OnnxExecutionProvider::Cpu)),
        Some(false) => Some(InferenceBackend::Api),
        None => None,
    }))
}

pub fn save_install_method(install_method: Option<&str>) -> Result<()> {
    let mut config = load_saved_config()?;
    config.install_method = install_method.map(|method| method.to_string());
    save_config(&config)
}

pub fn save_runtime_config(config: &vera_core::config::VeraConfig) -> Result<()> {
    let mut stored = load_saved_config()?;
    stored.core_config = Some(config.clone());
    save_config(&stored)
}

pub fn save_api_setup(
    embedding: &ApiSetupInput,
    reranker: Option<&ApiSetupInput>,
    runtime: &vera_core::config::VeraConfig,
) -> Result<()> {
    let mut config = load_saved_config()?;
    let mut secrets = load_saved_secrets()?;
    config.backend = Some(vera_core::config::InferenceBackend::Api);
    config.local_mode = Some(false);
    config.embedding_api = Some(ApiEndpointConfig {
        base_url: embedding.base_url.clone(),
        model_id: embedding.model_id.clone(),
    });
    config.core_config = Some(runtime.clone());
    config.reranker_api = reranker.map(|cfg| ApiEndpointConfig {
        base_url: cfg.base_url.clone(),
        model_id: cfg.model_id.clone(),
    });
    save_config(&config)?;

    secrets.embedding_api_key = Some(embedding.api_key.clone());
    secrets.reranker_api_key = reranker.map(|cfg| cfg.api_key.clone());
    save_secrets(&secrets)
}

/// Drop persisted API reranker settings. Called when selecting a local
/// backend so local mode cannot silently rerank through a stale saved
/// endpoint; shell-set RERANKER_MODEL_* vars still signal explicit intent.
pub fn clear_reranker_setup() -> Result<()> {
    let mut config = load_saved_config()?;
    config.reranker_api = None;
    save_config(&config)?;

    let mut secrets = load_saved_secrets()?;
    secrets.reranker_api_key = None;
    save_secrets(&secrets)
}

pub fn load_runtime_config() -> Result<vera_core::config::VeraConfig> {
    Ok(load_saved_core_config()?.with_env_overrides())
}

/// Load saved values or built-in defaults for editing without env overrides.
pub fn load_saved_core_config() -> Result<vera_core::config::VeraConfig> {
    Ok(load_saved_config()?.core_config.unwrap_or_default())
}

pub fn config_path() -> Result<PathBuf> {
    Ok(vera_dir()?.join("config.json"))
}

pub fn credentials_path() -> Result<PathBuf> {
    Ok(vera_dir()?.join("credentials.json"))
}

pub fn install_path() -> Result<PathBuf> {
    Ok(vera_dir()?.join("install.json"))
}

pub fn vera_dir() -> Result<PathBuf> {
    vera_core::local_models::vera_home_dir()
}

pub fn user_home_dir() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("VERA_USER_HOME")
        && !path.trim().is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    dirs::home_dir().context("Could not find home directory")
}

pub fn apply_saved_env() -> Result<()> {
    apply_saved_env_impl(false)
}

pub fn apply_saved_env_force() -> Result<()> {
    apply_saved_env_impl(true)
}

fn apply_saved_env_impl(force: bool) -> Result<()> {
    let config = load_saved_config()?;
    let secrets = load_saved_secrets()?;

    if let Some(backend) = config.backend {
        set_env_value("VERA_BACKEND", &backend.to_string(), force);
        set_env_value(
            "VERA_LOCAL",
            if backend.is_local() { "1" } else { "0" },
            force,
        );
    } else if let Some(local_mode) = config.local_mode {
        set_env_value("VERA_LOCAL", if local_mode { "1" } else { "0" }, force);
    }

    if let Some(embedding) = config.embedding_api.as_ref() {
        set_env_value("EMBEDDING_MODEL_BASE_URL", &embedding.base_url, force);
        set_env_value("EMBEDDING_MODEL_ID", &embedding.model_id, force);
    }
    if let Some(api_key) = secrets.embedding_api_key.as_deref() {
        set_env_value("EMBEDDING_MODEL_API_KEY", api_key, force);
    }

    apply_reranker_env(
        config.reranker_api.as_ref(),
        secrets.reranker_api_key.as_deref(),
        force,
    );

    // Repaired in memory on the way to the process environment; see
    // `repaired_local_embedding_model`.
    let local_embedding_model = repaired_local_embedding_model(config.local_embedding_model);
    apply_local_embedding_env(local_embedding_model.as_ref(), force);

    Ok(())
}

fn save_config(config: &StoredConfig) -> Result<()> {
    let mut config = config.clone();
    config.config_format = Some(vera_core::config::SAVED_CONFIG_FORMAT);
    write_json_file(&config_path()?, &config)
}

fn save_secrets(secrets: &StoredSecrets) -> Result<()> {
    write_json_file(&credentials_path()?, secrets)
}

fn load_json_file<T>(path: &Path) -> Result<T>
where
    T: Default + for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(T::default());
    }

    let contents = fs::read(path)
        .with_context(|| format!("failed to read persistent state: {}", path.display()))?;
    if contents.is_empty() {
        return Ok(T::default());
    }

    serde_json::from_slice(&contents)
        .with_context(|| format!("failed to parse persistent state: {}", path.display()))
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let contents = serde_json::to_vec_pretty(value)
        .with_context(|| format!("failed to serialize state for {}", path.display()))?;
    write_private_file(path, &contents)
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&tmp_path)
        .with_context(|| format!("failed to open {}", tmp_path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("failed to finalize {}", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", tmp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", tmp_path.display()))?;
    }

    // Rename over the destination instead of removing it first: the rename is
    // atomic, so a crash mid-write leaves either the old file or the new one,
    // never a window where the config file does not exist at all.
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to move {} into place as {}",
            tmp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

const RERANKER_ENV_KEYS: [&str; 3] = [
    "RERANKER_MODEL_BASE_URL",
    "RERANKER_MODEL_ID",
    "RERANKER_MODEL_API_KEY",
];
// Remember only values Vera populated; shell overrides must survive a local switch.
static RERANKER_ENV_FROM_STATE: Mutex<[Option<OsString>; 3]> = Mutex::new([None, None, None]);

fn apply_reranker_env(endpoint: Option<&ApiEndpointConfig>, api_key: Option<&str>, force: bool) {
    let values = [
        endpoint.map(|value| value.base_url.as_str()),
        endpoint.map(|value| value.model_id.as_str()),
        api_key,
    ];
    let mut populated = RERANKER_ENV_FROM_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for ((key, value), remembered) in RERANKER_ENV_KEYS
        .iter()
        .zip(values)
        .zip(populated.iter_mut())
    {
        match value {
            Some(value) if force || std::env::var_os(key).is_none() => {
                set_process_env(key, value);
                *remembered = Some(OsString::from(value));
            }
            None if force => {
                if remembered.is_some() && std::env::var_os(key) == *remembered {
                    clear_process_env(key);
                }
                *remembered = None;
            }
            _ => {}
        }
    }
}

fn set_env_value(key: &str, value: &str, force: bool) {
    if force || std::env::var_os(key).is_none() {
        set_process_env(key, value);
    }
}

fn set_optional_env_value(key: &str, value: Option<&str>, force: bool) {
    match value {
        Some(value) => set_env_value(key, value, force),
        None if force => clear_process_env(key),
        None => {}
    }
}

fn apply_local_embedding_env(
    model: Option<&vera_core::local_models::LocalEmbeddingModelConfig>,
    force: bool,
) {
    let env_override_present = LOCAL_EMBEDDING_SOURCE_ENV_KEYS
        .iter()
        .any(|key| std::env::var_os(key).is_some());
    if !force && env_override_present {
        return;
    }

    let repo = model.and_then(|model| match &model.source {
        vera_core::local_models::LocalEmbeddingSource::HuggingFace { repo } => Some(repo.as_str()),
        vera_core::local_models::LocalEmbeddingSource::Directory { .. } => None,
    });
    let dir = model.and_then(|model| match &model.source {
        vera_core::local_models::LocalEmbeddingSource::Directory { path } => path.to_str(),
        vera_core::local_models::LocalEmbeddingSource::HuggingFace { .. } => None,
    });

    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_REPO_ENV,
        repo,
        force,
    );
    set_optional_env_value(vera_core::local_models::LOCAL_EMBEDDING_DIR_ENV, dir, force);
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_REVISION_ENV,
        model.and_then(|value| value.revision.as_deref()),
        force,
    );
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_ONNX_FILE_ENV,
        model.map(|value| value.onnx_file.as_str()),
        force,
    );
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_ONNX_DATA_FILE_ENV,
        model.and_then(|value| value.onnx_data_file.as_deref()),
        force,
    );
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_TOKENIZER_FILE_ENV,
        model.map(|value| value.tokenizer_file.as_str()),
        force,
    );
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_DIM_ENV,
        model
            .map(|value| value.embedding_dim.to_string())
            .as_deref(),
        force,
    );
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_POOLING_ENV,
        model.map(|value| value.pooling.to_string()).as_deref(),
        force,
    );
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_MAX_LENGTH_ENV,
        model.map(|value| value.max_length.to_string()).as_deref(),
        force,
    );
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_QUERY_PREFIX_ENV,
        model.and_then(|value| value.query_prefix.as_deref()),
        force,
    );
    if force {
        clear_process_env(vera_core::local_models::LEGACY_EMBEDDING_QUERY_PREFIX_ENV);
    }
    set_optional_env_value(
        vera_core::local_models::LOCAL_EMBEDDING_DOCUMENT_PREFIX_ENV,
        model.and_then(|value| value.document_prefix.as_deref()),
        force,
    );
}

fn set_process_env(key: &str, value: &str) {
    // SAFETY: CLI startup applies saved state before background work starts.
    // Setup/repair drop their download runtimes before applying a backend change,
    // and apply it before initializing ONNX or starting indexing threads.
    //
    // The unit tests below break that condition: libtest runs them on several
    // threads at once. They are sound instead because every test that reads or
    // writes any of `RESTORED_ENV_KEYS` holds `VERA_HOME_LOCK` for its whole
    // body, and nothing else in the test binary touches those variables. Any
    // new test that calls this, `clear_process_env`, or a helper reaching them
    // must take the same lock.
    // SAFETY: Startup applies configuration before native providers run. CLI
    // fixtures serialize writes and do not run native inference during mutation.
    unsafe {
        std::env::set_var(key, value);
    }
}

fn clear_process_env(key: &str) {
    // SAFETY: This shares set_process_env's startup and setup boundaries: it
    // clears configuration before native provider use, and test writes are
    // serialized as described there.
    unsafe {
        std::env::remove_var(key);
    }
}

const LOCAL_EMBEDDING_SOURCE_ENV_KEYS: &[&str] = &[
    vera_core::local_models::LOCAL_EMBEDDING_REPO_ENV,
    vera_core::local_models::LOCAL_EMBEDDING_DIR_ENV,
    vera_core::local_models::LOCAL_EMBEDDING_REVISION_ENV,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `VERA_HOME` is process-global, so the tests that redirect it at the
    /// config directory must not overlap with each other.
    static VERA_HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn config_edits_do_not_save_environment_overrides() {
        for value in [Some("valid"), None, Some("invalid")] {
            let dir = tempfile::tempdir().unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "state::tests::config_edits_do_not_save_environment_overrides_probe",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                ])
                .env("VERA_HOME", dir.path())
                .env_remove("VERA_LOCAL")
                .env_remove("VERA_BACKEND");
            for (key, valid) in [
                ("VERA_RANKING_DEFINITION_BOOST", "0"),
                ("VERA_MAX_OUTPUT_CHARS", "777"),
                ("VERA_RERANK_TIMEOUT_SECS", "9"),
                ("VERA_LOCAL", "1"),
            ] {
                match value {
                    Some("valid") => {
                        command.env(key, valid);
                    }
                    Some(value) => {
                        command.env(key, value);
                    }
                    None => {
                        command.env_remove(key);
                    }
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
    #[ignore = "driven by config_edits_do_not_save_environment_overrides"]
    fn config_edits_do_not_save_environment_overrides_probe() {
        let mut expected = vera_core::config::VeraConfig::default();
        expected.retrieval.default_limit = 7;
        crate::commands::config::run(
            &["set".into(), "retrieval.default_limit".into(), "7".into()],
            false,
        )
        .unwrap();
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(config_path().unwrap()).unwrap()).unwrap();
        assert_eq!(raw["core_config"], serde_json::to_value(&expected).unwrap());
        assert_eq!(raw["core_config"]["embedding"]["batch_size"], 128);
        assert_eq!(
            raw["core_config"]["embedding"]["max_concurrent_requests"],
            8
        );

        // A later explicit edit still saves the requested value, not the env.
        crate::commands::config::run(
            &[
                "set".into(),
                "retrieval.max_output_chars".into(),
                "321".into(),
            ],
            false,
        )
        .unwrap();
        expected.retrieval.max_output_chars = 321;
        let saved = load_saved_core_config().unwrap();
        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let runtime = load_runtime_config().unwrap();
        if std::env::var("VERA_MAX_OUTPUT_CHARS").as_deref() == Ok("777") {
            assert_eq!(runtime.retrieval.max_output_chars, 777);
            assert_eq!(runtime.retrieval.reranker_timeout_secs, 9);
            assert!(!runtime.retrieval.ranking_definition_boost);
        } else {
            assert_eq!(runtime.retrieval.max_output_chars, 321);
            assert_eq!(runtime.retrieval.reranker_timeout_secs, 30);
            assert!(runtime.retrieval.ranking_definition_boost);
        }

        let embedding = ApiSetupInput {
            base_url: "https://embedding.example".into(),
            model_id: "embedding".into(),
            api_key: "fixture-key".into(),
        };
        save_api_setup(&embedding, None, &saved).unwrap();
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(config_path().unwrap()).unwrap()).unwrap();
        assert_eq!(raw["core_config"], serde_json::to_value(expected).unwrap());
    }

    /// What `vera setup` wrote to `config.json` before the pooling fix.
    const LEGACY_JINA_CONFIG: &str = r#"{
      "local_embedding_model": {
        "source": {"source": "hugging-face",
                   "repo": "jinaai/jina-embeddings-v5-text-nano-retrieval"},
        "onnx_file": "onnx/model_quantized.onnx",
        "onnx_data_file": "onnx/model_quantized.onnx_data",
        "tokenizer_file": "tokenizer.json",
        "embedding_dim": 768,
        "pooling": "mean",
        "max_length": 512
      }
    }"#;

    /// What `vera setup --embedding-document-prefix 'Passage:'` writes. The
    /// prefix differs from jina's preset so the preset cannot stand in for it.
    const STORED_DOCUMENT_PREFIX_CONFIG: &str = r#"{
      "local_embedding_model": {
        "source": {"source": "hugging-face",
                   "repo": "jinaai/jina-embeddings-v5-text-nano-retrieval"},
        "onnx_file": "onnx/model_quantized.onnx",
        "onnx_data_file": "onnx/model_quantized.onnx_data",
        "tokenizer_file": "tokenizer.json",
        "embedding_dim": 768,
        "pooling": "last-token",
        "max_length": 512,
        "query_prefix": "Query:",
        "document_prefix": "Passage:"
      }
    }"#;

    /// A stored model that prefixes queries only.
    ///
    /// The repo is jina's so that `defaults_for_source` answers it with a preset
    /// that *does* carry a document prefix. That is not because the resolution
    /// consults it (the test pins the arm actually taken), but because a preset
    /// prefix is what gives the `None` assertion something to fail against:
    /// delete the explicit-model short-circuit and jina's
    /// `Document:` surfaces. An unrecognised repo would fall to
    /// `generic_defaults`, whose document prefix is `None` on both sides of that
    /// change, so nothing could distinguish them.
    const STORED_QUERY_PREFIX_ONLY_CONFIG: &str = r#"{
      "local_embedding_model": {
        "source": {"source": "hugging-face",
                   "repo": "jinaai/jina-embeddings-v5-text-nano-retrieval"},
        "onnx_file": "onnx/model_quantized.onnx",
        "tokenizer_file": "tokenizer.json",
        "embedding_dim": 384,
        "pooling": "cls",
        "max_length": 256,
        "query_prefix": "Ask:"
      }
    }"#;

    /// A stored model pinned to an immutable upstream revision.
    const STORED_REVISION_CONFIG: &str = r#"{
      "local_embedding_model": {
        "source": {"source": "hugging-face",
                   "repo": "org/model"},
        "revision": "d59c919d0159aea2c19ed7d04288fcdd048d0f9c",
        "onnx_file": "onnx/model.onnx",
        "tokenizer_file": "tokenizer.json",
        "embedding_dim": 768,
        "pooling": "mean",
        "max_length": 512
      }
    }"#;

    /// Everything `apply_saved_env_impl` can write, plus the redirect itself.
    const RESTORED_ENV_KEYS: &[&str] = &[
        "VERA_HOME",
        "VERA_BACKEND",
        "VERA_LOCAL",
        "EMBEDDING_MODEL_BASE_URL",
        "EMBEDDING_MODEL_ID",
        "EMBEDDING_MODEL_API_KEY",
        "RERANKER_MODEL_BASE_URL",
        "RERANKER_MODEL_ID",
        "RERANKER_MODEL_API_KEY",
        vera_core::local_models::LOCAL_EMBEDDING_REPO_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_DIR_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_REVISION_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_ONNX_FILE_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_ONNX_DATA_FILE_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_TOKENIZER_FILE_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_DIM_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_POOLING_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_MAX_LENGTH_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_QUERY_PREFIX_ENV,
        vera_core::local_models::LOCAL_EMBEDDING_DOCUMENT_PREFIX_ENV,
        vera_core::local_models::LEGACY_EMBEDDING_QUERY_PREFIX_ENV,
    ];

    struct VeraHomeGuard {
        _dir: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
        previous: Vec<Option<std::ffi::OsString>>,
        previous_reranker_env: [Option<OsString>; 3],
    }

    impl Drop for VeraHomeGuard {
        fn drop(&mut self) {
            for (key, value) in RESTORED_ENV_KEYS.iter().zip(&self.previous) {
                match value {
                    Some(value) => set_process_env(key, &value.to_string_lossy()),
                    None => clear_process_env(key),
                }
            }
            *RERANKER_ENV_FROM_STATE.lock().unwrap() = self.previous_reranker_env.clone();
        }
    }

    /// Point `VERA_HOME` at a temp dir seeded with `contents` so no test can
    /// reach the developer's real `~/.vera/config.json`.
    fn with_stored_config(contents: &str) -> VeraHomeGuard {
        let lock = VERA_HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = RESTORED_ENV_KEYS.iter().map(std::env::var_os).collect();
        let previous_reranker_env = std::mem::take(&mut *RERANKER_ENV_FROM_STATE.lock().unwrap());
        let dir = tempfile::tempdir().unwrap();
        set_process_env("VERA_HOME", dir.path().to_str().unwrap());
        fs::write(config_path().unwrap(), contents).unwrap();
        VeraHomeGuard {
            _dir: dir,
            _lock: lock,
            previous,
            previous_reranker_env,
        }
    }

    fn stored_pooling_on_disk() -> String {
        let raw = fs::read(config_path().unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["local_embedding_model"]["pooling"]
            .as_str()
            .expect("stored config should still carry a pooling field")
            .to_string()
    }

    #[test]
    fn pre_v2_pinned_embedding_defaults_upgrade_once_and_explicit_values_stay() {
        let mut dump = vera_core::config::VeraConfig::default();
        dump.embedding.max_concurrent_requests = 2;
        dump.embedding.max_in_flight_inputs = 16;
        dump.embedding.timeout_secs = 60;
        let _guard = with_stored_config(&serde_json::json!({ "core_config": dump }).to_string());
        let current = vera_core::config::VeraConfig::default()
            .with_env_overrides()
            .embedding;
        let upgraded = load_runtime_config().unwrap().embedding;
        assert_eq!(upgraded.max_in_flight_inputs, current.max_in_flight_inputs);
        assert_eq!(upgraded.timeout_secs, current.timeout_secs);
        assert_eq!(upgraded.max_concurrent_requests, 2);

        let mut explicit = load_saved_core_config().unwrap();
        explicit.embedding.max_in_flight_inputs = 16;
        explicit.embedding.timeout_secs = 60;
        save_runtime_config(&explicit).unwrap();
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(config_path().unwrap()).unwrap()).unwrap();
        assert_eq!(raw["config_format"], vera_core::config::SAVED_CONFIG_FORMAT);
        assert_eq!(raw["core_config"]["embedding"]["max_in_flight_inputs"], 16);
        let expected = explicit.with_env_overrides().embedding;
        let reloaded = load_runtime_config().unwrap().embedding;
        assert_eq!(
            (reloaded.max_in_flight_inputs, reloaded.timeout_secs),
            (expected.max_in_flight_inputs, expected.timeout_secs)
        );
    }

    #[test]
    fn disabling_saved_reranker_clears_loaded_values_but_preserves_shell_overrides() {
        let _guard = with_stored_config("{}");
        for shell_override in [false, true] {
            for key in RERANKER_ENV_KEYS {
                clear_process_env(key);
            }
            let embedding = ApiSetupInput {
                base_url: "https://embedding.example".into(),
                model_id: "embedding".into(),
                api_key: "embedding-key".into(),
            };
            let reranker = ApiSetupInput {
                base_url: "https://reranker.example".into(),
                model_id: "reranker".into(),
                api_key: "reranker-key".into(),
            };
            save_api_setup(
                &embedding,
                Some(&reranker),
                &vera_core::config::VeraConfig::default(),
            )
            .unwrap();
            if shell_override {
                for key in RERANKER_ENV_KEYS {
                    set_process_env(key, "shell-override");
                }
            }
            apply_saved_env().unwrap();
            assert!(
                RERANKER_ENV_KEYS
                    .iter()
                    .all(|key| std::env::var_os(key).is_some())
            );
            clear_reranker_setup().unwrap();
            apply_saved_env_force().unwrap();
            for key in RERANKER_ENV_KEYS {
                assert_eq!(
                    std::env::var_os(key),
                    shell_override.then(|| OsString::from("shell-override")),
                    "{key}"
                );
            }
        }
    }

    #[test]
    fn unrelated_save_does_not_rewrite_stored_pooling() {
        let _guard = with_stored_config(LEGACY_JINA_CONFIG);
        assert_eq!(stored_pooling_on_disk(), "mean");

        // Every save helper is a load-mutate-save cycle over the same struct,
        // so a repair applied at load time would be persisted from here.
        save_backend(vera_core::config::InferenceBackend::OnnxJina(
            vera_core::config::OnnxExecutionProvider::Cpu,
        ))
        .unwrap();

        // A Vera older than `last-token` still has to be able to parse this
        // file; its `FromStr` accepts only `mean` and `cls`.
        assert_eq!(stored_pooling_on_disk(), "mean");
    }

    #[test]
    fn repair_command_does_not_rewrite_stored_pooling() {
        let _guard = with_stored_config(LEGACY_JINA_CONFIG);
        assert_eq!(stored_pooling_on_disk(), "mean");

        // `vera repair` resolves an embedding model for asset preparation but
        // does not persist it. Sourcing it from the repaired accessor would
        // put `last-token` in the file and brick an older Vera installed
        // alongside.
        let model = crate::commands::repair::embedding_model_for_repair(
            vera_core::config::InferenceBackend::OnnxJina(
                vera_core::config::OnnxExecutionProvider::Cpu,
            ),
        )
        .unwrap()
        .expect("an ONNX backend always carries an embedding model");
        save_local_embedding_model(&model).unwrap();

        assert_eq!(stored_pooling_on_disk(), "mean");
    }

    #[test]
    fn repair_api_does_not_switch_backend_or_clear_reranker() {
        let _guard = with_stored_config(
            r#"{
              "backend": "potion-code",
              "local_mode": true,
              "reranker_api": {"base_url": "https://reranker.example", "model_id": "rerank"}
            }"#,
        );
        fs::write(
            credentials_path().unwrap(),
            r#"{"reranker_api_key":"reranker-secret"}"#,
        )
        .unwrap();
        set_process_env("EMBEDDING_MODEL_BASE_URL", "https://embedding.example");
        set_process_env("EMBEDDING_MODEL_ID", "embedding");
        set_process_env("EMBEDDING_MODEL_API_KEY", "embedding-secret");

        crate::commands::repair::run(None, true, true).unwrap();

        assert_eq!(
            saved_backend().unwrap(),
            Some(vera_core::config::InferenceBackend::PotionCode)
        );
        let config = load_saved_config().unwrap();
        assert_eq!(config.reranker_api.unwrap().model_id, "rerank");
        assert_eq!(
            load_saved_secrets().unwrap().reranker_api_key.as_deref(),
            Some("reranker-secret")
        );
    }

    #[test]
    fn runtime_readers_repair_stored_pooling_in_memory() {
        let _guard = with_stored_config(LEGACY_JINA_CONFIG);

        let model =
            repaired_local_embedding_model(load_saved_config().unwrap().local_embedding_model)
                .expect("stored config carries a local embedding model");
        assert_eq!(
            model.pooling,
            vera_core::local_models::LocalEmbeddingPooling::LastToken
        );

        apply_saved_env_force().unwrap();
        assert_eq!(
            std::env::var(vera_core::local_models::LOCAL_EMBEDDING_POOLING_ENV).unwrap(),
            "last-token"
        );

        assert_eq!(stored_pooling_on_disk(), "mean");
    }

    #[test]
    fn stored_document_prefix_reaches_the_env_config() {
        let _guard = with_stored_config(STORED_DOCUMENT_PREFIX_CONFIG);

        apply_saved_env_force().unwrap();

        // `from_env` is the only reader the embedding pipeline has, so a stored
        // prefix that never reaches the environment is a dropped flag.
        let model = vera_core::local_models::LocalEmbeddingModelConfig::from_env().unwrap();
        assert_eq!(model.document_prefix.as_deref(), Some("Passage:"));
    }

    #[test]
    fn stored_revision_reaches_the_env_config() {
        let _guard = with_stored_config(STORED_REVISION_CONFIG);

        apply_saved_env_force().unwrap();

        // `from_env` is the only reader the embedding pipeline has, so a stored
        // revision that never reaches the environment downloads `main` while
        // the config claims a pin.
        let model = vera_core::local_models::LocalEmbeddingModelConfig::from_env().unwrap();
        assert_eq!(
            model.revision.as_deref(),
            Some("d59c919d0159aea2c19ed7d04288fcdd048d0f9c")
        );
    }

    #[test]
    fn a_stored_legacy_jina_config_is_pinned_over_a_stale_revision() {
        let _guard = with_stored_config(LEGACY_JINA_CONFIG);
        set_process_env(
            vera_core::local_models::LOCAL_EMBEDDING_REVISION_ENV,
            "stale-revision",
        );

        apply_saved_env_force().unwrap();

        // The legacy jina config is repaired in memory to the current jina
        // preset, which carries a pinned revision. Forcing it over the
        // environment has to overwrite a stale inherited revision with the
        // pin, or the pin would silently swap the model bytes under an
        // unchanged config.
        assert_eq!(
            std::env::var(vera_core::local_models::LOCAL_EMBEDDING_REVISION_ENV).unwrap(),
            "ac5d898c8d382b17167c33e5c8af644a3519b47d"
        );
        let model = vera_core::local_models::LocalEmbeddingModelConfig::from_env().unwrap();
        assert_eq!(
            model.revision.as_deref(),
            Some("ac5d898c8d382b17167c33e5c8af644a3519b47d")
        );
    }

    #[test]
    fn a_stored_config_without_a_revision_clears_a_stale_one() {
        // An unknown repo: repair_stored_defaults does not fire, and known
        // repos always resolve to their pin, so the unpinned case only exists
        // for custom models.
        let _guard = with_stored_config(&LEGACY_JINA_CONFIG.replace(
            "jinaai/jina-embeddings-v5-text-nano-retrieval",
            "org/custom-model",
        ));
        set_process_env(
            vera_core::local_models::LOCAL_EMBEDDING_REVISION_ENV,
            "stale-revision",
        );

        apply_saved_env_force().unwrap();

        // Forcing the stored config over the environment has to remove a
        // revision the stored config does not have, or an inherited pin would
        // silently swap the model bytes under an unchanged config.
        assert!(std::env::var_os(vera_core::local_models::LOCAL_EMBEDDING_REVISION_ENV).is_none());
        let model = vera_core::local_models::LocalEmbeddingModelConfig::from_env().unwrap();
        assert_eq!(model.revision, None);
    }

    #[test]
    fn a_stored_config_without_a_document_prefix_clears_a_stale_one() {
        let _guard = with_stored_config(STORED_QUERY_PREFIX_ONLY_CONFIG);
        set_process_env(
            vera_core::local_models::LOCAL_EMBEDDING_DOCUMENT_PREFIX_ENV,
            "Stale-Passage-Marker:",
        );

        apply_saved_env_force().unwrap();

        // Forcing the stored config over the environment has to remove a
        // document prefix the stored config does not have, or an inherited one
        // would keep prefixing passages the stored model never asked for.
        assert!(
            std::env::var_os(vera_core::local_models::LOCAL_EMBEDDING_DOCUMENT_PREFIX_ENV)
                .is_none()
        );
        // Clearing one side must not clear the other.
        assert_eq!(
            std::env::var(vera_core::local_models::LOCAL_EMBEDDING_QUERY_PREFIX_ENV).unwrap(),
            "Ask:"
        );

        // The force path exports a source and an onnx file together, which is
        // exactly the pair `model_source_and_onnx_file_are_set` tests, so
        // `explicit_model_env` is always on downstream of it. That makes
        // `resolve_optional_env_value` take its `None if explicit_model_env`
        // arm; the `None => default` arm is unreachable from this entry point,
        // so no fixture stored through `config.json` can exercise it.
        assert!(
            std::env::var_os(vera_core::local_models::LOCAL_EMBEDDING_REPO_ENV).is_some()
                && std::env::var_os(vera_core::local_models::LOCAL_EMBEDDING_ONNX_FILE_ENV)
                    .is_some(),
            "the force path is supposed to make the model explicit through the environment"
        );

        let model = vera_core::local_models::LocalEmbeddingModelConfig::from_env().unwrap();
        assert_eq!(model.query_prefix.as_deref(), Some("Ask:"));
        // What is left to pin is that arm returning nothing rather than the
        // preset's prefix. There is a preset prefix to return, so `None` below
        // is a declined default and not an absent one.
        assert!(
            vera_core::local_models::LocalEmbeddingModelConfig::jina()
                .document_prefix
                .is_some(),
            "the fixture's repo must have a preset document prefix, or `None` below proves nothing"
        );
        assert_eq!(model.document_prefix, None);
    }

    /// The opt-out has to survive the file and the environment, not just the
    /// struct. It used to be filtered out on the way to `config.json`, whose
    /// missing key then let jina's preset reinstate the prefix on the next run,
    /// so disabling a prefix lasted exactly one invocation.
    #[test]
    fn an_explicitly_emptied_prefix_survives_a_save_and_reload() {
        let _guard = with_stored_config("{}");

        let mut model = vera_core::local_models::LocalEmbeddingModelConfig::jina();
        model.query_prefix = Some(String::new());
        model.document_prefix = Some(String::new());
        save_local_embedding_model(&model).unwrap();

        // The empty value has to reach the file; a skipped key is what the
        // preset fills back in.
        let raw = fs::read(config_path().unwrap()).unwrap();
        let stored: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(stored["local_embedding_model"]["query_prefix"], "");
        assert_eq!(stored["local_embedding_model"]["document_prefix"], "");

        apply_saved_env_force().unwrap();

        let reloaded = vera_core::local_models::LocalEmbeddingModelConfig::from_env().unwrap();
        assert_eq!(
            reloaded.query_prefix, None,
            "jina's preset query prefix came back after an explicit opt-out"
        );
        assert_eq!(
            reloaded.document_prefix, None,
            "jina's preset document prefix came back after an explicit opt-out"
        );
        assert_eq!(reloaded.query_text("find main"), "find main");
        assert_eq!(reloaded.document_text("fn main() {}"), "fn main() {}");
    }

    #[test]
    fn stored_config_defaults_are_empty() {
        let config = StoredConfig::default();
        assert!(config.local_mode.is_none());
        assert!(config.backend.is_none());
        assert!(config.install_method.is_none());
        assert!(config.embedding_api.is_none());
        assert!(config.reranker_api.is_none());
        assert!(config.core_config.is_none());
        assert!(config.local_embedding_model.is_none());
    }

    #[test]
    fn stored_secrets_default_empty() {
        let secrets = StoredSecrets::default();
        assert!(secrets.embedding_api_key.is_none());
        assert!(secrets.reranker_api_key.is_none());
    }

    #[test]
    fn install_provenance_defaults_are_empty() {
        let provenance = InstallProvenance::default();
        assert!(provenance.install_method.is_none());
        assert!(provenance.version.is_none());
        assert!(provenance.binary_path.is_none());
    }

    /// Overwriting an existing file must go through a plain rename: the old
    /// content stays readable until the new content replaces it atomically,
    /// and no temp file survives either way.
    #[test]
    fn write_private_file_replaces_an_existing_file_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        write_private_file(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first\n");

        write_private_file(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second\n");

        let residue: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(residue, vec!["config.json".to_string()]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the rewritten file must stay private");
        }
    }
}
