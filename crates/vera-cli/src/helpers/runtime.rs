//! Runtime selection, model flags, and cooperative cancellation.

use anyhow::Context;
use clap::Args;

/// Install the process interrupt handler and return a future for its first event.
#[cfg(unix)]
pub fn wait_for_interrupt(
    runtime: &tokio::runtime::Handle,
) -> std::io::Result<impl std::future::Future<Output = ()> + Send + 'static> {
    let _guard = runtime.enter();
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    Ok(async move {
        signal.recv().await;
    })
}

/// Install the process interrupt handler and return a future for its first event.
#[cfg(windows)]
pub fn wait_for_interrupt(
    runtime: &tokio::runtime::Handle,
) -> std::io::Result<impl std::future::Future<Output = ()> + Send + 'static> {
    let _guard = runtime.enter();
    let mut signal = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        signal.recv().await;
    })
}

/// Cancel a spawned operation when signalled, then wait for it to stop safely.
///
/// The operation runs separately so the signal handler is active during synchronous discovery
/// and parsing. If publication has already started, this waits for and returns its real result.
pub async fn cancel_task_on_signal<T, Signal>(
    mut task: tokio::task::JoinHandle<anyhow::Result<T>>,
    signal: Signal,
    cancellation: vera_core::CancellationToken,
    operation_name: &str,
) -> anyhow::Result<T>
where
    Signal: std::future::Future<Output = ()>,
{
    tokio::pin!(signal);

    let result = tokio::select! {
        biased;
        result = &mut task => result,
        _ = &mut signal => {
            cancellation.cancel();
            task.await
        },
    };

    result.with_context(|| format!("{operation_name} task failed"))?
}

/// Whether an error represents cooperative cancellation (typed check).
///
/// Thin wrapper around [`vera_core::is_cancel_error`] so CLI call sites use
/// the typed helper instead of brittle `to_string().contains("cancel")`.
pub fn is_cancel_error(err: &anyhow::Error) -> bool {
    vera_core::is_cancel_error(err)
}

#[derive(Debug, Clone, Default, Args)]
pub struct LocalBackendFlags {
    /// Use Potion Code static embeddings on CPU (the default backend).
    #[arg(long = "potion-code", visible_alias = "potion-cpu", group = "backend")]
    pub potion_code: bool,
    /// Use local ONNX models on CPU.
    #[arg(long = "onnx-jina-cpu", group = "backend")]
    pub onnx_jina_cpu: bool,
    /// Use local ONNX models with CUDA (NVIDIA GPU).
    #[arg(long = "onnx-jina-cuda", group = "backend")]
    pub onnx_jina_cuda: bool,
    /// Use local ONNX models with ROCm (AMD GPU, Linux only).
    #[arg(long = "onnx-jina-rocm", group = "backend")]
    pub onnx_jina_rocm: bool,
    /// Use local ONNX models with DirectML (Windows GPU).
    #[arg(long = "onnx-jina-directml", group = "backend")]
    pub onnx_jina_directml: bool,
    /// Use local ONNX models with CoreML (Apple Silicon).
    #[arg(long = "onnx-jina-coreml", group = "backend")]
    pub onnx_jina_coreml: bool,
    /// Use local ONNX models with OpenVINO (Intel GPU/iGPU, Linux only).
    #[arg(long = "onnx-jina-openvino", group = "backend")]
    pub onnx_jina_openvino: bool,
    /// Alias for --onnx-jina-cpu (backwards compatibility).
    #[arg(long, group = "backend", hide = true)]
    pub local: bool,
}

impl LocalBackendFlags {
    pub fn any_set(&self) -> bool {
        self.potion_code
            || self.onnx_jina_cpu
            || self.onnx_jina_cuda
            || self.onnx_jina_rocm
            || self.onnx_jina_directml
            || self.onnx_jina_coreml
            || self.onnx_jina_openvino
            || self.local
    }

    pub fn explicit_backend(&self) -> Option<vera_core::config::InferenceBackend> {
        self.any_set().then(|| resolve_backend_flags(self))
    }

    pub fn resolve(&self) -> vera_core::config::InferenceBackend {
        resolve_backend_flags(self)
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct LocalEmbeddingModelFlags {
    /// Use CodeRankEmbed instead of Vera's default Jina ONNX embedding model.
    #[arg(
        long = "code-rank-embed",
        alias = "coderankembed",
        group = "local_embedding_source"
    )]
    pub code_rank_embed: bool,
    /// Hugging Face repo id or full Hugging Face URL for a custom local embedding model.
    #[arg(
        long = "embedding-repo",
        value_name = "REPO_OR_URL",
        group = "local_embedding_source"
    )]
    pub embedding_repo: Option<String>,
    /// Local directory containing a custom ONNX embedding model.
    #[arg(
        long = "embedding-dir",
        value_name = "DIR",
        group = "local_embedding_source"
    )]
    pub embedding_dir: Option<String>,
    /// Relative path to the ONNX file inside the selected repo or directory.
    #[arg(long = "embedding-onnx-file", value_name = "PATH")]
    pub embedding_onnx_file: Option<String>,
    /// Relative path to the ONNX external data file inside the selected repo or directory.
    #[arg(
        long = "embedding-onnx-data-file",
        value_name = "PATH",
        conflicts_with = "embedding_no_onnx_data"
    )]
    pub embedding_onnx_data_file: Option<String>,
    /// Use models that do not require an ONNX external data file.
    #[arg(long = "embedding-no-onnx-data")]
    pub embedding_no_onnx_data: bool,
    /// Relative path to the tokenizer file inside the selected repo or directory.
    #[arg(long = "embedding-tokenizer-file", value_name = "PATH")]
    pub embedding_tokenizer_file: Option<String>,
    /// Embedding dimension the model returns.
    #[arg(long = "embedding-dim", value_name = "DIM")]
    pub embedding_dim: Option<usize>,
    /// Pooling strategy for token-level output models.
    #[arg(long = "embedding-pooling", value_name = "POOLING", value_parser = ["mean", "cls", "last-token"])]
    pub embedding_pooling: Option<String>,
    /// Tokenizer truncation length for local embedding inference.
    #[arg(long = "embedding-max-length", value_name = "TOKENS")]
    pub embedding_max_length: Option<usize>,
    /// Optional asymmetric query prefix for models that require it.
    #[arg(long = "embedding-query-prefix", value_name = "TEXT")]
    pub embedding_query_prefix: Option<String>,
    /// Optional asymmetric document prefix for models that require it.
    #[arg(long = "embedding-document-prefix", value_name = "TEXT")]
    pub embedding_document_prefix: Option<String>,
}

impl LocalEmbeddingModelFlags {
    pub fn any_set(&self) -> bool {
        self.code_rank_embed
            || self.embedding_repo.is_some()
            || self.embedding_dir.is_some()
            || self.embedding_onnx_file.is_some()
            || self.embedding_onnx_data_file.is_some()
            || self.embedding_no_onnx_data
            || self.embedding_tokenizer_file.is_some()
            || self.embedding_dim.is_some()
            || self.embedding_pooling.is_some()
            || self.embedding_max_length.is_some()
            || self.embedding_query_prefix.is_some()
            || self.embedding_document_prefix.is_some()
    }
}

/// Resolve an `InferenceBackend` from the per-command boolean flags.
pub fn resolve_backend_flags(flags: &LocalBackendFlags) -> vera_core::config::InferenceBackend {
    use vera_core::config::{InferenceBackend, OnnxExecutionProvider};
    let explicit = if flags.potion_code {
        Some(InferenceBackend::PotionCode)
    } else if flags.onnx_jina_cpu || flags.local {
        Some(InferenceBackend::OnnxJina(OnnxExecutionProvider::Cpu))
    } else if flags.onnx_jina_cuda {
        Some(InferenceBackend::OnnxJina(OnnxExecutionProvider::Cuda))
    } else if flags.onnx_jina_rocm {
        Some(InferenceBackend::OnnxJina(OnnxExecutionProvider::Rocm))
    } else if flags.onnx_jina_directml {
        Some(InferenceBackend::OnnxJina(OnnxExecutionProvider::DirectMl))
    } else if flags.onnx_jina_coreml {
        Some(InferenceBackend::OnnxJina(OnnxExecutionProvider::CoreMl))
    } else if flags.onnx_jina_openvino {
        Some(InferenceBackend::OnnxJina(OnnxExecutionProvider::OpenVino))
    } else {
        None
    };
    vera_core::config::resolve_backend(explicit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_local_embedding_flag_on_its_own_counts_as_set() {
        // `any_set` decides whether `vera setup` runs unattended and whether a
        // non-ONNX backend rejects the flags. A flag missing from it is
        // silently ignored when it is the only one passed, so each one is
        // checked alone rather than in combination.
        type FlagMutator = fn(&mut LocalEmbeddingModelFlags);
        let mutators: Vec<(&str, FlagMutator)> = vec![
            ("--code-rank-embed", |f| f.code_rank_embed = true),
            ("--embedding-repo", |f| {
                f.embedding_repo = Some("org/repo".to_string())
            }),
            ("--embedding-dir", |f| {
                f.embedding_dir = Some("/models".to_string())
            }),
            ("--embedding-onnx-file", |f| {
                f.embedding_onnx_file = Some("onnx/model.onnx".to_string())
            }),
            ("--embedding-onnx-data-file", |f| {
                f.embedding_onnx_data_file = Some("onnx/model.onnx_data".to_string())
            }),
            ("--embedding-no-onnx-data", |f| {
                f.embedding_no_onnx_data = true
            }),
            ("--embedding-tokenizer-file", |f| {
                f.embedding_tokenizer_file = Some("tokenizer.json".to_string())
            }),
            ("--embedding-dim", |f| f.embedding_dim = Some(768)),
            ("--embedding-pooling", |f| {
                f.embedding_pooling = Some("cls".to_string())
            }),
            ("--embedding-max-length", |f| {
                f.embedding_max_length = Some(512)
            }),
            ("--embedding-query-prefix", |f| {
                f.embedding_query_prefix = Some("Query:".to_string())
            }),
            ("--embedding-document-prefix", |f| {
                f.embedding_document_prefix = Some("Document:".to_string())
            }),
        ];

        assert!(!LocalEmbeddingModelFlags::default().any_set());
        for (flag, set_it) in mutators {
            let mut flags = LocalEmbeddingModelFlags::default();
            set_it(&mut flags);
            assert!(flags.any_set(), "{flag} alone was not treated as set");
        }
    }

    #[tokio::test]
    async fn ready_operation_error_wins_over_ready_signal() {
        for _ in 0..64 {
            let task = tokio::spawn(async { Err::<(), _>(anyhow::anyhow!("provider failed")) });
            tokio::task::yield_now().await;
            let error = cancel_task_on_signal(
                task,
                std::future::ready(()),
                vera_core::CancellationToken::new(),
                "test operation",
            )
            .await
            .unwrap_err();

            assert_eq!(error.to_string(), "provider failed");
        }
    }

    #[tokio::test]
    async fn cancellation_waits_for_the_operation_to_stop() {
        let cancellation = vera_core::CancellationToken::new();
        let operation_cancellation = cancellation.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();

        let task = tokio::spawn(async move {
            let _ = started_tx.send(());
            while !operation_cancellation.is_cancelled() {
                tokio::task::yield_now().await;
            }
            Err::<(), _>(anyhow::anyhow!("operation cancelled safely"))
        });
        let signal = async move {
            let _ = started_rx.await;
        };

        let error = cancel_task_on_signal(task, signal, cancellation, "test operation")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "operation cancelled safely");
    }
}
