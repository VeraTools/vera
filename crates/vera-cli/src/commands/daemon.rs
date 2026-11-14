//! Persistent local search daemon and its private CLI protocol.

#[cfg(not(unix))]
use anyhow::Result;

type SearchOutcomeResult = anyhow::Result<(
    Vec<vera_core::types::SearchResult>,
    vera_core::retrieval::search_service::SearchTimings,
)>;
type SearchOutcomes = Vec<SearchOutcomeResult>;

#[cfg(unix)]
mod local {
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::Arc;

    use anyhow::{Context, Result, bail};
    use serde::{Deserialize, Serialize};
    use vera_core::config::{InferenceBackend, VeraConfig};
    use vera_core::retrieval::cross_index::{IndexSearch, search_indexes};
    use vera_core::retrieval::search_service::{SearchContext, SearchTimings};
    use vera_core::types::{SearchFilters, SearchResult, SearchScope};

    #[path = "daemon_transport.rs"]
    mod transport;

    use transport::{
        SocketGuard, acquire_daemon_lock, daemon_socket_path, existing_socket_path,
        read_async_frame, read_frame, remove_stale_socket, write_async_frame, write_frame,
    };

    const PROTOCOL_VERSION: u32 = 1;

    #[derive(Serialize, Deserialize)]
    struct SearchRequest {
        protocol_version: u32,
        indexes: Vec<SearchIndexRequest>,
        queries: Vec<String>,
        intent: Option<String>,
        config: VeraConfig,
        backend: InferenceBackend,
        result_limit: usize,
        deep: bool,
    }

    #[derive(Serialize, Deserialize)]
    struct SearchIndexRequest {
        index_dir: PathBuf,
        filters: SearchFiltersRequest,
    }

    #[derive(Serialize, Deserialize)]
    struct SearchFiltersRequest {
        language: Option<String>,
        path_glob: Vec<String>,
        exact_paths: Option<Vec<String>>,
        symbol_type: Option<String>,
        scope: Option<SearchScope>,
        include_generated: Option<bool>,
    }

    impl From<&SearchFilters> for SearchFiltersRequest {
        fn from(filters: &SearchFilters) -> Self {
            let mut exact_paths = filters
                .exact_paths
                .as_ref()
                .map(|paths| paths.iter().cloned().collect::<Vec<_>>());
            if let Some(paths) = &mut exact_paths {
                paths.sort();
            }
            Self {
                language: filters.language.clone(),
                path_glob: filters.path_glob.clone(),
                exact_paths,
                symbol_type: filters.symbol_type.clone(),
                scope: filters.scope,
                include_generated: filters.include_generated,
            }
        }
    }

    impl From<SearchFiltersRequest> for SearchFilters {
        fn from(filters: SearchFiltersRequest) -> Self {
            Self {
                language: filters.language,
                path_glob: filters.path_glob,
                exact_paths: filters
                    .exact_paths
                    .map(|paths| Arc::new(paths.into_iter().collect::<HashSet<_>>())),
                symbol_type: filters.symbol_type,
                scope: filters.scope,
                include_generated: filters.include_generated,
            }
        }
    }

    #[derive(Serialize, Deserialize)]
    struct DaemonResponse {
        protocol_version: u32,
        body: ResponseBody,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum ResponseBody {
        Search { outcomes: Vec<SearchOutcome> },
        Error { message: String },
    }

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "status", rename_all = "snake_case")]
    enum SearchOutcome {
        Success {
            results: Vec<SearchResult>,
            timings: SearchTimings,
        },
        Error {
            message: String,
        },
    }

    struct CachedContext {
        key: String,
        context: Arc<SearchContext>,
    }

    struct DaemonState {
        context: Option<CachedContext>,
        keyword_context: Arc<SearchContext>,
    }

    impl DaemonState {
        fn new() -> Self {
            Self {
                context: None,
                keyword_context: Arc::new(SearchContext::bm25_only()),
            }
        }

        async fn search_context(
            &mut self,
            config: &VeraConfig,
            backend: InferenceBackend,
        ) -> Result<Arc<SearchContext>> {
            let key = format!("{backend}|{}", serde_json::to_string(config)?);
            if let Some(cached) = self.context.as_ref().filter(|cached| cached.key == key) {
                return Ok(Arc::clone(&cached.context));
            }

            let context = Arc::new(SearchContext::new(config, backend).await);
            self.context = Some(CachedContext {
                key,
                context: Arc::clone(&context),
            });
            Ok(context)
        }
    }

    /// Try the daemon before constructing any per-command search context.
    ///
    /// `None` means no daemon is listening and the caller should use its
    /// existing local search path. Once connected, protocol/search errors are
    /// returned to the user rather than retried locally.
    pub(crate) fn try_search(
        config: &VeraConfig,
        backend: InferenceBackend,
        indexes: &[(PathBuf, PathBuf, SearchFilters)],
        queries: &[String],
        intent: Option<&str>,
        result_limit: usize,
        deep: bool,
    ) -> Result<Option<super::SearchOutcomes>> {
        let Some(socket_path) = existing_socket_path() else {
            eprintln!("vera: local");
            return Ok(None);
        };
        let mut stream = match UnixStream::connect(&socket_path) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                eprintln!("vera: local");
                return Ok(None);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to connect to Vera daemon at {}",
                        socket_path.display()
                    )
                });
            }
        };
        eprintln!("vera: daemon");

        let request = SearchRequest {
            protocol_version: PROTOCOL_VERSION,
            indexes: indexes
                .iter()
                .map(|(_, index_dir, filters)| SearchIndexRequest {
                    index_dir: index_dir.clone(),
                    filters: SearchFiltersRequest::from(filters),
                })
                .collect(),
            queries: queries.to_vec(),
            intent: intent.map(str::to_owned),
            config: config.clone(),
            backend,
            result_limit,
            deep,
        };
        write_frame(&mut stream, &request)?;
        let response: DaemonResponse = read_frame(&mut stream)?;
        if response.protocol_version != PROTOCOL_VERSION {
            bail!(
                "Vera daemon protocol mismatch (client {}, server {}). Restart the daemon with the current Vera version.",
                PROTOCOL_VERSION,
                response.protocol_version
            );
        }

        match response.body {
            ResponseBody::Search { outcomes } => Ok(Some(
                outcomes
                    .into_iter()
                    .map(|outcome| match outcome {
                        SearchOutcome::Success { results, timings } => Ok((results, timings)),
                        SearchOutcome::Error { message } => Err(anyhow::anyhow!(message)),
                    })
                    .collect(),
            )),
            ResponseBody::Error { message } => Err(anyhow::anyhow!("Vera daemon: {message}")),
        }
    }

    pub(crate) fn run() -> Result<()> {
        let socket_path = daemon_socket_path()?;
        let _lock = acquire_daemon_lock(&socket_path)?;
        remove_stale_socket(&socket_path)?;

        let listener = StdUnixListener::bind(&socket_path)
            .with_context(|| format!("failed to bind daemon socket {}", socket_path.display()))?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to secure daemon socket {}", socket_path.display()))?;
        listener.set_nonblocking(true)?;
        let _socket_guard = SocketGuard(socket_path.clone());

        eprintln!(
            "Vera search daemon listening at {}. Press Ctrl-C to stop.",
            socket_path.display()
        );

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("failed to create daemon runtime")?;
        runtime.block_on(async move {
            let listener = tokio::net::UnixListener::from_std(listener)
                .context("failed to initialize daemon listener")?;
            let mut state = DaemonState::new();
            let shutdown = tokio::signal::ctrl_c();
            tokio::pin!(shutdown);

            loop {
                tokio::select! {
                    signal = &mut shutdown => {
                        signal.context("failed to listen for Ctrl-C")?;
                        break;
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                if let Err(error) = handle_client(stream, &mut state).await {
                                    tracing::warn!(%error, "daemon request failed");
                                }
                            }
                            Err(error) => tracing::warn!(%error, "failed to accept daemon connection"),
                        }
                    }
                }
            }
            Ok(())
        })
    }

    async fn handle_client(
        mut stream: tokio::net::UnixStream,
        state: &mut DaemonState,
    ) -> Result<()> {
        let request: SearchRequest = read_async_frame(&mut stream).await?;
        eprintln!(
            "search request: queries={}, indexes={}",
            request.queries.len(),
            request.indexes.len()
        );
        let response = process_search(state, request).await;
        write_async_frame(&mut stream, &response).await
    }

    async fn process_search(state: &mut DaemonState, request: SearchRequest) -> DaemonResponse {
        if request.protocol_version != PROTOCOL_VERSION {
            return error_response(format!(
                "protocol mismatch (client {}, server {})",
                request.protocol_version, PROTOCOL_VERSION
            ));
        }
        if request.indexes.is_empty() {
            return error_response("search request contains no indexes");
        }

        let mut prepared = Vec::with_capacity(request.indexes.len());
        let mut outcomes = std::iter::repeat_with(|| None)
            .take(request.indexes.len())
            .collect::<Vec<Option<SearchOutcome>>>();
        for (index, request_index) in request.indexes.into_iter().enumerate() {
            match vera_core::indexing::is_full_text_only(&request_index.index_dir) {
                Ok(full_text_only) => prepared.push((
                    index,
                    request_index.index_dir,
                    SearchFilters::from(request_index.filters),
                    full_text_only,
                )),
                Err(error) if index == 0 => return error_response(format!("{error:#}")),
                Err(error) => {
                    outcomes[index] = Some(SearchOutcome::Error {
                        message: format!("{error:#}"),
                    });
                }
            }
        }

        let context = if prepared
            .iter()
            .any(|(_, _, _, full_text_only)| !full_text_only)
        {
            match state.search_context(&request.config, request.backend).await {
                Ok(context) => context,
                Err(error) => return error_response(format!("{error:#}")),
            }
        } else {
            Arc::clone(&state.keyword_context)
        };

        let index_searches = prepared
            .iter()
            .map(|(_, index_dir, filters, _)| IndexSearch {
                context: &context,
                index_dir,
                filters,
            })
            .collect::<Vec<_>>();
        let search_results = match search_indexes(
            &index_searches,
            &request.queries,
            request.intent.as_deref(),
            &request.config,
            request.result_limit,
            request.deep,
        )
        .await
        {
            Ok(results) => results,
            Err(error) => return error_response(format!("{error:#}")),
        };
        for ((original_index, _, _, _), result) in prepared.into_iter().zip(search_results) {
            outcomes[original_index] = Some(match result {
                Ok((results, timings)) => SearchOutcome::Success { results, timings },
                Err(error) => SearchOutcome::Error {
                    message: format!("{error:#}"),
                },
            });
        }

        DaemonResponse {
            protocol_version: PROTOCOL_VERSION,
            body: ResponseBody::Search {
                outcomes: outcomes
                    .into_iter()
                    .map(|outcome| outcome.expect("every index has a search outcome"))
                    .collect(),
            },
        }
    }

    fn error_response(message: impl Into<String>) -> DaemonResponse {
        DaemonResponse {
            protocol_version: PROTOCOL_VERSION,
            body: ResponseBody::Error {
                message: message.into(),
            },
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn search_filters_wire_round_trip_preserves_exact_paths() {
            let filters = SearchFilters {
                language: Some("rust".into()),
                path_glob: vec!["src/**".into()],
                exact_paths: Some(Arc::new(HashSet::from([
                    "src/main.rs".into(),
                    "src/lib.rs".into(),
                ]))),
                symbol_type: Some("function".into()),
                scope: Some(SearchScope::Source),
                include_generated: Some(false),
            };

            let restored = SearchFilters::from(SearchFiltersRequest::from(&filters));
            assert_eq!(restored.language, filters.language);
            assert_eq!(restored.path_glob, filters.path_glob);
            assert_eq!(restored.exact_paths, filters.exact_paths);
            assert_eq!(restored.symbol_type, filters.symbol_type);
            assert_eq!(restored.scope, filters.scope);
            assert_eq!(restored.include_generated, filters.include_generated);
        }
    }
}

#[cfg(unix)]
pub(crate) use local::{run, try_search};

#[cfg(not(unix))]
pub(crate) fn run() -> Result<()> {
    anyhow::bail!("`vera daemon` is currently supported on Unix platforms only")
}

#[cfg(not(unix))]
pub(crate) fn try_search(
    _config: &vera_core::config::VeraConfig,
    _backend: vera_core::config::InferenceBackend,
    _indexes: &[(
        std::path::PathBuf,
        std::path::PathBuf,
        vera_core::types::SearchFilters,
    )],
    _queries: &[String],
    _intent: Option<&str>,
    _result_limit: usize,
    _deep: bool,
) -> Result<Option<SearchOutcomes>> {
    Ok(None)
}
