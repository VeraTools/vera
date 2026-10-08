//! Process-lifetime search runtime, context cache, and repository watchers.

use std::collections::{HashMap, hash_map::Entry};
use std::sync::{Arc, Mutex, OnceLock};

use crate::protocol::ToolCallResult;
use crate::watcher::WatchHandle;

/// Watcher handles keyed by canonical repository path. Each handle is kept
/// alive for the lifetime of the MCP server process.
static WATCHERS: OnceLock<Mutex<HashMap<std::path::PathBuf, WatchHandle>>> = OnceLock::new();

static SEARCH_CONTEXT: Mutex<Option<CachedSearchContext>> = Mutex::new(None);

static SEARCH_RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();

fn watchers() -> &'static Mutex<HashMap<std::path::PathBuf, WatchHandle>> {
    WATCHERS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct CachedSearchContext {
    key: String,
    context: Arc<vera_core::retrieval::search_service::SearchContext>,
}

pub(super) fn ensure_index_and_watcher(
    cwd: &std::path::Path,
) -> Result<std::path::PathBuf, ToolCallResult> {
    let index_dir = vera_core::indexing::index_dir(cwd);

    if !index_dir.exists() {
        let (rt, provider, idx_config, model_name) = create_runtime_and_provider()?;
        rt.block_on(vera_core::indexing::index_repository(
            cwd,
            &provider,
            &idx_config,
            &model_name,
        ))
        .map_err(|e| ToolCallResult::error(format!("Auto-indexing failed: {e}")))?;
    }

    let watcher_path = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut guard = watchers().lock().unwrap_or_else(|e| e.into_inner());
    if let Entry::Vacant(entry) = guard.entry(watcher_path) {
        match crate::watcher::start_watching(cwd) {
            Ok(handle) => {
                entry.insert(handle);
            }
            Err(e) => tracing::warn!("failed to start file watcher: {e}"),
        }
    }

    Ok(index_dir)
}

/// Create a tokio runtime, resolve backend config, and build an embedding provider.
fn create_runtime_and_provider() -> Result<
    (
        tokio::runtime::Runtime,
        vera_core::embedding::DynamicProvider,
        vera_core::config::VeraConfig,
        String,
    ),
    ToolCallResult,
> {
    let backend = vera_core::config::resolve_backend(None);
    let mut config = crate::saved_config::load_saved_runtime_config();
    config.adjust_for_backend(backend);

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| ToolCallResult::error(format!("Failed to create runtime: {e}")))?;

    let (provider, model_name) = rt
        .block_on(vera_core::embedding::create_dynamic_provider(
            &config, backend,
        ))
        .map_err(|e| ToolCallResult::error(format!("Failed to create embedding provider: {e}")))?;

    Ok((rt, provider, config, model_name))
}

pub(super) fn search_runtime() -> Result<&'static tokio::runtime::Runtime, ToolCallResult> {
    SEARCH_RUNTIME
        .get_or_init(|| tokio::runtime::Runtime::new().map_err(|err| err.to_string()))
        .as_ref()
        .map_err(|err| ToolCallResult::error(format!("Failed to create runtime: {err}")))
}

pub(super) fn cached_search_context(
    rt: &tokio::runtime::Runtime,
    config: &vera_core::config::VeraConfig,
    backend: vera_core::config::InferenceBackend,
) -> Result<Arc<vera_core::retrieval::search_service::SearchContext>, ToolCallResult> {
    let key = search_context_key(config, backend)?;
    let mut guard = SEARCH_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(cached) = guard.as_ref().filter(|cached| cached.key == key) {
        return Ok(Arc::clone(&cached.context));
    }

    let context = Arc::new(
        rt.block_on(vera_core::retrieval::search_service::SearchContext::new(
            config, backend,
        )),
    );
    *guard = Some(CachedSearchContext {
        key,
        context: Arc::clone(&context),
    });
    Ok(context)
}

fn search_context_key(
    config: &vera_core::config::VeraConfig,
    backend: vera_core::config::InferenceBackend,
) -> Result<String, ToolCallResult> {
    let config_json = serde_json::to_string(config).map_err(|error| {
        ToolCallResult::error(format!("Failed to serialize search configuration: {error}"))
    })?;
    Ok(format!("{backend}|{config_json}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_search_context_reuses_context_for_same_key() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let config = vera_core::config::VeraConfig::default();
        let backend = vera_core::config::InferenceBackend::Api;

        let first = cached_search_context(&rt, &config, backend).unwrap();
        let second = cached_search_context(&rt, &config, backend).unwrap();

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn search_context_key_does_not_collapse_non_finite_config() {
        // serde_json renders non-finite floats as `null` instead of failing,
        // so the key must still distinguish such a config from the default.
        let mut config = vera_core::config::VeraConfig::default();
        config.retrieval.rrf_k = f64::NAN;

        let key = search_context_key(&config, vera_core::config::InferenceBackend::Api)
            .expect("VeraConfig is always serializable");
        let default_key = search_context_key(
            &vera_core::config::VeraConfig::default(),
            vera_core::config::InferenceBackend::Api,
        )
        .expect("VeraConfig is always serializable");

        assert!(!key.is_empty());
        assert_ne!(key, default_key);
    }
}
