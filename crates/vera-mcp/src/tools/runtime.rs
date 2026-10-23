//! Process-lifetime search runtime, context cache, and repository watchers.

use std::collections::{BTreeMap, HashMap, hash_map::Entry};
use std::sync::{Arc, Mutex, OnceLock};

use crate::protocol::ToolCallResult;
use crate::watcher::WatchHandle;
use serde::Serialize;

/// Watcher handles keyed by canonical repository path. Each handle is kept
/// alive for the lifetime of the MCP server process.
static WATCHERS: OnceLock<Mutex<HashMap<std::path::PathBuf, WatchHandle>>> = OnceLock::new();

static SEARCH_CONTEXT: Mutex<Option<CachedSearchContext>> = Mutex::new(None);

static SEARCH_RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();

static LOADED_INDEXES: OnceLock<Mutex<BTreeMap<std::path::PathBuf, LoadedIndex>>> = OnceLock::new();

#[derive(Clone, Debug, Serialize)]
pub(super) struct LoadedIndex {
    pub name: String,
    pub index_dir: String,
    pub repository_root: String,
    pub index_mode: Option<String>,
    pub file_count: u64,
    pub chunk_count: u64,
    pub index_size_bytes: u64,
}

fn watchers() -> &'static Mutex<HashMap<std::path::PathBuf, WatchHandle>> {
    WATCHERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn loaded_indexes() -> &'static Mutex<BTreeMap<std::path::PathBuf, LoadedIndex>> {
    LOADED_INDEXES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) fn load_index(path: &str) -> Result<LoadedIndex, ToolCallResult> {
    let supplied_dir = std::path::Path::new(path);
    let index_dir = supplied_dir
        .canonicalize()
        .map_err(|error| ToolCallResult::error(format!("Cannot resolve index path: {error}")))?;
    if !index_dir.is_dir() {
        return Err(ToolCallResult::error(format!(
            "Index path is not a directory: {}",
            index_dir.display()
        )));
    }

    let metadata_path = index_dir.join("metadata.db");
    let metadata = vera_core::storage::metadata::MetadataStore::open_existing(&metadata_path)
        .map_err(|error| ToolCallResult::error(format!("Cannot load index metadata: {error:#}")))?;
    let stored_root = metadata
        .get_index_meta(vera_core::indexing::pipeline::INDEX_REPOSITORY_ROOT_KEY)
        .map_err(|error| ToolCallResult::error(format!("Cannot read index metadata: {error:#}")))?;
    let repository_root = match stored_root {
        Some(root) => std::path::PathBuf::from(root),
        None if index_dir.file_name().is_some_and(|name| name == ".vera") => index_dir
            .parent()
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| ToolCallResult::error("Index path has no repository root"))?,
        None => {
            return Err(ToolCallResult::error(
                "Index metadata has no repository_root; load a Vera index with repository scope metadata.",
            ));
        }
    };
    let repository_root = repository_root.canonicalize().map_err(|error| {
        ToolCallResult::error(format!(
            "Cannot resolve index repository root {}: {error}",
            repository_root.display()
        ))
    })?;

    {
        let indexes = loaded_indexes()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(existing) = indexes.get(&repository_root) {
            if existing.index_dir == index_dir.display().to_string() {
                return Ok(existing.clone());
            }
            return Err(ToolCallResult::error(format!(
                "Repository scope {} is already registered to index {}",
                existing.repository_root, existing.index_dir
            )));
        }
    }

    let stats = vera_core::stats::collect_stats_from_index_dir(&index_dir, &repository_root)
        .map_err(|error| ToolCallResult::error(format!("Cannot validate index: {error:#}")))?;
    let loaded = LoadedIndex {
        name: repository_root
            .file_name()
            .unwrap_or(repository_root.as_os_str())
            .to_string_lossy()
            .into_owned(),
        index_dir: index_dir.display().to_string(),
        repository_root: repository_root.display().to_string(),
        index_mode: metadata
            .get_index_meta(vera_core::indexing::pipeline::INDEX_MODE_KEY)
            .map_err(|error| {
                ToolCallResult::error(format!("Cannot read index metadata: {error:#}"))
            })?,
        file_count: stats.file_count,
        chunk_count: stats.chunk_count,
        index_size_bytes: stats.index_size_bytes,
    };

    let mut indexes = loaded_indexes()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = repository_root;
    if let Some(existing) = indexes.get(&root) {
        if existing.index_dir != loaded.index_dir {
            return Err(ToolCallResult::error(format!(
                "Repository scope {} is already registered to index {}",
                existing.repository_root, existing.index_dir
            )));
        }
    }
    indexes.insert(root, loaded.clone());
    Ok(loaded)
}

pub(super) fn list_loaded_indexes() -> Vec<LoadedIndex> {
    loaded_indexes()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .values()
        .cloned()
        .collect()
}

/// Select the most specific registered repository scope containing `path`.
/// With no selector, a single registered index is unambiguous; multiple
/// indexes require a scope path or index name.
pub(super) fn select_loaded_index(
    scope_path: Option<&std::path::Path>,
    index_name: Option<&str>,
) -> Result<Option<LoadedIndex>, ToolCallResult> {
    let indexes = loaded_indexes()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    select_index_from(&indexes, scope_path, index_name).map_err(ToolCallResult::error)
}

fn select_index_from(
    indexes: &BTreeMap<std::path::PathBuf, LoadedIndex>,
    scope_path: Option<&std::path::Path>,
    index_name: Option<&str>,
) -> Result<Option<LoadedIndex>, String> {
    if indexes.is_empty() {
        return Ok(None);
    }
    if scope_path.is_some() && index_name.is_some() {
        return Err("Specify either 'scope_path' or 'index_name', not both.".to_string());
    }
    if let Some(name) = index_name {
        let mut matching = indexes.values().filter(|index| index.name == name);
        let selected = matching.next().ok_or_else(|| {
            format!("No loaded index is named '{name}'. Call list_indexes to see available names.")
        })?;
        if matching.next().is_some() {
            return Err(format!(
                "Multiple loaded indexes are named '{name}'. Select one using 'scope_path'."
            ));
        }
        return Ok(Some(selected.clone()));
    }
    let Some(path) = scope_path else {
        if indexes.len() == 1 {
            return Ok(indexes.values().next().cloned());
        }
        return Err("Multiple indexes are loaded. Supply an absolute scope_path within the desired repository, or call list_indexes.".to_string());
    };
    if !path.is_absolute() {
        return Err(
            "scope_path must be an absolute path so Vera can select its repository index."
                .to_string(),
        );
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("Cannot resolve scope_path {}: {error}", path.display()))?;
    indexes
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, index)| Some(index.clone()))
        .ok_or_else(|| {
            format!(
                "No loaded index covers scope_path {}. Call load_index with the index for that repository.",
                path.display()
            )
        })
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
    fn loaded_index_selection_routes_to_most_specific_scope() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let file = nested.join("file.rs");
        std::fs::write(&file, "").unwrap();
        let mut indexes = BTreeMap::new();
        indexes.insert(
            root.path().canonicalize().unwrap(),
            test_loaded_index(root.path(), root.path().join(".vera")),
        );
        indexes.insert(
            nested.canonicalize().unwrap(),
            test_loaded_index(&nested, nested.join(".vera")),
        );

        let selected = select_index_from(&indexes, Some(&file), None)
            .unwrap()
            .unwrap();
        assert_eq!(
            selected.repository_root,
            nested.canonicalize().unwrap().display().to_string()
        );
    }

    #[test]
    fn loaded_index_selection_requires_path_when_scopes_are_ambiguous() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut indexes = BTreeMap::new();
        indexes.insert(
            first.path().to_path_buf(),
            test_loaded_index(first.path(), first.path().join(".vera")),
        );
        indexes.insert(
            second.path().to_path_buf(),
            test_loaded_index(second.path(), second.path().join(".vera")),
        );

        let error = select_index_from(&indexes, None, None).unwrap_err();
        assert!(error.contains("Multiple indexes are loaded"));
    }

    #[test]
    fn loaded_index_selection_accepts_repository_folder_name() {
        let root = tempfile::tempdir().unwrap();
        let mut indexes = BTreeMap::new();
        indexes.insert(
            root.path().to_path_buf(),
            test_loaded_index(root.path(), root.path().join(".vera")),
        );

        let name = root.path().file_name().unwrap().to_str().unwrap();
        let selected = select_index_from(&indexes, None, Some(name)).unwrap();
        assert_eq!(
            selected.unwrap().repository_root,
            root.path().display().to_string()
        );
    }

    #[test]
    fn loaded_index_selection_rejects_ambiguous_names() {
        let first_parent = tempfile::tempdir().unwrap();
        let second_parent = tempfile::tempdir().unwrap();
        let first = first_parent.path().join("repo");
        let second = second_parent.path().join("repo");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let mut indexes = BTreeMap::new();
        indexes.insert(
            first.clone(),
            test_loaded_index(&first, first.join(".vera")),
        );
        indexes.insert(
            second.clone(),
            test_loaded_index(&second, second.join(".vera")),
        );

        let error = select_index_from(&indexes, None, Some("repo")).unwrap_err();
        assert!(error.contains("Multiple loaded indexes are named 'repo'"));
    }

    fn test_loaded_index(root: &std::path::Path, index_dir: std::path::PathBuf) -> LoadedIndex {
        LoadedIndex {
            name: root
                .file_name()
                .unwrap_or(root.as_os_str())
                .to_string_lossy()
                .into_owned(),
            index_dir: index_dir.display().to_string(),
            repository_root: root.display().to_string(),
            index_mode: None,
            file_count: 1,
            chunk_count: 1,
            index_size_bytes: 1,
        }
    }

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
