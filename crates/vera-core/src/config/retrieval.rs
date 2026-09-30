//! Retrieval, ranking, and reranker configuration.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use super::{env_bool, env_f64, env_usize};

/// Reranker wire protocol / capability selection.
///
/// `Generic` covers SiliconFlow, Jina, Cohere and other OpenAI-style
/// `/rerank` endpoints (`top_n` + `results`). `Voyage` covers Voyage AI
/// (`top_k` + `data`). Explicit selection overrides hostname auto-detection;
/// `None` (the default) preserves auto-detection for backward compatibility:
/// a Voyage hostname maps to `Voyage`, everything else to `Generic`.
/// Custom proxies can select either protocol without hostname spoofing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RerankerProtocol {
    Generic,
    Voyage,
}

impl FromStr for RerankerProtocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "generic" => Ok(Self::Generic),
            "voyage" => Ok(Self::Voyage),
            other => Err(format!("unknown reranker protocol: {other}")),
        }
    }
}

impl fmt::Display for RerankerProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generic => write!(f, "generic"),
            Self::Voyage => write!(f, "voyage"),
        }
    }
}

/// Configuration for the retrieval pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    /// Number of results to return by default.
    pub default_limit: usize,
    /// RRF fusion constant (k in 1/(k + rank)).
    pub rrf_k: f64,
    /// Number of candidates to pass to the reranker.
    pub rerank_candidates: usize,
    /// Whether to enable reranking (requires API credentials).
    pub reranking_enabled: bool,
    /// Maximum documents per reranker API call. Larger candidate sets are
    /// partitioned into batches and scores merged. 0 means no batching.
    #[serde(default = "default_max_rerank_batch")]
    pub max_rerank_batch: usize,
    /// Total character budget for search output. Results are progressively
    /// truncated so the combined output stays within this limit.
    /// 0 means unlimited.
    #[serde(default = "default_max_output_chars")]
    pub max_output_chars: usize,
    /// Explicit reranker wire protocol. `None` preserves hostname
    /// auto-detection (Voyage hostname → Voyage, else Generic).
    #[serde(default)]
    pub reranker_protocol: Option<RerankerProtocol>,
    /// Explicit reranker endpoint path override. `None` keeps the default
    /// `{base}/rerank`. When `Some`, the value is used verbatim (leading
    /// `/` required; no extra `/rerank` appended).
    #[serde(default)]
    pub reranker_endpoint_path: Option<String>,
    /// Optional reranker task instruction (scoring guidance, separate from
    /// Vera `--intent`). Sent only when the selected protocol supports it
    /// or an explicit wire field is configured.
    #[serde(default)]
    pub reranker_task_instruction: Option<String>,
    /// Explicit wire field name for the task instruction. When `Some`, the
    /// instruction is serialized under this field regardless of protocol
    /// capability; when `None`, the protocol's default field is used if
    /// supported, otherwise the instruction is omitted.
    #[serde(default)]
    pub reranker_task_field: Option<String>,
    /// Per-document character budget for reranker input. 4800 default,
    /// newline-safe truncation; 0 means unlimited (no truncation).
    #[serde(default = "default_reranker_max_doc_chars")]
    pub reranker_max_doc_chars: usize,
    /// Reranker request timeout in seconds. 30s default.
    #[serde(default = "default_reranker_timeout_secs")]
    pub reranker_timeout_secs: u64,
    /// Reranker max retries on transient errors. 2 default.
    #[serde(default = "default_reranker_max_retries")]
    pub reranker_max_retries: u32,
    /// Cap on 429 rate-limit wait in seconds. `None` (default) keeps the
    /// short generic backoff; `Some(n)` sleeps until the quota window reset
    /// clamped to `n` seconds. `0` is treated as `None` (CLI `0` maps to
    /// `null`, file `0` likewise means no cap) so the three layers share one
    /// contract.
    #[serde(
        default = "default_reranker_rate_limit_wait_secs",
        deserialize_with = "deserialize_reranker_rate_limit_wait_secs"
    )]
    pub reranker_rate_limit_wait_secs: Option<u64>,
    /// How `return_documents` is sent. `None` omits the field (per-protocol
    /// default is `Some(false)` for current providers). `Some(v)` sends that
    /// boolean verbatim.
    #[serde(default = "default_reranker_return_documents")]
    pub reranker_return_documents: Option<bool>,
    // ── Issue #196 ranking signals (individually toggleable for ablations) ──
    /// Filename-stem keyword boost for natural-language queries.
    ///
    /// When enabled, files whose stem or parent directory matches query keywords
    /// receive a pool-relative boost. Mechanism: file names are human-chosen
    /// module labels; matching them indicates the file implements the queried
    /// concept (e.g. `session.rs` for "session renewal"). This addresses
    /// recall-oriented misses where keyword-dense docs/tests outrank the correct
    /// source file.
    #[serde(default = "default_ranking_filename_stem_boost")]
    pub ranking_filename_stem_boost: bool,
    /// Minimum match ratio for the filename-stem boost (issue #196 gating knob).
    ///
    /// Controls the firing threshold for the filename-stem boost: a file must
    /// match at least this fraction of the query's keywords (ratio >= threshold,
    /// inclusive) to receive the boost. Mechanism: raising the threshold from
    /// the historical 0.05 filters single-token coincidences (e.g. a repo
    /// literally named `json` matching one generic word) while preserving
    /// genuine multi-keyword file-name signals.
    /// Default 0.05 (env `VERA_RANKING_FILENAME_STEM_MIN_RATIO` authoritative).
    #[serde(default = "default_ranking_filename_stem_min_ratio")]
    pub ranking_filename_stem_min_ratio: f64,
    /// Skip the filename-stem boost for symbol queries (issue #196 gating knob).
    ///
    /// When enabled, suppresses the filename-stem boost whenever the exact-
    /// identifier machinery is engaged (the query carries embedded symbols or
    /// an exact identifier). Mechanism: symbol lookups already route through
    /// exact-identifier, exact-filename and content-symbol boosts; the
    /// filename-keyword inference double-counts there and hurts the independent
    /// set. A NaturalLanguage query without an exact identifier still gets the
    /// boost when this knob is on.
    /// Default false (env `VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES` authoritative).
    #[serde(default = "default_ranking_filename_stem_skip_symbol_queries")]
    pub ranking_filename_stem_skip_symbol_queries: bool,
    /// Definition-content boost for natural-language queries.
    ///
    /// When enabled, chunks whose content defines a queried concept (e.g.
    /// `class Session`) and definition chunks whose symbol overlaps query
    /// keywords are boosted. Mechanism: definitions are canonical anchors for a
    /// concept; developers asking about a concept usually want the definition
    /// site, not incidental mentions in docs or fixtures.
    #[serde(default = "default_ranking_definition_boost")]
    pub ranking_definition_boost: bool,
    /// Recall-oriented candidate-pool expansion for natural-language queries.
    ///
    /// When enabled, broad NL queries (≥4 words) and structural queries expand
    /// the fetch limit (up to 8×) so low-ranking but correct files enter the
    /// ranking stage. Mechanism: intent and cross-file queries are open-ended;
    /// a tight pool prematurely prunes the answer before ranking signals can
    /// promote it.
    #[serde(default = "default_ranking_recall_pool_expansion")]
    pub ranking_recall_pool_expansion: bool,
    /// Filter-during-scan optimization for filtered flat-vector queries.
    ///
    /// When enabled, filtered queries on the flat backend avoid hydrating the whole index:
    /// a lazy per-store eligibility map (chunk row -> path-id, language) is built once per store+generation,
    /// distinct paths are tested against glob filters via `GlobMatcher`, and the flat SIMD scan collects top-K only among eligible rows.
    /// Scope gate: only path globs, exact paths, and language filters are eligible; other dimensions fall back to whole-index fetch.
    /// Default ON (1) since the r5 evidence-backed flip (issue #197); env override `VERA_VECTOR_FILTER_DURING_SCAN` authoritative.
    #[serde(default = "default_vector_filter_during_scan")]
    pub vector_filter_during_scan: bool,
}

fn default_max_output_chars() -> usize {
    env_usize("VERA_MAX_OUTPUT_CHARS", 0)
}

fn default_max_rerank_batch() -> usize {
    env_usize("VERA_MAX_RERANK_BATCH", 20)
}

fn default_reranker_max_doc_chars() -> usize {
    env_usize("VERA_MAX_RERANK_DOC_CHARS", 4800)
}

fn default_reranker_timeout_secs() -> u64 {
    env_usize("VERA_RERANK_TIMEOUT_SECS", 30) as u64
}

fn default_reranker_max_retries() -> u32 {
    env_usize("VERA_RERANK_MAX_RETRIES", 2) as u32
}

fn default_reranker_rate_limit_wait_secs() -> Option<u64> {
    match std::env::var("VERA_RERANK_RATE_LIMIT_WAIT_SECS") {
        Ok(value) => match value.parse::<u64>() {
            Ok(parsed) => {
                if parsed == 0 {
                    None
                } else {
                    Some(parsed)
                }
            }
            Err(error) => {
                tracing::warn!(
                    key = "VERA_RERANK_RATE_LIMIT_WAIT_SECS",
                    value = %value,
                    error = %error,
                    "invalid numeric environment override; using default"
                );
                None
            }
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => {
            tracing::warn!(
                key = "VERA_RERANK_RATE_LIMIT_WAIT_SECS",
                error = %error,
                "could not read numeric environment override; using default"
            );
            None
        }
    }
}

fn deserialize_reranker_rate_limit_wait_secs<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<u64>::deserialize(deserializer)?;
    Ok(opt.filter(|v| *v != 0))
}

fn default_reranker_return_documents() -> Option<bool> {
    // No env override for this; keep `Some(false)` as the compatible default
    // so generic endpoints see today's wire shape. Users can set `None`
    // (via `null` in JSON or config set) to omit the field per capability.
    Some(false)
}

fn default_ranking_filename_stem_boost() -> bool {
    env_bool("VERA_RANKING_FILENAME_STEM_BOOST", true)
}

fn default_ranking_filename_stem_min_ratio() -> f64 {
    // Default 0.05 preserves pre-knob behavior. Env authoritative.
    env_f64("VERA_RANKING_FILENAME_STEM_MIN_RATIO", 0.05)
}

fn default_ranking_filename_stem_skip_symbol_queries() -> bool {
    // Default false preserves pre-knob behavior. Env authoritative.
    env_bool("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES", false)
}

fn default_ranking_definition_boost() -> bool {
    env_bool("VERA_RANKING_DEFINITION_BOOST", true)
}

fn default_ranking_recall_pool_expansion() -> bool {
    env_bool("VERA_RANKING_RECALL_POOL_EXPANSION", true)
}

fn default_vector_filter_during_scan() -> bool {
    // Default ON since the r5 evidence-backed flip (issue #197). The r5
    // preregistered decision round at 98e6e50 (3 flag-on + 3 flag-off
    // full-suite runs) passed all three gates: mechanism control (p50 delta
    // 5.651 ms > 0.6, p95 delta 74.090 ms > 5), same-head nDCG parity
    // (|delta| 0.0000049 <= 0.001), and absolute latency acceptance (p50
    // 6.353 ms <= 7.38, p95 65.034 ms <= 65.88 vs the 072c725 9800X3D
    // baseline 7.879/60.880). Evidence: docs/adr/008-filter-during-scan-default.md.
    env_bool("VERA_VECTOR_FILTER_DURING_SCAN", true)
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            default_limit: 5,
            rrf_k: 60.0,
            rerank_candidates: 50,
            reranking_enabled: false,
            max_rerank_batch: default_max_rerank_batch(),
            max_output_chars: default_max_output_chars(),
            reranker_protocol: None,
            reranker_endpoint_path: None,
            reranker_task_instruction: None,
            reranker_task_field: None,
            reranker_max_doc_chars: default_reranker_max_doc_chars(),
            reranker_timeout_secs: default_reranker_timeout_secs(),
            reranker_max_retries: default_reranker_max_retries(),
            reranker_rate_limit_wait_secs: default_reranker_rate_limit_wait_secs(),
            reranker_return_documents: default_reranker_return_documents(),
            ranking_filename_stem_boost: default_ranking_filename_stem_boost(),
            ranking_filename_stem_min_ratio: default_ranking_filename_stem_min_ratio(),
            ranking_filename_stem_skip_symbol_queries:
                default_ranking_filename_stem_skip_symbol_queries(),
            ranking_definition_boost: default_ranking_definition_boost(),
            ranking_recall_pool_expansion: default_ranking_recall_pool_expansion(),
            vector_filter_during_scan: default_vector_filter_during_scan(),
        }
    }
}

impl RetrievalConfig {
    /// Filename-stem boost enabled, with env-var override for cheap ablations.
    pub fn ranking_filename_stem_boost_enabled(&self) -> bool {
        if std::env::var("VERA_RANKING_FILENAME_STEM_BOOST").is_ok() {
            env_bool(
                "VERA_RANKING_FILENAME_STEM_BOOST",
                self.ranking_filename_stem_boost,
            )
        } else {
            self.ranking_filename_stem_boost
        }
    }

    /// Minimum ratio for filename-stem boost, with env-var override.
    /// Default 0.05; env `VERA_RANKING_FILENAME_STEM_MIN_RATIO` authoritative.
    pub fn ranking_filename_stem_min_ratio_effective(&self) -> f64 {
        env_f64(
            "VERA_RANKING_FILENAME_STEM_MIN_RATIO",
            self.ranking_filename_stem_min_ratio,
        )
    }

    /// Whether to skip filename-stem boost for symbol queries, with env-var override.
    /// Default false; env `VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES` authoritative.
    pub fn ranking_filename_stem_skip_symbol_queries_enabled(&self) -> bool {
        env_bool(
            "VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES",
            self.ranking_filename_stem_skip_symbol_queries,
        )
    }

    /// Definition-content boost enabled, with env-var override.
    pub fn ranking_definition_boost_enabled(&self) -> bool {
        if std::env::var("VERA_RANKING_DEFINITION_BOOST").is_ok() {
            env_bool(
                "VERA_RANKING_DEFINITION_BOOST",
                self.ranking_definition_boost,
            )
        } else {
            self.ranking_definition_boost
        }
    }

    /// Recall-pool expansion enabled, with env-var override.
    pub fn ranking_recall_pool_expansion_enabled(&self) -> bool {
        if std::env::var("VERA_RANKING_RECALL_POOL_EXPANSION").is_ok() {
            env_bool(
                "VERA_RANKING_RECALL_POOL_EXPANSION",
                self.ranking_recall_pool_expansion,
            )
        } else {
            self.ranking_recall_pool_expansion
        }
    }

    /// Filter-during-scan optimization enabled, with env-var override.
    /// Default ON since the r5 evidence-backed flip (issue #197);
    /// env `VERA_VECTOR_FILTER_DURING_SCAN` authoritative.
    pub fn vector_filter_during_scan_enabled(&self) -> bool {
        env_bool(
            "VERA_VECTOR_FILTER_DURING_SCAN",
            self.vector_filter_during_scan,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VeraConfig;
    use crate::test_env::run_env_test;

    #[test]
    fn reranker_protocol_parses_case_insensitively() {
        assert_eq!(
            "generic".parse::<RerankerProtocol>().unwrap(),
            RerankerProtocol::Generic
        );
        assert_eq!(
            "VOYAGE".parse::<RerankerProtocol>().unwrap(),
            RerankerProtocol::Voyage
        );
        assert!("unknown".parse::<RerankerProtocol>().is_err());
        assert_eq!(RerankerProtocol::Generic.to_string(), "generic");
        assert_eq!(RerankerProtocol::Voyage.to_string(), "voyage");
    }

    #[test]
    fn retrieval_config_serialization_round_trips_all_reranker_keys() {
        let cfg = RetrievalConfig {
            reranker_protocol: Some(RerankerProtocol::Voyage),
            reranker_endpoint_path: Some("/v1/reranking".to_string()),
            reranker_task_instruction: Some("rank by relevance".to_string()),
            reranker_task_field: Some("instruction".to_string()),
            reranker_max_doc_chars: 1234,
            reranker_timeout_secs: 42,
            reranker_max_retries: 5,
            reranker_rate_limit_wait_secs: Some(15),
            reranker_return_documents: Some(true),
            max_rerank_batch: 8,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RetrievalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.reranker_protocol, cfg.reranker_protocol);
        assert_eq!(back.reranker_endpoint_path, cfg.reranker_endpoint_path);
        assert_eq!(
            back.reranker_task_instruction,
            cfg.reranker_task_instruction
        );
        assert_eq!(back.reranker_task_field, cfg.reranker_task_field);
        assert_eq!(back.reranker_max_doc_chars, cfg.reranker_max_doc_chars);
        assert_eq!(back.reranker_timeout_secs, cfg.reranker_timeout_secs);
        assert_eq!(back.reranker_max_retries, cfg.reranker_max_retries);
        assert_eq!(
            back.reranker_rate_limit_wait_secs,
            cfg.reranker_rate_limit_wait_secs
        );
        assert_eq!(
            back.reranker_return_documents,
            cfg.reranker_return_documents
        );
        assert_eq!(back.max_rerank_batch, cfg.max_rerank_batch);
    }

    #[test]
    fn legacy_retrieval_config_deserializes_with_today_defaults() {
        // Minimal JSON from before the refactor (no new reranker keys)
        let legacy = r#"{
            "default_limit": 5,
            "rrf_k": 60.0,
            "rerank_candidates": 50,
            "reranking_enabled": false,
            "max_rerank_batch": 20,
            "max_output_chars": 0
        }"#;
        let cfg: RetrievalConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(cfg.max_rerank_batch, 20);
        assert_eq!(cfg.reranker_max_doc_chars, 4800);
        assert_eq!(cfg.reranker_timeout_secs, 30);
        assert_eq!(cfg.reranker_max_retries, 2);
        assert_eq!(cfg.reranker_rate_limit_wait_secs, None);
        assert_eq!(cfg.reranker_protocol, None);
        assert_eq!(cfg.reranker_task_instruction, None);
        assert_eq!(cfg.reranker_task_field, None);
        assert_eq!(cfg.reranker_endpoint_path, None);
        assert_eq!(cfg.reranker_return_documents, Some(false));

        // VeraConfig wrapper also tolerates missing `core_config.reranker_*`
        let vera_legacy = r#"{"indexing":{"max_chunk_lines":200,"default_excludes":[],"max_file_size_bytes":1000000,"max_chunk_bytes":24576},"retrieval":{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0},"embedding":{"batch_size":128,"max_concurrent_requests":8,"timeout_secs":60,"max_retries":3,"max_stored_dim":1024}}"#;
        let vera: VeraConfig = serde_json::from_str(vera_legacy).unwrap();
        assert_eq!(vera.retrieval.reranker_max_doc_chars, 4800);
        assert_eq!(vera.retrieval.reranker_protocol, None);
    }

    #[test]
    fn reranker_doc_budget_env_precedence_matrix() {
        // env-only, env+config (config wins), unset — for VERA_MAX_RERANK_DOC_CHARS
        run_env_test(
            "config::retrieval::tests::reranker_doc_budget_env_precedence_matrix_probe",
            &[
                ("VERA_MAX_RERANK_DOC_CHARS", Some("9999")),
                ("VERA_MAX_RERANK_BATCH", None),
                ("VERA_RERANK_RATE_LIMIT_WAIT_SECS", None),
                ("VERA_RERANK_TIMEOUT_SECS", None),
                ("VERA_RERANK_MAX_RETRIES", None),
            ],
        );
        run_env_test(
            "config::retrieval::tests::reranker_doc_budget_config_wins_over_env_probe",
            &[("VERA_MAX_RERANK_DOC_CHARS", Some("9999"))],
        );
        run_env_test(
            "config::retrieval::tests::reranker_doc_budget_unset_defaults_probe",
            &[("VERA_MAX_RERANK_DOC_CHARS", None)],
        );
    }

    #[test]
    #[ignore = "driven by reranker_doc_budget_env_precedence_matrix"]
    fn reranker_doc_budget_env_precedence_matrix_probe() {
        // env-only: no config key, env present => env value observed
        assert_eq!(default_reranker_max_doc_chars(), 9999);
        let cfg = RetrievalConfig::default();
        assert_eq!(cfg.reranker_max_doc_chars, 9999);
    }

    #[test]
    #[ignore = "driven by reranker_doc_budget_env_precedence_matrix"]
    fn reranker_doc_budget_config_wins_over_env_probe() {
        // env + explicit config JSON: file value must win (config authoritative)
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"reranker_max_doc_chars":1200}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            cfg.reranker_max_doc_chars, 1200,
            "config file must win over env 9999"
        );
        // Also verify RetrievalConfig::default still sees env (covered by other probe)
    }

    #[test]
    #[ignore = "driven by reranker_doc_budget_env_precedence_matrix"]
    fn reranker_doc_budget_unset_defaults_probe() {
        assert!(std::env::var("VERA_MAX_RERANK_DOC_CHARS").is_err());
        assert_eq!(default_reranker_max_doc_chars(), 4800);
        assert_eq!(RetrievalConfig::default().reranker_max_doc_chars, 4800);
    }

    #[test]
    fn reranker_rate_limit_env_precedence_matrix() {
        run_env_test(
            "config::retrieval::tests::reranker_rate_limit_env_precedence_probe",
            &[("VERA_RERANK_RATE_LIMIT_WAIT_SECS", Some("42"))],
        );
        run_env_test(
            "config::retrieval::tests::reranker_rate_limit_config_wins_probe",
            &[("VERA_RERANK_RATE_LIMIT_WAIT_SECS", Some("99"))],
        );
        run_env_test(
            "config::retrieval::tests::reranker_rate_limit_unset_probe",
            &[("VERA_RERANK_RATE_LIMIT_WAIT_SECS", None)],
        );
    }

    #[test]
    #[ignore = "driven by reranker_rate_limit_env_precedence_matrix"]
    fn reranker_rate_limit_env_precedence_probe() {
        assert_eq!(default_reranker_rate_limit_wait_secs(), Some(42));
        assert_eq!(
            RetrievalConfig::default().reranker_rate_limit_wait_secs,
            Some(42)
        );
    }

    #[test]
    #[ignore = "driven by reranker_rate_limit_env_precedence_matrix"]
    fn reranker_rate_limit_config_wins_probe() {
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"reranker_rate_limit_wait_secs":10}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.reranker_rate_limit_wait_secs, Some(10));
        // 0 in config becomes None (explicit unlimited/short), env ignored
        let json_zero = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"reranker_rate_limit_wait_secs":null}"#;
        let cfg2: RetrievalConfig = serde_json::from_str(json_zero).unwrap();
        assert_eq!(cfg2.reranker_rate_limit_wait_secs, None);
    }

    #[test]
    #[ignore = "driven by reranker_rate_limit_env_precedence_matrix"]
    fn reranker_rate_limit_unset_probe() {
        assert!(std::env::var("VERA_RERANK_RATE_LIMIT_WAIT_SECS").is_err());
        assert_eq!(default_reranker_rate_limit_wait_secs(), None);
        assert_eq!(
            RetrievalConfig::default().reranker_rate_limit_wait_secs,
            None
        );
    }

    #[test]
    fn reranker_batch_env_precedence_matrix() {
        // Batch is the pinned case: config authoritative on BOTH dynamic and static paths (aae94f7)
        run_env_test(
            "config::retrieval::tests::reranker_batch_env_only_probe",
            &[("VERA_MAX_RERANK_BATCH", Some("7"))],
        );
        run_env_test(
            "config::retrieval::tests::reranker_batch_config_authoritative_probe",
            &[("VERA_MAX_RERANK_BATCH", Some("99"))],
        );
        run_env_test(
            "config::retrieval::tests::reranker_batch_unset_probe",
            &[("VERA_MAX_RERANK_BATCH", None)],
        );
    }

    #[test]
    #[ignore = "driven by reranker_batch_env_precedence_matrix"]
    fn reranker_batch_env_only_probe() {
        assert_eq!(default_max_rerank_batch(), 7);
        assert_eq!(RetrievalConfig::default().max_rerank_batch, 7);
    }

    #[test]
    #[ignore = "driven by reranker_batch_env_precedence_matrix"]
    fn reranker_batch_config_authoritative_probe() {
        // Even with env=99, explicit JSON 8 must win (config authoritative)
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":8,"max_output_chars":0}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.max_rerank_batch, 8);
        // Dynamic path: ApiReranker::from_configs must use retrieval's 8, not env 99
        let rcfg = crate::retrieval::reranker::RerankerConfig::new(
            "http://example.com".to_string(),
            "m".to_string(),
            "k".to_string(),
        );
        let r = crate::retrieval::reranker::ApiReranker::from_configs(rcfg, &cfg).unwrap();
        assert_eq!(
            r.max_rerank_batch, 8,
            "dynamic path: config 8 must win over env 99"
        );
        // Static legacy path still honors env when no explicit retrieval value is passed
        // (covered by env_only probe); from_configs is the authoritative path.
    }

    #[test]
    #[ignore = "driven by reranker_batch_env_precedence_matrix"]
    fn reranker_batch_unset_probe() {
        assert!(std::env::var("VERA_MAX_RERANK_BATCH").is_err());
        assert_eq!(default_max_rerank_batch(), 20);
        assert_eq!(RetrievalConfig::default().max_rerank_batch, 20);
    }

    #[test]
    fn legacy_retrieval_config_new_flags_default_to_true() {
        let legacy = r#"{
            "default_limit": 5,
            "rrf_k": 60.0,
            "rerank_candidates": 50,
            "reranking_enabled": false,
            "max_rerank_batch": 20,
            "max_output_chars": 0
        }"#;
        let cfg: RetrievalConfig = serde_json::from_str(legacy).unwrap();
        assert!(
            cfg.ranking_filename_stem_boost,
            "missing flag must default true for backward compat"
        );
        assert!(
            cfg.ranking_definition_boost,
            "missing flag must default true"
        );
        assert!(
            cfg.ranking_recall_pool_expansion,
            "missing flag must default true"
        );
        assert!(cfg.ranking_filename_stem_boost_enabled());
        assert!(cfg.ranking_definition_boost_enabled());
        assert!(cfg.ranking_recall_pool_expansion_enabled());
    }

    #[test]
    fn retrieval_config_new_flags_round_trip() {
        let cfg = RetrievalConfig {
            ranking_filename_stem_boost: false,
            ranking_definition_boost: false,
            ranking_recall_pool_expansion: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RetrievalConfig = serde_json::from_str(&json).unwrap();
        assert!(!back.ranking_filename_stem_boost);
        assert!(!back.ranking_definition_boost);
        assert!(!back.ranking_recall_pool_expansion);
        assert!(!back.ranking_filename_stem_boost_enabled());
        assert!(!back.ranking_definition_boost_enabled());
        assert!(!back.ranking_recall_pool_expansion_enabled());

        // true round-trip
        let cfg2 = RetrievalConfig {
            ranking_filename_stem_boost: true,
            ..Default::default()
        };
        let json2 = serde_json::to_string(&cfg2).unwrap();
        let back2: RetrievalConfig = serde_json::from_str(&json2).unwrap();
        assert!(back2.ranking_filename_stem_boost);
        assert!(back2.ranking_filename_stem_boost_enabled());
    }

    #[test]
    fn ranking_filename_stem_boost_env_precedence_matrix() {
        run_env_test(
            "config::retrieval::tests::ranking_filename_stem_boost_env_only_probe",
            &[("VERA_RANKING_FILENAME_STEM_BOOST", Some("0"))],
        );
        run_env_test(
            "config::retrieval::tests::ranking_filename_stem_boost_env_true_overrides_config_false_probe",
            &[("VERA_RANKING_FILENAME_STEM_BOOST", Some("1"))],
        );
        run_env_test(
            "config::retrieval::tests::ranking_filename_stem_boost_unset_defaults_probe",
            &[("VERA_RANKING_FILENAME_STEM_BOOST", None)],
        );
    }

    #[test]
    #[ignore = "driven by ranking_filename_stem_boost_env_precedence_matrix"]
    fn ranking_filename_stem_boost_env_only_probe() {
        // env=0 overrides default true
        assert!(!default_ranking_filename_stem_boost());
        let cfg = RetrievalConfig::default();
        assert!(!cfg.ranking_filename_stem_boost_enabled());
    }

    #[test]
    #[ignore = "driven by ranking_filename_stem_boost_env_precedence_matrix"]
    fn ranking_filename_stem_boost_env_true_overrides_config_false_probe() {
        // config file says false but env=1 must win (env authoritative for these flags)
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"ranking_filename_stem_boost":false}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!(
            cfg.ranking_filename_stem_boost_enabled(),
            "env 1 must override config false"
        );
    }

    #[test]
    #[ignore = "driven by ranking_filename_stem_boost_env_precedence_matrix"]
    fn ranking_filename_stem_boost_unset_defaults_probe() {
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_BOOST").is_err());
        assert!(default_ranking_filename_stem_boost());
        assert!(RetrievalConfig::default().ranking_filename_stem_boost_enabled());
    }

    #[test]
    fn ranking_definition_boost_env_precedence_matrix() {
        run_env_test(
            "config::retrieval::tests::ranking_definition_boost_env_only_probe",
            &[("VERA_RANKING_DEFINITION_BOOST", Some("false"))],
        );
        run_env_test(
            "config::retrieval::tests::ranking_definition_boost_env_true_overrides_config_false_probe",
            &[("VERA_RANKING_DEFINITION_BOOST", Some("true"))],
        );
        run_env_test(
            "config::retrieval::tests::ranking_definition_boost_unset_defaults_probe",
            &[("VERA_RANKING_DEFINITION_BOOST", None)],
        );
    }

    #[test]
    #[ignore = "driven by ranking_definition_boost_env_precedence_matrix"]
    fn ranking_definition_boost_env_only_probe() {
        assert!(!default_ranking_definition_boost());
        assert!(!RetrievalConfig::default().ranking_definition_boost_enabled());
    }

    #[test]
    #[ignore = "driven by ranking_definition_boost_env_precedence_matrix"]
    fn ranking_definition_boost_env_true_overrides_config_false_probe() {
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"ranking_definition_boost":false}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!(
            cfg.ranking_definition_boost_enabled(),
            "env true must override config false"
        );
    }

    #[test]
    #[ignore = "driven by ranking_definition_boost_env_precedence_matrix"]
    fn ranking_definition_boost_unset_defaults_probe() {
        assert!(std::env::var("VERA_RANKING_DEFINITION_BOOST").is_err());
        assert!(default_ranking_definition_boost());
        assert!(RetrievalConfig::default().ranking_definition_boost_enabled());
    }

    #[test]
    fn ranking_recall_pool_expansion_env_precedence_matrix() {
        run_env_test(
            "config::retrieval::tests::ranking_recall_pool_expansion_env_only_probe",
            &[("VERA_RANKING_RECALL_POOL_EXPANSION", Some("0"))],
        );
        run_env_test(
            "config::retrieval::tests::ranking_recall_pool_expansion_env_true_overrides_config_false_probe",
            &[("VERA_RANKING_RECALL_POOL_EXPANSION", Some("1"))],
        );
        run_env_test(
            "config::retrieval::tests::ranking_recall_pool_expansion_unset_defaults_probe",
            &[("VERA_RANKING_RECALL_POOL_EXPANSION", None)],
        );
    }

    #[test]
    #[ignore = "driven by ranking_recall_pool_expansion_env_precedence_matrix"]
    fn ranking_recall_pool_expansion_env_only_probe() {
        assert!(!default_ranking_recall_pool_expansion());
        assert!(!RetrievalConfig::default().ranking_recall_pool_expansion_enabled());
    }

    #[test]
    #[ignore = "driven by ranking_recall_pool_expansion_env_precedence_matrix"]
    fn ranking_recall_pool_expansion_env_true_overrides_config_false_probe() {
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"ranking_recall_pool_expansion":false}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!(
            cfg.ranking_recall_pool_expansion_enabled(),
            "env 1 must override config false"
        );
    }

    #[test]
    #[ignore = "driven by ranking_recall_pool_expansion_env_precedence_matrix"]
    fn ranking_recall_pool_expansion_unset_defaults_probe() {
        assert!(std::env::var("VERA_RANKING_RECALL_POOL_EXPANSION").is_err());
        assert!(default_ranking_recall_pool_expansion());
        assert!(RetrievalConfig::default().ranking_recall_pool_expansion_enabled());
    }

    // ── Filter-during-scan knob (#197) ──
    #[test]
    fn vector_filter_default_on() {
        run_env_test(
            "config::retrieval::tests::vector_filter_default_on_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", None)],
        );
    }

    #[test]
    #[ignore = "driven by vector_filter_default_on"]
    fn vector_filter_default_on_probe() {
        assert!(std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").is_err());
        assert!(default_vector_filter_during_scan());
        assert!(RetrievalConfig::default().vector_filter_during_scan);
        assert!(
            RetrievalConfig::default().vector_filter_during_scan_enabled(),
            "default must be ON"
        );
        assert!(
            VeraConfig::default()
                .retrieval
                .vector_filter_during_scan_enabled(),
            "VeraConfig default must be ON"
        );
        // Legacy JSON without field defaults to true (r5-flipped default).
        let legacy = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0}"#;
        let rcfg: RetrievalConfig = serde_json::from_str(legacy).unwrap();
        assert!(rcfg.vector_filter_during_scan);
        assert!(rcfg.vector_filter_during_scan_enabled());
    }

    #[test]
    fn vector_filter_env_override() {
        run_env_test(
            "config::retrieval::tests::vector_filter_env_on_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", Some("1"))],
        );
        run_env_test(
            "config::retrieval::tests::vector_filter_env_off_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", Some("0"))],
        );
        run_env_test(
            "config::retrieval::tests::vector_filter_env_true_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", Some("true"))],
        );
        run_env_test(
            "config::retrieval::tests::vector_filter_env_override_config_false_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", Some("1"))],
        );
    }

    #[test]
    #[ignore = "driven by vector_filter_env_override"]
    fn vector_filter_env_on_probe() {
        assert_eq!(
            std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").unwrap(),
            "1"
        );
        assert!(default_vector_filter_during_scan());
        assert!(RetrievalConfig::default().vector_filter_during_scan_enabled());
    }

    #[test]
    #[ignore = "driven by vector_filter_env_override"]
    fn vector_filter_env_off_probe() {
        assert_eq!(
            std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").unwrap(),
            "0"
        );
        assert!(!default_vector_filter_during_scan());
        // Even if file says true, env 0 must win.
        let cfg = RetrievalConfig {
            vector_filter_during_scan: true,
            ..Default::default()
        };
        assert!(
            !cfg.vector_filter_during_scan_enabled(),
            "env 0 must override file true"
        );
    }

    #[test]
    #[ignore = "driven by vector_filter_env_override"]
    fn vector_filter_env_true_probe() {
        assert_eq!(
            std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").unwrap(),
            "true"
        );
        assert!(default_vector_filter_during_scan());
        let cfg = RetrievalConfig {
            vector_filter_during_scan: false,
            ..Default::default()
        };
        assert!(
            cfg.vector_filter_during_scan_enabled(),
            "env true must override file false"
        );
    }

    #[test]
    #[ignore = "driven by vector_filter_env_override"]
    fn vector_filter_env_override_config_false_probe() {
        assert_eq!(
            std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").unwrap(),
            "1"
        );
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"vector_filter_during_scan":false}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!(
            cfg.vector_filter_during_scan_enabled(),
            "env 1 must override config false"
        );
    }

    #[test]
    fn vector_filter_config_precedence() {
        run_env_test(
            "config::retrieval::tests::vector_filter_config_file_true_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", None)],
        );
        run_env_test(
            "config::retrieval::tests::vector_filter_env_beats_file_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", Some("0"))],
        );
    }

    #[test]
    #[ignore = "driven by vector_filter_config_precedence"]
    fn vector_filter_config_file_true_probe() {
        assert!(std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").is_err());
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"vector_filter_during_scan":true}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.vector_filter_during_scan);
        assert!(cfg.vector_filter_during_scan_enabled());
    }

    #[test]
    #[ignore = "driven by vector_filter_config_precedence"]
    fn vector_filter_env_beats_file_probe() {
        assert_eq!(
            std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").unwrap(),
            "0"
        );
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"vector_filter_during_scan":true}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!(
            !cfg.vector_filter_during_scan_enabled(),
            "env 0 must beat file true"
        );
    }

    #[test]
    fn vector_filter_alias_parity() {
        // Single alias, but parity requires default_* and enabled helper share order.
        run_env_test(
            "config::retrieval::tests::vector_filter_alias_parity_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", Some("1"))],
        );
        run_env_test(
            "config::retrieval::tests::vector_filter_alias_parity_off_probe",
            &[("VERA_VECTOR_FILTER_DURING_SCAN", Some("0"))],
        );
    }

    #[test]
    #[ignore = "driven by vector_filter_alias_parity"]
    fn vector_filter_alias_parity_probe() {
        assert_eq!(
            std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").unwrap(),
            "1"
        );
        // Both helpers must see same alias set precedence (only one alias, but still).
        assert!(default_vector_filter_during_scan());
        let cfg = RetrievalConfig {
            vector_filter_during_scan: false,
            ..Default::default()
        };
        assert!(cfg.vector_filter_during_scan_enabled());
    }

    #[test]
    #[ignore = "driven by vector_filter_alias_parity"]
    fn vector_filter_alias_parity_off_probe() {
        assert_eq!(
            std::env::var("VERA_VECTOR_FILTER_DURING_SCAN").unwrap(),
            "0"
        );
        assert!(!default_vector_filter_during_scan());
        let cfg = RetrievalConfig {
            vector_filter_during_scan: true,
            ..Default::default()
        };
        assert!(!cfg.vector_filter_during_scan_enabled());
    }

    // ── Stem-boost gating knobs (#196) ──

    #[test]
    fn stem_gating_default_preserving() {
        run_env_test(
            "config::retrieval::tests::stem_gating_default_preserving_probe",
            &[
                ("VERA_RANKING_FILENAME_STEM_MIN_RATIO", None),
                ("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES", None),
                ("VERA_RANKING_FILENAME_STEM_BOOST", None),
            ],
        );
    }

    #[test]
    #[ignore = "driven by stem_gating_default_preserving"]
    fn stem_gating_default_preserving_probe() {
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_MIN_RATIO").is_err());
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES").is_err());
        assert_eq!(default_ranking_filename_stem_min_ratio(), 0.05);
        assert!(!default_ranking_filename_stem_skip_symbol_queries());
        let cfg = RetrievalConfig::default();
        assert!((cfg.ranking_filename_stem_min_ratio - 0.05).abs() < 1e-9);
        assert!(!cfg.ranking_filename_stem_skip_symbol_queries);
        assert!((cfg.ranking_filename_stem_min_ratio_effective() - 0.05).abs() < 1e-9);
        assert!(!cfg.ranking_filename_stem_skip_symbol_queries_enabled());
        assert!(
            cfg.ranking_filename_stem_boost,
            "existing boost default true preserved"
        );
        assert!(cfg.ranking_filename_stem_boost_enabled());
        // Legacy JSON without new fields must deserialize to defaults
        let legacy = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0}"#;
        let rcfg: RetrievalConfig = serde_json::from_str(legacy).unwrap();
        assert!((rcfg.ranking_filename_stem_min_ratio - 0.05).abs() < 1e-9);
        assert!(!rcfg.ranking_filename_stem_skip_symbol_queries);
    }

    #[test]
    fn stem_gating_config_deserialization() {
        run_env_test(
            "config::retrieval::tests::stem_gating_config_omitted_probe",
            &[
                ("VERA_RANKING_FILENAME_STEM_MIN_RATIO", None),
                ("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES", None),
            ],
        );
        run_env_test(
            "config::retrieval::tests::stem_gating_config_explicit_probe",
            &[
                ("VERA_RANKING_FILENAME_STEM_MIN_RATIO", None),
                ("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES", None),
            ],
        );
    }

    #[test]
    #[ignore = "driven by stem_gating_config_deserialization"]
    fn stem_gating_config_omitted_probe() {
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_MIN_RATIO").is_err());
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES").is_err());
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!(
            (cfg.ranking_filename_stem_min_ratio - 0.05).abs() < 1e-9,
            "omitted min_ratio must be 0.05"
        );
        assert!(
            !cfg.ranking_filename_stem_skip_symbol_queries,
            "omitted skip must be false"
        );
        // effective helpers without env should mirror stored values
        assert!((cfg.ranking_filename_stem_min_ratio_effective() - 0.05).abs() < 1e-9);
        assert!(!cfg.ranking_filename_stem_skip_symbol_queries_enabled());
        // Ensure unrelated fields unchanged
        assert!(cfg.ranking_filename_stem_boost);
    }

    #[test]
    #[ignore = "driven by stem_gating_config_deserialization"]
    fn stem_gating_config_explicit_probe() {
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_MIN_RATIO").is_err());
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES").is_err());
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"ranking_filename_stem_min_ratio":0.5,"ranking_filename_stem_skip_symbol_queries":true}"#;
        let cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!((cfg.ranking_filename_stem_min_ratio - 0.5).abs() < 1e-9);
        assert!(cfg.ranking_filename_stem_skip_symbol_queries);
        assert!((cfg.ranking_filename_stem_min_ratio_effective() - 0.5).abs() < 1e-9);
        assert!(cfg.ranking_filename_stem_skip_symbol_queries_enabled());
    }

    #[test]
    fn stem_gating_env_precedence() {
        run_env_test(
            "config::retrieval::tests::stem_gating_env_overrides_file_and_default_min_ratio_probe",
            &[("VERA_RANKING_FILENAME_STEM_MIN_RATIO", Some("0.75"))],
        );
        run_env_test(
            "config::retrieval::tests::stem_gating_env_overrides_file_and_default_skip_probe",
            &[("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES", Some("1"))],
        );
        run_env_test(
            "config::retrieval::tests::stem_gating_env_unset_fallback_probe",
            &[
                ("VERA_RANKING_FILENAME_STEM_MIN_RATIO", None),
                ("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES", None),
            ],
        );
    }

    #[test]
    #[ignore = "driven by stem_gating_env_precedence"]
    fn stem_gating_env_overrides_file_and_default_min_ratio_probe() {
        assert_eq!(
            std::env::var("VERA_RANKING_FILENAME_STEM_MIN_RATIO").unwrap(),
            "0.75"
        );
        // env should make default helper return 0.75 even when no file
        assert!((default_ranking_filename_stem_min_ratio() - 0.75).abs() < 1e-9);
        // stored false/0.05 vs env 0.75 -> env wins
        let cfg = RetrievalConfig {
            ranking_filename_stem_min_ratio: 0.05,
            ..Default::default()
        };
        assert!(
            (cfg.ranking_filename_stem_min_ratio_effective() - 0.75).abs() < 1e-9,
            "env 0.75 must override stored 0.05"
        );
        // file says 0.5 but env 0.75 still wins
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"ranking_filename_stem_min_ratio":0.5}"#;
        let file_cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!((file_cfg.ranking_filename_stem_min_ratio - 0.5).abs() < 1e-9);
        assert!(
            (file_cfg.ranking_filename_stem_min_ratio_effective() - 0.75).abs() < 1e-9,
            "env must beat file 0.5"
        );
    }

    #[test]
    #[ignore = "driven by stem_gating_env_precedence"]
    fn stem_gating_env_overrides_file_and_default_skip_probe() {
        assert_eq!(
            std::env::var("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES").unwrap(),
            "1"
        );
        assert!(default_ranking_filename_stem_skip_symbol_queries());
        let cfg = RetrievalConfig {
            ranking_filename_stem_skip_symbol_queries: false,
            ..Default::default()
        };
        assert!(
            cfg.ranking_filename_stem_skip_symbol_queries_enabled(),
            "env 1 must override stored false"
        );
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"ranking_filename_stem_skip_symbol_queries":false}"#;
        let file_cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!(!file_cfg.ranking_filename_stem_skip_symbol_queries);
        assert!(
            file_cfg.ranking_filename_stem_skip_symbol_queries_enabled(),
            "env must beat file false"
        );
        // Also verify env false overrides file true
    }

    #[test]
    #[ignore = "driven by stem_gating_env_precedence"]
    fn stem_gating_env_unset_fallback_probe() {
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_MIN_RATIO").is_err());
        assert!(std::env::var("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES").is_err());
        assert!((default_ranking_filename_stem_min_ratio() - 0.05).abs() < 1e-9);
        assert!(!default_ranking_filename_stem_skip_symbol_queries());
        let cfg = RetrievalConfig::default();
        assert!((cfg.ranking_filename_stem_min_ratio - 0.05).abs() < 1e-9);
        assert!(!cfg.ranking_filename_stem_skip_symbol_queries);
        // explicit file values survive when env unset
        let json = r#"{"default_limit":5,"rrf_k":60.0,"rerank_candidates":50,"reranking_enabled":false,"max_rerank_batch":20,"max_output_chars":0,"ranking_filename_stem_min_ratio":0.5,"ranking_filename_stem_skip_symbol_queries":true}"#;
        let file_cfg: RetrievalConfig = serde_json::from_str(json).unwrap();
        assert!((file_cfg.ranking_filename_stem_min_ratio_effective() - 0.5).abs() < 1e-9);
        assert!(file_cfg.ranking_filename_stem_skip_symbol_queries_enabled());
    }

    #[test]
    fn stem_gating_alias_parity() {
        run_env_test(
            "config::retrieval::tests::stem_gating_min_ratio_alias_parity_probe",
            &[("VERA_RANKING_FILENAME_STEM_MIN_RATIO", Some("0.5"))],
        );
        run_env_test(
            "config::retrieval::tests::stem_gating_min_ratio_alias_parity_off_probe",
            &[("VERA_RANKING_FILENAME_STEM_MIN_RATIO", Some("0.25"))],
        );
        run_env_test(
            "config::retrieval::tests::stem_gating_skip_alias_parity_probe",
            &[("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES", Some("1"))],
        );
        run_env_test(
            "config::retrieval::tests::stem_gating_skip_alias_parity_off_probe",
            &[("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES", Some("0"))],
        );
    }

    #[test]
    #[ignore = "driven by stem_gating_alias_parity"]
    fn stem_gating_min_ratio_alias_parity_probe() {
        assert_eq!(
            std::env::var("VERA_RANKING_FILENAME_STEM_MIN_RATIO").unwrap(),
            "0.5"
        );
        assert!((default_ranking_filename_stem_min_ratio() - 0.5).abs() < 1e-9);
        let cfg = RetrievalConfig {
            ranking_filename_stem_min_ratio: 0.05,
            ..Default::default()
        };
        assert!(
            (cfg.ranking_filename_stem_min_ratio_effective() - 0.5).abs() < 1e-9,
            "effective must match default helper's env resolution"
        );
        // stored 0.9 must be ignored when env present
        let cfg2 = RetrievalConfig {
            ranking_filename_stem_min_ratio: 0.9,
            ..Default::default()
        };
        assert!((cfg2.ranking_filename_stem_min_ratio_effective() - 0.5).abs() < 1e-9);
    }

    #[test]
    #[ignore = "driven by stem_gating_alias_parity"]
    fn stem_gating_min_ratio_alias_parity_off_probe() {
        assert_eq!(
            std::env::var("VERA_RANKING_FILENAME_STEM_MIN_RATIO").unwrap(),
            "0.25"
        );
        assert!((default_ranking_filename_stem_min_ratio() - 0.25).abs() < 1e-9);
        let cfg = RetrievalConfig {
            ranking_filename_stem_min_ratio: 0.8,
            ..Default::default()
        };
        assert!((cfg.ranking_filename_stem_min_ratio_effective() - 0.25).abs() < 1e-9);
    }

    #[test]
    #[ignore = "driven by stem_gating_alias_parity"]
    fn stem_gating_skip_alias_parity_probe() {
        assert_eq!(
            std::env::var("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES").unwrap(),
            "1"
        );
        assert!(default_ranking_filename_stem_skip_symbol_queries());
        let cfg = RetrievalConfig {
            ranking_filename_stem_skip_symbol_queries: false,
            ..Default::default()
        };
        assert!(cfg.ranking_filename_stem_skip_symbol_queries_enabled());
    }

    #[test]
    #[ignore = "driven by stem_gating_alias_parity"]
    fn stem_gating_skip_alias_parity_off_probe() {
        assert_eq!(
            std::env::var("VERA_RANKING_FILENAME_STEM_SKIP_SYMBOL_QUERIES").unwrap(),
            "0"
        );
        assert!(!default_ranking_filename_stem_skip_symbol_queries());
        let cfg = RetrievalConfig {
            ranking_filename_stem_skip_symbol_queries: true,
            ..Default::default()
        };
        assert!(!cfg.ranking_filename_stem_skip_symbol_queries_enabled());
    }
}
