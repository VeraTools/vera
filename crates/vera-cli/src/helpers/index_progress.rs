//! Shared index/update telemetry; only this layer writes progress to stderr.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vera_core::embedding::{DynamicProvider, EmbeddingProvider, EmbeddingRequestStats};
use vera_core::indexing::progress::{
    EmbedDisplay, PeriodicProgress, embedding_failure_message, embedding_message,
};

#[derive(Default)]
struct RunProgress {
    started: Option<Instant>,
    finished: Option<Instant>,
    done: usize,
    total: Option<usize>,
}

#[derive(Clone)]
pub struct EmbeddingReporter {
    provider: Arc<DynamicProvider>,
    initial_stats: EmbeddingRequestStats,
    state: Arc<Mutex<RunProgress>>,
    enabled: bool,
}

impl EmbeddingReporter {
    pub fn new(provider: Arc<DynamicProvider>, enabled: bool) -> Self {
        let initial_stats = provider
            .stats()
            .map_or(EmbeddingRequestStats::default(), |stats| stats.snapshot());
        Self {
            provider,
            initial_stats,
            state: Arc::default(),
            enabled,
        }
    }

    pub fn observe(&self, display: Option<EmbedDisplay>, total: Option<usize>) {
        let mut state = self.state.lock().unwrap();
        if total.is_some() {
            state.total = total;
        }
        match display {
            Some(EmbedDisplay::Indeterminate { done } | EmbedDisplay::Determinate { done, .. }) => {
                state.started.get_or_insert_with(Instant::now);
                state.done = done;
            }
            Some(EmbedDisplay::Done { count }) => {
                state.done = count;
                state.total = Some(count);
                state.finished = Some(Instant::now());
            }
            None => {}
        }
    }

    fn stats(&self) -> EmbeddingRequestStats {
        self.provider
            .stats()
            .map_or(EmbeddingRequestStats::default(), |stats| stats.snapshot())
            .since(self.initial_stats)
    }

    pub fn message(&self, plain: bool) -> Option<String> {
        let state = self.state.lock().unwrap();
        let start = state.started?;
        let elapsed = state
            .finished
            .unwrap_or_else(Instant::now)
            .duration_since(start);
        Some(embedding_message(
            state.done,
            state.total,
            elapsed,
            self.stats(),
            plain,
        ))
    }

    /// Tick independently of batch completion, including during retries/timeouts.
    /// The operation future stays responsible for cooperative cancellation.
    pub async fn wait<T>(
        &self,
        operation: impl Future<Output = anyhow::Result<T>>,
        interactive: bool,
        refresh: impl Fn(String),
    ) -> anyhow::Result<T> {
        if !self.enabled {
            return operation.await;
        }
        tokio::pin!(operation);
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut throttle = PeriodicProgress::default();
        loop {
            tokio::select! {
                biased;
                result = &mut operation => {
                    {
                        let mut state = self.state.lock().unwrap();
                        state.finished.get_or_insert_with(Instant::now);
                    }
                    if !interactive && throttle.should_print_final()
                        && let Some(message) = self.message(true)
                    {
                        eprintln!("{message}");
                    }
                    return result;
                }
                _ = interval.tick() => {
                    let elapsed = {
                        let state = self.state.lock().unwrap();
                        state.started.filter(|_| state.finished.is_none()).map(|start| start.elapsed())
                    };
                    if let Some(elapsed) = elapsed {
                        if interactive {
                            if let Some(message) = self.message(false) { refresh(message); }
                        } else if throttle.should_print(elapsed)
                            && let Some(message) = self.message(true)
                        {
                            eprintln!("{message}");
                        }
                    }
                }
            }
        }
    }

    pub fn print_failure<T>(&self, result: &anyhow::Result<T>) {
        let state = self.state.lock().unwrap();
        if result.is_err() && state.started.is_some() {
            eprintln!(
                "{}",
                embedding_failure_message(state.done, state.total, self.stats())
            );
        }
    }
}
