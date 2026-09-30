//! Backend selection and backend-specific configuration adjustments.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use super::{VeraConfig, detect_gpu_info};

/// ONNX execution provider for local inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnnxExecutionProvider {
    Cpu,
    Cuda,
    Rocm,
    DirectMl,
    CoreMl,
    OpenVino,
}

impl fmt::Display for OnnxExecutionProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Cuda => write!(f, "cuda"),
            Self::Rocm => write!(f, "rocm"),
            Self::DirectMl => write!(f, "directml"),
            Self::CoreMl => write!(f, "coreml"),
            Self::OpenVino => write!(f, "openvino"),
        }
    }
}

/// Inference backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InferenceBackend {
    /// Use external OpenAI-compatible API for embeddings/reranking.
    Api,
    /// Use local ONNX models with the specified execution provider.
    OnnxJina(OnnxExecutionProvider),
    /// Use the CPU-first Potion Code static embedding model (the default local backend).
    PotionCode,
}

impl InferenceBackend {
    /// True if this backend uses local inference.
    pub fn is_local(self) -> bool {
        matches!(self, Self::OnnxJina(_) | Self::PotionCode)
    }

    /// True if this backend uses local ONNX inference.
    pub fn is_onnx(self) -> bool {
        matches!(self, Self::OnnxJina(_))
    }

    /// Get the execution provider (only for local backends).
    pub fn execution_provider(self) -> Option<OnnxExecutionProvider> {
        match self {
            Self::OnnxJina(ep) => Some(ep),
            Self::Api | Self::PotionCode => None,
        }
    }
}

impl fmt::Display for InferenceBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api => write!(f, "api"),
            Self::OnnxJina(ep) => write!(f, "onnx-jina-{ep}"),
            Self::PotionCode => write!(f, "potion-code-cpu"),
        }
    }
}

impl FromStr for InferenceBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "api" => Ok(Self::Api),
            "onnx-jina-cpu" => Ok(Self::OnnxJina(OnnxExecutionProvider::Cpu)),
            "onnx-jina-cuda" => Ok(Self::OnnxJina(OnnxExecutionProvider::Cuda)),
            "onnx-jina-rocm" => Ok(Self::OnnxJina(OnnxExecutionProvider::Rocm)),
            "onnx-jina-directml" => Ok(Self::OnnxJina(OnnxExecutionProvider::DirectMl)),
            "onnx-jina-coreml" => Ok(Self::OnnxJina(OnnxExecutionProvider::CoreMl)),
            "onnx-jina-openvino" => Ok(Self::OnnxJina(OnnxExecutionProvider::OpenVino)),
            "potion-code-cpu" | "potion-code" | "potion-cpu" => Ok(Self::PotionCode),
            other => Err(format!("unknown backend: {other}")),
        }
    }
}

/// Check if the local inference mode is active (legacy env var support).
pub fn is_local_mode() -> bool {
    std::env::var("VERA_LOCAL")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

fn backend_from_env() -> Option<InferenceBackend> {
    std::env::var("VERA_BACKEND")
        .ok()
        .and_then(|value| InferenceBackend::from_str(&value).ok())
}

impl VeraConfig {
    /// Adjust embedding parameters to match the actual backend.
    ///
    /// Saved configs may have API-mode defaults (batch 128, concurrency 8)
    /// even when the user switches to local mode. CPU inference needs small
    /// batches; GPU can handle larger ones. For GPU backends, this picks a
    /// coarse outer batch ceiling from available VRAM. The local ONNX provider
    /// still shapes the actual micro-batches from sequence length at runtime.
    pub fn adjust_for_backend(&mut self, backend: InferenceBackend) {
        match backend {
            InferenceBackend::PotionCode => {
                self.embedding.batch_size = 1024;
                self.embedding.max_concurrent_requests = 1;
                self.embedding.max_stored_dim = self.embedding.max_stored_dim.min(256);
            }
            InferenceBackend::OnnxJina(OnnxExecutionProvider::Cpu) => {
                self.embedding.batch_size = 4;
                self.embedding.max_concurrent_requests = 1;
            }
            InferenceBackend::OnnxJina(ep) => {
                self.embedding.max_concurrent_requests = 1;

                if self.embedding.low_vram {
                    self.embedding.batch_size = 1;
                    if self.embedding.gpu_mem_limit_mb == 0 {
                        self.embedding.gpu_mem_limit_mb = 1024;
                    }
                    tracing::info!(
                        "low-vram mode: batch_size=1, gpu_mem_limit={}MB",
                        self.embedding.gpu_mem_limit_mb
                    );
                    return;
                }

                let gpu_info = detect_gpu_info(ep);
                if let Some(vram) = gpu_info.vram_free_mb {
                    tracing::info!("detected GPU VRAM: {vram}MB");
                    // Auto-scale batch_size based on VRAM.
                    // Prioritize speed: use large batches when VRAM allows.
                    // A GPU reporting ~0 free MB is full or shared, so run
                    // the most conservative shape rather than trusting the
                    // reading as headroom.
                    let auto_batch = if vram < 512 {
                        1
                    } else if vram < 3072 {
                        4
                    } else if vram < 5120 {
                        16
                    } else if vram < 8192 {
                        32
                    } else if vram < 12288 {
                        64
                    } else {
                        128
                    };
                    // Unified memory is shared with macOS and apps; cap the
                    // CoreML batch so large-RAM Macs don't starve the system.
                    if ep == OnnxExecutionProvider::CoreMl {
                        self.embedding.batch_size = auto_batch.min(64);
                    } else {
                        self.embedding.batch_size = auto_batch;
                    }

                    // Set a conservative memory limit only for low-VRAM GPUs
                    // to prevent ORT from grabbing all VRAM. For >=8GB, no limit.
                    if self.embedding.gpu_mem_limit_mb == 0 && vram < 8192 {
                        // Use 80% of available VRAM, floored so a near-zero
                        // reading cannot decay into 0, which means "no ORT
                        // memory cap" everywhere else.
                        self.embedding.gpu_mem_limit_mb = ((vram as f64 * 0.8) as u64).max(128);
                        tracing::info!(
                            "auto-set gpu_mem_limit={}MB (80% of {vram}MB)",
                            self.embedding.gpu_mem_limit_mb
                        );
                    }
                } else {
                    // Could not detect VRAM; use conservative defaults.
                    // DirectML/CoreML/OpenVINO lack CLI VRAM detection,
                    // so pick a safe batch size that won't OOM on small GPUs.
                    self.embedding.batch_size = 16;
                }
            }
            InferenceBackend::Api => {}
        }
    }
}

/// Resolve the effective inference backend from a CLI flag or environment.
pub fn resolve_backend(backend: Option<InferenceBackend>) -> InferenceBackend {
    if let Some(b) = backend {
        return b;
    }
    if let Some(b) = backend_from_env() {
        return b;
    }
    // Legacy: VERA_LOCAL=1 maps to onnx-jina-cpu
    if is_local_mode() {
        return InferenceBackend::OnnxJina(OnnxExecutionProvider::Cpu);
    }
    InferenceBackend::Api
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::run_env_test;

    #[test]
    fn openvino_backend_round_trip() {
        let backend = InferenceBackend::from_str("onnx-jina-openvino").unwrap();
        assert_eq!(
            backend,
            InferenceBackend::OnnxJina(OnnxExecutionProvider::OpenVino)
        );
        assert_eq!(backend.to_string(), "onnx-jina-openvino");
        assert!(backend.is_local());
    }

    #[test]
    fn potion_code_backend_round_trip() {
        let backend = InferenceBackend::from_str("potion-code-cpu").unwrap();
        assert_eq!(backend, InferenceBackend::PotionCode);
        assert_eq!(backend.to_string(), "potion-code-cpu");
        assert!(backend.is_local());
        assert!(!backend.is_onnx());
        assert_eq!(backend.execution_provider(), None);
    }

    #[test]
    fn resolve_backend_prefers_saved_backend_env() {
        run_env_test(
            "config::backend::tests::resolve_backend_prefers_saved_backend_env_probe",
            &[
                ("VERA_BACKEND", Some("onnx-jina-cuda")),
                ("VERA_LOCAL", Some("1")),
            ],
        );
    }

    #[test]
    #[ignore = "driven by resolve_backend_prefers_saved_backend_env"]
    fn resolve_backend_prefers_saved_backend_env_probe() {
        assert_eq!(
            resolve_backend(None),
            InferenceBackend::OnnxJina(OnnxExecutionProvider::Cuda)
        );
    }

    #[test]
    fn resolve_backend_falls_back_to_legacy_local_env() {
        run_env_test(
            "config::backend::tests::resolve_backend_falls_back_to_legacy_local_env_probe",
            &[("VERA_BACKEND", None), ("VERA_LOCAL", Some("1"))],
        );
    }

    #[test]
    #[ignore = "driven by resolve_backend_falls_back_to_legacy_local_env"]
    fn resolve_backend_falls_back_to_legacy_local_env_probe() {
        assert_eq!(
            resolve_backend(None),
            InferenceBackend::OnnxJina(OnnxExecutionProvider::Cpu)
        );
    }
}
