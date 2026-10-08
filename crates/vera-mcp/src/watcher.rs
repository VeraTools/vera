//! Background file watcher for automatic index updates in MCP mode.
//!
//! Watches a project directory for file changes and triggers incremental
//! index updates after a debounce period. This keeps the index fresh
//! without requiring manual update calls.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use tracing::{debug, info, warn};

use vera_core::config::{IndexingConfig, InferenceBackend, VeraConfig};
use vera_core::discovery::ExclusionMatcher;
use vera_core::indexing::UpdateSummary;
use vera_core::storage::metadata::MetadataStore;

/// Debounce interval: wait this long after the last file change before updating.
const DEBOUNCE_SECS: u64 = 2;
const MAX_UPDATE_PASSES: usize = 8;

#[derive(Default)]
struct UpdateState {
    updating: bool,
    pending: bool,
}

type UpdateStateHandle = Arc<Mutex<UpdateState>>;

/// Mark an event as pending, or claim the right to start an update if none is
/// currently running.
fn begin_update(state: &Mutex<UpdateState>) -> bool {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if state.updating {
        state.pending = true;
        false
    } else {
        state.updating = true;
        state.pending = false;
        true
    }
}

#[cfg(test)]
fn mark_update_pending(state: &Mutex<UpdateState>) {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if state.updating {
        state.pending = true;
    }
}

/// Runs one incremental update. Built once per watcher and reused across cycles,
/// so the tokio runtime and the embedding model are paid for once rather than
/// per debounce window.
trait IncrementalUpdate: Send + Sync {
    fn update(&self, repo_path: &Path, config: &VeraConfig)
    -> Result<UpdateSummary, anyhow::Error>;
}

type Engine = Arc<dyn IncrementalUpdate>;
type EngineBuilder = Arc<dyn Fn(&WatchRuntime) -> Result<Engine, anyhow::Error> + Send + Sync>;
type RuntimeResolver = Arc<dyn Fn() -> Result<WatchRuntime, anyhow::Error> + Send + Sync>;

#[derive(Clone)]
struct WatchRuntime {
    config: VeraConfig,
    backend: InferenceBackend,
    provider_identity: String,
}

struct CachedEngine {
    provider_identity: String,
    engine: Engine,
}

/// Holds the update engine for the current provider identity.
///
/// Construction is deferred to the first cycle so starting a watcher never
/// loads a model, and a construction failure stays a per-cycle error that the
/// next cycle can retry instead of killing the watcher.
struct SharedEngine {
    build: EngineBuilder,
    cached: Mutex<Option<CachedEngine>>,
}

impl SharedEngine {
    fn get(&self, runtime: &WatchRuntime) -> Result<Engine, anyhow::Error> {
        let mut guard = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = guard.as_ref()
            && cached.provider_identity == runtime.provider_identity
        {
            return Ok(Arc::clone(&cached.engine));
        }
        let engine = (self.build)(runtime)?;
        *guard = Some(CachedEngine {
            provider_identity: runtime.provider_identity.clone(),
            engine: Arc::clone(&engine),
        });
        Ok(engine)
    }
}

/// The production engine: one tokio runtime and one embedding provider for a
/// single effective provider identity.
struct EmbeddingUpdateEngine {
    runtime: tokio::runtime::Runtime,
    provider: vera_core::embedding::DynamicProvider,
    model_name: String,
}

impl EmbeddingUpdateEngine {
    fn build(runtime_config: &WatchRuntime) -> Result<Self, anyhow::Error> {
        let runtime = tokio::runtime::Runtime::new()?;
        let (provider, model_name) =
            runtime.block_on(vera_core::embedding::create_dynamic_provider(
                &runtime_config.config,
                runtime_config.backend,
            ))?;
        Ok(Self {
            runtime,
            provider,
            model_name,
        })
    }
}

impl IncrementalUpdate for EmbeddingUpdateEngine {
    fn update(
        &self,
        repo_path: &Path,
        config: &VeraConfig,
    ) -> Result<UpdateSummary, anyhow::Error> {
        self.runtime
            .block_on(vera_core::indexing::update_repository(
                repo_path,
                &self.provider,
                config,
                &self.model_name,
            ))
    }
}

/// Handle to a running file watcher. Dropping it stops the watcher.
pub struct WatchHandle {
    _watcher: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
}

/// Start watching a project directory for file changes.
///
/// When changes are detected (after debouncing), triggers an incremental
/// index update in a background thread. Returns a handle that keeps the
/// watcher alive; drop it to stop watching.
pub fn start_watching(repo_path: &Path) -> Result<WatchHandle, String> {
    start_watching_internal(repo_path, false)
}

/// Start watching with progress logs printed to stderr.
///
/// Intended for `vera watch` CLI mode where users expect visible activity.
pub fn start_watching_with_progress(repo_path: &Path) -> Result<WatchHandle, String> {
    start_watching_internal(repo_path, true)
}

fn start_watching_internal(repo_path: &Path, progress_logs: bool) -> Result<WatchHandle, String> {
    let runtime = resolve_watch_runtime()
        .map_err(|error| format!("Failed to resolve watcher runtime configuration: {error}"))?;
    let indexing = runtime.config.indexing.clone();
    let resolve: RuntimeResolver = Arc::new(resolve_watch_runtime);
    let build: EngineBuilder =
        Arc::new(|runtime| Ok(Arc::new(EmbeddingUpdateEngine::build(runtime)?) as Engine));

    start_watching_with_runtime(
        repo_path,
        progress_logs,
        Duration::from_secs(DEBOUNCE_SECS),
        &indexing,
        resolve,
        build,
    )
}

#[cfg(test)]
fn start_watching_with(
    repo_path: &Path,
    progress_logs: bool,
    debounce: Duration,
    indexing: &IndexingConfig,
    build: EngineBuilder,
) -> Result<WatchHandle, String> {
    let runtime = WatchRuntime {
        config: VeraConfig {
            indexing: indexing.clone(),
            ..VeraConfig::default()
        },
        backend: InferenceBackend::Api,
        provider_identity: "test".to_string(),
    };
    let resolve: RuntimeResolver = Arc::new(move || Ok(runtime.clone()));
    start_watching_with_runtime(repo_path, progress_logs, debounce, indexing, resolve, build)
}

fn start_watching_with_runtime(
    repo_path: &Path,
    progress_logs: bool,
    debounce: Duration,
    indexing: &IndexingConfig,
    resolve: RuntimeResolver,
    build: EngineBuilder,
) -> Result<WatchHandle, String> {
    let repo_path = repo_path
        .canonicalize()
        .map_err(|e| format!("Failed to resolve path: {e}"))?;

    let idx_dir = vera_core::indexing::index_dir(&repo_path);
    if !idx_dir.exists() {
        // This error surfaces to both audiences: `vera watch` (CLI) and the
        // MCP `start_watch` tool, so it cannot assume MCP wording.
        return Err(
            "No index found. Run `vera index` (CLI) or call search_code (MCP) first to auto-index."
                .to_string(),
        );
    }

    ExclusionMatcher::new(&repo_path, indexing)
        .map_err(|e| format!("Failed to build watcher exclusions: {e:#}"))?;

    // The index directory is filtered on its own, never through `exclusions`:
    // an update cycle writes into it, so a watcher that reacts to those writes
    // re-triggers itself. `no_default_excludes`, or a stored `default_excludes`
    // without `.vera`, would switch that guard off. The same applies to the
    // `.vera.build`/`.vera.old` staging siblings a full index swaps through.
    let index_dir = idx_dir.clone();

    let update_state = Arc::new(Mutex::new(UpdateState::default()));
    let update_state_clone = Arc::clone(&update_state);
    let repo_clone = repo_path.clone();
    let engine = Arc::new(SharedEngine {
        build,
        cached: Mutex::new(None),
    });
    let resolve_runtime = Arc::clone(&resolve);

    let mut debouncer = new_debouncer(
        debounce,
        move |events: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            let events = match events {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "File watcher error");
                    return;
                }
            };

            let runtime = match resolve_runtime() {
                Ok(runtime) => runtime,
                Err(error) => {
                    warn!(error = %error, "Failed to resolve watcher runtime configuration");
                    return;
                }
            };
            let exclusions = match ExclusionMatcher::new(&repo_clone, &runtime.config.indexing) {
                Ok(exclusions) => exclusions,
                Err(error) => {
                    warn!(error = %error, "Failed to resolve watcher exclusions");
                    return;
                }
            };
            let tracked_paths = load_tracked_paths(&index_dir, &repo_clone);

            // Ignore the index directory always (including the `.vera.build`
            // and `.vera.old` staging siblings a full index swaps in and
            // out), plus the directories indexing would not walk anyway
            // (target, node_modules, ...). A build churning target/ used to
            // start a full update cycle per debounce window.
            let has_relevant_changes = events.iter().any(|e| {
                e.kind == DebouncedEventKind::Any
                    && !vera_core::indexing::pipeline::path_in_index_artifacts(&index_dir, &e.path)
                    && is_relevant_change(&e.path, &exclusions, &tracked_paths)
            });

            if !has_relevant_changes {
                return;
            }

            // Coalesce events that arrive while an update is running. The
            // completion path consumes this flag and performs a trailing pass.
            if !begin_update(&update_state_clone) {
                debug!("Skipping auto-update: previous update still running");
                if progress_logs {
                    eprintln!(
                        "[watch] update already running, changes will be picked up next cycle"
                    );
                }
                return;
            }

            if progress_logs {
                eprintln!("[watch] file changes detected, starting incremental update");
            }

            let repo = repo_clone.clone();
            let state = Arc::clone(&update_state_clone);
            let engine = Arc::clone(&engine);

            std::thread::spawn(move || {
                run_incremental_update(&engine, &repo, &runtime, &state, progress_logs);
            });
        },
    )
    .map_err(|e| format!("Failed to create file watcher: {e}"))?;

    debouncer
        .watcher()
        .watch(&repo_path, notify::RecursiveMode::Recursive)
        .map_err(|e| watch_failure_message(&repo_path, &e))?;

    info!(path = %repo_path.display(), "Started file watcher for auto-indexing");

    Ok(WatchHandle {
        _watcher: debouncer,
    })
}

fn resolve_watch_runtime() -> Result<WatchRuntime, anyhow::Error> {
    let backend = vera_core::config::resolve_backend(None);
    let mut config = crate::saved_config::load_saved_runtime_config();
    config.adjust_for_backend(backend);
    let model_name = match backend {
        InferenceBackend::Api => std::env::var("EMBEDDING_MODEL_ID").unwrap_or_default(),
        InferenceBackend::OnnxJina(_) => vera_core::local_models::configured_local_model_name(),
        InferenceBackend::PotionCode => vera_core::local_models::potion_code_model_name(),
    };
    let endpoint = match backend {
        InferenceBackend::Api => std::env::var("EMBEDDING_MODEL_BASE_URL").unwrap_or_default(),
        _ => String::new(),
    };
    let prefix_env = |key: &str| std::env::var(key).unwrap_or_default();
    let (query_prefix, document_prefix) = match backend {
        InferenceBackend::Api => (
            prefix_env("EMBEDDING_QUERY_PREFIX"),
            prefix_env("EMBEDDING_DOCUMENT_PREFIX"),
        ),
        InferenceBackend::OnnxJina(_) => (
            std::env::var(vera_core::local_models::LOCAL_EMBEDDING_QUERY_PREFIX_ENV)
                .or_else(|_| std::env::var("VERA_EMBEDDING_QUERY_PREFIX"))
                .unwrap_or_default(),
            String::new(),
        ),
        InferenceBackend::PotionCode => (String::new(), String::new()),
    };
    let provider_identity = format!(
        "{backend}|endpoint={endpoint}|model={model_name}|query_prefix={query_prefix}|document_prefix={document_prefix}|timeout={}|retries={}|gpu_mem_limit={}|low_vram={}",
        config.embedding.timeout_secs,
        config.embedding.max_retries,
        config.embedding.gpu_mem_limit_mb,
        config.embedding.low_vram,
    );
    Ok(WatchRuntime {
        config,
        backend,
        provider_identity,
    })
}

fn load_tracked_paths(idx_dir: &Path, repo_path: &Path) -> HashSet<std::path::PathBuf> {
    let metadata_path = idx_dir.join("metadata.db");
    let Ok(metadata) = MetadataStore::open(&metadata_path) else {
        return HashSet::new();
    };
    metadata
        .tracked_files()
        .unwrap_or_default()
        .into_iter()
        .map(|path| repo_path.join(path))
        .collect()
}

/// Keep excluded descendants quiet, but let a path that was indexed before it
/// became excluded reach the incremental scan so its old rows can be purged.
fn is_relevant_change(
    path: &Path,
    exclusions: &ExclusionMatcher,
    tracked_paths: &HashSet<std::path::PathBuf>,
) -> bool {
    !exclusions.is_excluded(path) || path.is_dir() || tracked_paths.contains(path)
}

/// Describe a failed `watch()` call, with the remedy for the one failure mode
/// that is both common and fixable: the per-user OS watch limit, which a
/// recursive watch over a repository with build output can exhaust.
fn watch_failure_message(repo_path: &Path, error: &notify::Error) -> String {
    if matches!(error.kind, notify::ErrorKind::MaxFilesWatch) {
        return format!(
            "Failed to watch {}: the OS file watch limit was reached, so the index will not \
             auto-update. Raise it (Linux: sysctl fs.inotify.max_user_watches) or exclude build \
             output directories from the repository.",
            repo_path.display()
        );
    }
    format!("Failed to watch directory: {error}")
}

/// Run an update and any coalesced trailing passes, with a bound so a noisy
/// filesystem cannot keep one worker alive forever.
fn run_incremental_update(
    engine: &SharedEngine,
    repo_path: &Path,
    runtime: &WatchRuntime,
    state: &UpdateStateHandle,
    progress_logs: bool,
) {
    for pass in 0..MAX_UPDATE_PASSES {
        debug!(path = %repo_path.display(), pass = pass + 1, "Auto-update triggered by file changes");

        let result = engine
            .get(runtime)
            .and_then(|engine| engine.update(repo_path, &runtime.config));

        match result {
            Ok(summary) => {
                let changed = summary.files_modified + summary.files_added + summary.files_deleted;
                if changed > 0 {
                    info!(
                        modified = summary.files_modified,
                        added = summary.files_added,
                        deleted = summary.files_deleted,
                        "Auto-update complete"
                    );
                    if progress_logs {
                        eprintln!(
                            "[watch] update complete: {} modified, {} added, {} deleted",
                            summary.files_modified, summary.files_added, summary.files_deleted
                        );
                    }
                } else {
                    debug!("Auto-update: no changes detected");
                    if progress_logs {
                        eprintln!("[watch] no indexable changes detected");
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Auto-update failed");
                if progress_logs {
                    eprintln!("[watch] update failed: {e}");
                }
            }
        }

        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.pending {
            state.updating = false;
            return;
        }
        if pass + 1 == MAX_UPDATE_PASSES {
            warn!(
                path = %repo_path.display(),
                limit = MAX_UPDATE_PASSES,
                "Auto-update reached trailing pass limit; waiting for another file event"
            );
            state.updating = false;
            state.pending = false;
            return;
        }
        state.pending = false;
    }

    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    state.updating = false;
}

#[cfg(test)]
fn test_runtime() -> WatchRuntime {
    WatchRuntime {
        config: VeraConfig::default(),
        backend: InferenceBackend::Api,
        provider_identity: "test".to_string(),
    }
}

#[cfg(test)]
fn test_summary() -> UpdateSummary {
    UpdateSummary {
        embeddings_reused: 0,
        embedding_requests: 0,
        embedding_retries: 0,
        embedding_timeouts: 0,
        embedding_failed_batches: 0,
        phase_secs: Default::default(),
        files_modified: 0,
        files_added: 0,
        files_deleted: 0,
        files_unchanged: 0,
        files_with_tree_sitter_errors: 0,
        files_using_tier0_fallback: 0,
        parse_errors: Vec::new(),
        files_deferred: 0,
        total_chunks: 0,
        elapsed_secs: 0.0,
    }
}

#[cfg(test)]
struct BlockingEngine {
    updates: Arc<std::sync::atomic::AtomicUsize>,
    first_started: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(test)]
impl IncrementalUpdate for BlockingEngine {
    fn update(
        &self,
        _repo_path: &Path,
        _config: &VeraConfig,
    ) -> Result<UpdateSummary, anyhow::Error> {
        let update_number = self
            .updates
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if update_number == 1 {
            self.first_started.wait();
            self.release.wait();
        }
        Ok(test_summary())
    }
}

#[cfg(test)]
struct ContinuousEngine {
    updates: Arc<std::sync::atomic::AtomicUsize>,
    state: UpdateStateHandle,
}

#[cfg(test)]
impl IncrementalUpdate for ContinuousEngine {
    fn update(
        &self,
        _repo_path: &Path,
        _config: &VeraConfig,
    ) -> Result<UpdateSummary, anyhow::Error> {
        self.updates
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        mark_update_pending(&self.state);
        Ok(test_summary())
    }
}

#[cfg(test)]
fn shared_test_engine(engine: Engine) -> SharedEngine {
    let build: EngineBuilder = Arc::new(move |_| Ok(Arc::clone(&engine)));
    SharedEngine {
        build,
        cached: Mutex::new(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use vera_core::config::VeraConfig;
    use vera_core::embedding::{EmbeddingError, EmbeddingProvider};
    use vera_core::indexing::{index_repository, update_repository};
    use vera_core::storage::metadata::MetadataStore;

    const TEST_DEBOUNCE: Duration = Duration::from_millis(300);

    /// Stands in for the real engine: records that it was built, and how many
    /// update cycles ran through it.
    struct CountingEngine {
        updates: Arc<AtomicUsize>,
    }

    impl IncrementalUpdate for CountingEngine {
        fn update(
            &self,
            _repo_path: &Path,
            _config: &VeraConfig,
        ) -> Result<UpdateSummary, anyhow::Error> {
            self.updates.fetch_add(1, Ordering::SeqCst);
            Ok(UpdateSummary {
                embeddings_reused: 0,
                embedding_requests: 0,
                embedding_retries: 0,
                embedding_timeouts: 0,
                embedding_failed_batches: 0,
                phase_secs: Default::default(),
                files_modified: 0,
                files_added: 0,
                files_deleted: 0,
                files_unchanged: 0,
                files_with_tree_sitter_errors: 0,
                files_using_tier0_fallback: 0,
                parse_errors: Vec::new(),
                files_deferred: 0,
                total_chunks: 0,
                elapsed_secs: 0.0,
            })
        }
    }

    struct TestProvider;

    impl EmbeddingProvider for TestProvider {
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            Ok(texts.iter().map(|_| vec![0.0; 8]).collect())
        }

        fn expected_dim(&self) -> Option<usize> {
            Some(8)
        }
    }

    struct IndexingEngine {
        runtime: tokio::runtime::Runtime,
        config: VeraConfig,
    }

    impl IncrementalUpdate for IndexingEngine {
        fn update(
            &self,
            repo_path: &Path,
            _config: &VeraConfig,
        ) -> Result<UpdateSummary, anyhow::Error> {
            self.runtime.block_on(update_repository(
                repo_path,
                &TestProvider,
                &self.config,
                "test-model",
            ))
        }
    }

    fn indexing_builder(config: VeraConfig) -> EngineBuilder {
        Arc::new(move |_| {
            Ok(Arc::new(IndexingEngine {
                runtime: tokio::runtime::Runtime::new()?,
                config: config.clone(),
            }) as Engine)
        })
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Runtime::new()
            .expect("test runtime")
            .block_on(future)
    }

    fn counting_builder(builds: Arc<AtomicUsize>, updates: Arc<AtomicUsize>) -> EngineBuilder {
        Arc::new(move |_| {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(CountingEngine {
                updates: Arc::clone(&updates),
            }) as Engine)
        })
    }

    /// A repository laid out with the directories the watcher has to tell apart.
    fn repo_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".vera")).expect("index dir");
        std::fs::create_dir_all(dir.path().join("src")).expect("src dir");
        std::fs::create_dir_all(dir.path().join("target/debug")).expect("target dir");
        dir
    }

    fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        predicate()
    }

    /// Prove the watcher observes an indexable change and return the settled
    /// update-cycle count as a baseline. Under load a single write can split
    /// across debounce windows and legitimately run a trailing pass, so
    /// callers compare against this baseline instead of asserting an absolute
    /// cycle count.
    fn write_and_settle(path: &std::path::Path, updates: &AtomicUsize) -> usize {
        std::fs::write(path, "fn real() {}").expect("write");
        assert!(
            wait_until(Duration::from_secs(10), || updates.load(Ordering::SeqCst)
                >= 1),
            "an indexable change must trigger an update cycle"
        );
        // Absorb any debounce-split trailing pass before the baseline is read.
        std::thread::sleep(TEST_DEBOUNCE * 6);
        updates.load(Ordering::SeqCst)
    }

    #[test]
    fn event_during_update_triggers_one_trailing_pass() {
        let updates = Arc::new(AtomicUsize::new(0));
        let first_started = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let engine: Engine = Arc::new(BlockingEngine {
            updates: Arc::clone(&updates),
            first_started: Arc::clone(&first_started),
            release: Arc::clone(&release),
        });
        let shared = shared_test_engine(engine);
        let state = Arc::new(Mutex::new(UpdateState::default()));
        assert!(begin_update(&state));
        let runtime = test_runtime();
        let worker_state = Arc::clone(&state);
        let worker = std::thread::spawn(move || {
            run_incremental_update(
                &shared,
                Path::new("/tmp/test-repo"),
                &runtime,
                &worker_state,
                false,
            );
        });

        first_started.wait();
        assert!(
            !begin_update(&state),
            "the mid-update event must be coalesced"
        );
        release.wait();
        worker.join().expect("update worker");

        assert_eq!(updates.load(Ordering::SeqCst), 2);
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!state.updating);
        assert!(!state.pending);
    }

    #[test]
    fn continuous_events_stop_at_the_trailing_pass_limit() {
        let updates = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(Mutex::new(UpdateState::default()));
        let engine: Engine = Arc::new(ContinuousEngine {
            updates: Arc::clone(&updates),
            state: Arc::clone(&state),
        });
        let shared = shared_test_engine(engine);
        assert!(begin_update(&state));

        run_incremental_update(
            &shared,
            Path::new("/tmp/test-repo"),
            &test_runtime(),
            &state,
            false,
        );

        assert_eq!(updates.load(Ordering::SeqCst), MAX_UPDATE_PASSES);
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!state.updating);
        assert!(!state.pending);
    }

    #[test]
    fn engine_is_built_once_across_debounce_cycles() {
        let repo = repo_fixture();
        let builds = Arc::new(AtomicUsize::new(0));
        let updates = Arc::new(AtomicUsize::new(0));

        let _handle = start_watching_with(
            repo.path(),
            false,
            TEST_DEBOUNCE,
            &IndexingConfig::default(),
            counting_builder(Arc::clone(&builds), Arc::clone(&updates)),
        )
        .expect("watcher starts");

        std::fs::write(repo.path().join("src/first.rs"), "fn first() {}").expect("write");
        assert!(
            wait_until(Duration::from_secs(10), || updates.load(Ordering::SeqCst)
                >= 1),
            "first change should trigger an update cycle"
        );

        std::fs::write(repo.path().join("src/second.rs"), "fn second() {}").expect("write");
        assert!(
            wait_until(Duration::from_secs(10), || updates.load(Ordering::SeqCst)
                >= 2),
            "second change should trigger another update cycle"
        );

        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "the engine must be built once and reused, not rebuilt per cycle"
        );
    }

    #[test]
    fn engine_rebuilds_when_provider_identity_changes() {
        let builds = Arc::new(AtomicUsize::new(0));
        let updates = Arc::new(AtomicUsize::new(0));
        let shared = SharedEngine {
            build: counting_builder(Arc::clone(&builds), Arc::clone(&updates)),
            cached: Mutex::new(None),
        };
        let runtime_a = WatchRuntime {
            config: VeraConfig::default(),
            backend: InferenceBackend::Api,
            provider_identity: "model-a".to_string(),
        };

        let first = shared.get(&runtime_a).expect("first engine");
        let reused = shared.get(&runtime_a).expect("reused engine");
        assert!(Arc::ptr_eq(&first, &reused));
        assert_eq!(builds.load(Ordering::SeqCst), 1);

        let runtime_b = WatchRuntime {
            provider_identity: "model-b".to_string(),
            ..runtime_a
        };
        let rebuilt = shared.get(&runtime_b).expect("rebuilt engine");
        assert!(!Arc::ptr_eq(&first, &rebuilt));
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn changes_under_excluded_directories_do_not_trigger_updates() {
        let repo = repo_fixture();
        let builds = Arc::new(AtomicUsize::new(0));
        let updates = Arc::new(AtomicUsize::new(0));

        let _handle = start_watching_with(
            repo.path(),
            false,
            TEST_DEBOUNCE,
            &IndexingConfig::default(),
            counting_builder(Arc::clone(&builds), Arc::clone(&updates)),
        )
        .expect("watcher starts");

        // Presence first: a watcher that never fires would pass the absence
        // assertion below on its own.
        let baseline = write_and_settle(&repo.path().join("src/real.rs"), &updates);

        for i in 0..5 {
            std::fs::write(
                repo.path().join(format!("target/debug/artifact{i}.o")),
                format!("build output {i}"),
            )
            .expect("write");
        }
        std::thread::sleep(TEST_DEBOUNCE * 6);

        assert_eq!(
            updates.load(Ordering::SeqCst),
            baseline,
            "build output under target/ must not trigger an update cycle"
        );
    }

    /// An update cycle writes into `.vera`, so a watcher that reacts to those
    /// writes re-triggers itself forever. The index directory therefore cannot
    /// be filtered through the configurable exclusions: `no_default_excludes`
    /// (and a stored `default_excludes` that omits `.vera`) would switch the
    /// guard off.
    #[test]
    fn index_writes_never_trigger_updates_even_without_default_excludes() {
        let repo = repo_fixture();
        let builds = Arc::new(AtomicUsize::new(0));
        let updates = Arc::new(AtomicUsize::new(0));
        let indexing = IndexingConfig {
            no_default_excludes: true,
            ..IndexingConfig::default()
        };

        let _handle = start_watching_with(
            repo.path(),
            false,
            TEST_DEBOUNCE,
            &indexing,
            counting_builder(Arc::clone(&builds), Arc::clone(&updates)),
        )
        .expect("watcher starts");

        // Presence first: with the default exclusions off, an ordinary source
        // change must still be seen, or the absence assertion below is vacuous.
        let baseline = write_and_settle(&repo.path().join("src/real.rs"), &updates);

        for i in 0..5 {
            std::fs::write(
                repo.path().join(format!(".vera/chunk{i}.db")),
                format!("index write {i}"),
            )
            .expect("write");
        }
        std::thread::sleep(TEST_DEBOUNCE * 6);

        assert_eq!(
            updates.load(Ordering::SeqCst),
            baseline,
            "writes into the index directory must never trigger an update cycle"
        );
    }

    /// A full index build swaps staging siblings (`.vera.build`, `.vera.old`)
    /// next to the live index. Those writes are index-internal too: `Path::
    /// starts_with` on the live dir does not cover them, so the guard must.
    #[test]
    fn staging_dir_writes_never_trigger_updates() {
        let repo = repo_fixture();
        let builds = Arc::new(AtomicUsize::new(0));
        let updates = Arc::new(AtomicUsize::new(0));

        let _handle = start_watching_with(
            repo.path(),
            false,
            TEST_DEBOUNCE,
            &IndexingConfig::default(),
            counting_builder(Arc::clone(&builds), Arc::clone(&updates)),
        )
        .expect("watcher starts");

        let baseline = write_and_settle(&repo.path().join("src/real.rs"), &updates);

        for staging in [".vera.build", ".vera.old"] {
            std::fs::create_dir_all(repo.path().join(staging)).expect("staging dir");
            for i in 0..3 {
                std::fs::write(
                    repo.path().join(format!("{staging}/chunk{i}.json")),
                    format!("staging write {i}"),
                )
                .expect("write");
            }
        }
        std::thread::sleep(TEST_DEBOUNCE * 6);

        assert_eq!(
            updates.load(Ordering::SeqCst),
            baseline,
            "writes into index staging directories must never trigger an update cycle"
        );
    }

    #[test]
    fn replacing_indexed_file_with_excluded_directory_purges_chunks() {
        let repo = tempfile::tempdir().expect("tempdir");
        let target = repo.path().join("target");
        std::fs::write(&target, "fn stale_target_symbol() {}\n").expect("write source");
        let config = VeraConfig::default();

        block_on(index_repository(
            repo.path(),
            &TestProvider,
            &config,
            "test-model",
        ))
        .expect("initial index");

        let index_dir = vera_core::indexing::index_dir(&repo.path().canonicalize().unwrap());
        let metadata = MetadataStore::open(&index_dir.join("metadata.db")).expect("metadata");
        assert!(
            !metadata
                .get_chunks_by_file("target")
                .expect("initial chunks")
                .is_empty(),
            "the regular file must be indexed before the transition"
        );

        let indexing = config.indexing.clone();
        let _handle = start_watching_with(
            repo.path(),
            false,
            TEST_DEBOUNCE,
            &indexing,
            indexing_builder(config),
        )
        .expect("watcher starts");

        std::fs::remove_file(&target).expect("remove source");
        std::fs::create_dir(&target).expect("replace with excluded directory");

        assert!(
            wait_until(Duration::from_secs(10), || metadata
                .get_chunks_by_file("target")
                .expect("chunks after update")
                .is_empty()),
            "the directory transition must trigger an update that purges stale chunks"
        );
    }
}
