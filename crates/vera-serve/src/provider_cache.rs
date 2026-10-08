//! Residency cache for the models `vera serve` hands to requests.
//!
//! Each model gets its own slot with its own lock, so a cold embedding load
//! never blocks a rerank request. A slot owns its in-flight load, allowing
//! concurrent requests to await one build without holding the slot lock.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as AsyncMutex, watch};

/// How long a loaded model stays resident.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheMode {
    /// Build a model per request and drop it when the request ends.
    PerRequest,
    /// Keep the model loaded, evicting it after this much inactivity.
    Idle(Duration),
    /// Keep the model loaded for the lifetime of the process.
    Forever,
}

impl CacheMode {
    /// Map the `--idle-timeout` seconds value onto a cache mode.
    ///
    /// `0` disables the cache, any negative value keeps models loaded for the
    /// process lifetime, and a positive value evicts after that many seconds.
    pub fn from_idle_timeout_secs(secs: i64) -> Self {
        match secs {
            0 => Self::PerRequest,
            n if n < 0 => Self::Forever,
            n => Self::Idle(Duration::from_secs(n as u64)),
        }
    }

    fn idle_timeout(self) -> Option<Duration> {
        match self {
            Self::Idle(d) => Some(d),
            Self::PerRequest | Self::Forever => None,
        }
    }
}

struct Resident<T> {
    model: Arc<T>,
    last_used: Instant,
}

type LoadState<T, E> = Option<Result<Option<Arc<T>>, E>>;

struct SlotState<T, E> {
    resident: Option<Resident<T>>,
    loading: Option<watch::Receiver<LoadState<T, E>>>,
}

/// One cached model, loaded on first use.
pub(crate) struct ModelSlot<T, E = ()> {
    state: Arc<AsyncMutex<SlotState<T, E>>>,
    mode: CacheMode,
}

impl<T, E> ModelSlot<T, E> {
    pub(crate) fn new(mode: CacheMode) -> Self {
        Self {
            state: Arc::new(AsyncMutex::new(SlotState {
                resident: None,
                loading: None,
            })),
            mode,
        }
    }

    /// Hand out the cached model, loading it through `load` if the slot is empty.
    ///
    /// A loader that reports the model as unavailable (`Ok(None)`) leaves the
    /// slot empty, so a transient failure is retried by the next request instead
    /// of being cached for the lifetime of the process. A panicking cached
    /// loader also reports unavailable and leaves the slot retryable.
    pub(crate) async fn get_or_load<F, Fut>(&self, load: F) -> Result<Option<Arc<T>>, E>
    where
        T: Send + Sync + 'static,
        E: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<Arc<T>>, E>> + Send + 'static,
    {
        if matches!(self.mode, CacheMode::PerRequest) {
            return load().await;
        }

        let (receiver, sender) = {
            let mut state = self.state.lock().await;
            if let Some(resident) = state.resident.as_mut() {
                resident.last_used = Instant::now();
                return Ok(Some(Arc::clone(&resident.model)));
            }

            if state
                .loading
                .as_ref()
                .is_some_and(|receiver| receiver.has_changed().is_err())
            {
                state.loading = None;
            }

            if let Some(receiver) = state.loading.as_ref() {
                (receiver.clone(), None)
            } else {
                let (sender, receiver) = watch::channel(None);
                state.loading = Some(receiver.clone());
                (receiver, Some(sender))
            }
        };

        if let Some(sender) = sender {
            let state = Arc::clone(&self.state);
            let future = catch_unwind(AssertUnwindSafe(load));
            tokio::spawn(async move {
                // Observe the loader's task failure so every waiter receives a
                // result and a failed cold load cannot leave a stuck slot.
                let result = match future {
                    Ok(future) => match tokio::spawn(future).await {
                        Ok(result) => result,
                        Err(error) => {
                            tracing::error!(
                                panicked = error.is_panic(),
                                "model loader task failed before publishing a result"
                            );
                            Ok(None)
                        }
                    },
                    Err(_) => {
                        tracing::error!("model loader panicked before starting");
                        Ok(None)
                    }
                };
                let mut state = state.lock().await;
                if let Ok(Some(model)) = result.as_ref() {
                    state.resident = Some(Resident {
                        model: Arc::clone(model),
                        last_used: Instant::now(),
                    });
                }
                let _ = sender.send(Some(result));
                state.loading = None;
            });
        }

        wait_for_load(receiver).await
    }

    /// Put an already-loaded model into the slot.
    ///
    /// `run_server` probe-loads the embedding model to validate the config
    /// before it listens; seeding hands that model to the slot so the first
    /// request does not pay for a second load of a model already in memory.
    /// A no-op in `PerRequest` mode, which must rebuild per request.
    pub(crate) async fn seed(&self, model: Arc<T>) {
        if matches!(self.mode, CacheMode::PerRequest) {
            return;
        }
        self.state.lock().await.resident = Some(Resident {
            model,
            last_used: Instant::now(),
        });
    }

    /// Drop the model if it has been idle past the configured timeout and no
    /// request still holds it. Returns whether it evicted.
    pub(crate) async fn evict_if_idle(&self) -> bool {
        let Some(timeout) = self.mode.idle_timeout() else {
            return false;
        };
        let mut state = self.state.lock().await;
        let Some(resident) = state.resident.as_mut() else {
            return false;
        };
        // Evicting a model a request still holds would force a second load in
        // parallel with the first one. A held model is also in use *now*, so
        // restart its idle clock: `last_used` is stamped at acquisition, and
        // without this a request that outlives the timeout would be followed by
        // an immediate eviction instead of a fresh idle window.
        if Arc::strong_count(&resident.model) > 1 {
            resident.last_used = Instant::now();
            return false;
        }
        if resident.last_used.elapsed() < timeout {
            return false;
        }
        state.resident = None;
        true
    }
}

async fn wait_for_load<T, E>(
    mut receiver: watch::Receiver<LoadState<T, E>>,
) -> Result<Option<Arc<T>>, E>
where
    E: Clone,
{
    loop {
        let result = receiver.borrow().clone();
        if let Some(result) = result {
            return result;
        }
        if receiver.changed().await.is_err() {
            tracing::error!("model load channel closed without a result");
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Stand-in for a model: `Builds` counts how many were constructed, which is
    /// the property under test. The real models need ONNX Runtime and downloaded
    /// assets, so a test built on them would skip silently.
    struct Builds(Arc<AtomicUsize>);

    impl Builds {
        fn new() -> Self {
            Self(Arc::new(AtomicUsize::new(0)))
        }

        fn count(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }

        fn build(&self) -> Arc<usize> {
            let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            Arc::new(n)
        }

        /// A loader that succeeds, counting each construction.
        fn load(&self) -> impl Future<Output = Result<Option<Arc<usize>>, ()>> + Send + 'static {
            let builds = Arc::clone(&self.0);
            async move {
                let n = builds.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(Some(Arc::new(n)))
            }
        }
    }

    #[tokio::test]
    async fn cached_mode_builds_the_model_once_across_requests() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let builds = Builds::new();

        for _ in 0..5 {
            let model = slot.get_or_load(|| builds.load()).await.unwrap();
            assert_eq!(*model.unwrap(), 1);
        }

        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn per_request_mode_builds_the_model_every_time() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::PerRequest);
        let builds = Builds::new();

        for expected in 1..=3 {
            let model = slot.get_or_load(|| builds.load()).await.unwrap();
            assert_eq!(*model.unwrap(), expected);
        }

        assert_eq!(builds.count(), 3);
    }

    #[tokio::test]
    async fn concurrent_cold_requests_share_one_build() {
        let slot: Arc<ModelSlot<usize>> = Arc::new(ModelSlot::new(CacheMode::Forever));
        let builds = Arc::new(Builds::new());

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let slot = Arc::clone(&slot);
            let builds = Arc::clone(&builds);
            tasks.push(tokio::spawn(async move {
                let model = slot
                    .get_or_load(move || async move {
                        // Widen the window a serialized load has to lose in.
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        builds.load().await
                    })
                    .await
                    .unwrap();
                *model.unwrap()
            }));
        }

        for task in tasks {
            assert_eq!(task.await.unwrap(), 1);
        }
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn an_unavailable_model_is_not_cached_and_a_later_load_succeeds() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let attempts = Arc::new(AtomicUsize::new(0));
        let builds = Arc::new(Builds::new());

        let load = || {
            let attempts = Arc::clone(&attempts);
            let builds = Arc::clone(&builds);
            async move {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    // The shape `create_dynamic_reranker` failures take: swallowed
                    // into "not available" rather than surfaced as an error.
                    return Ok(None);
                }
                builds.load().await
            }
        };

        assert!(slot.get_or_load(load).await.unwrap().is_none());
        assert_eq!(builds.count(), 0);

        let model = slot.get_or_load(load).await.unwrap();
        assert_eq!(*model.expect("retried after the failed load"), 1);

        // And the recovered model is now the cached one.
        let again = slot.get_or_load(load).await.unwrap();
        assert_eq!(*again.unwrap(), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn a_failed_load_is_not_cached_and_a_later_load_succeeds() {
        let slot: ModelSlot<usize, &'static str> = ModelSlot::new(CacheMode::Forever);
        let attempts = Arc::new(AtomicUsize::new(0));
        let builds = Arc::new(Builds::new());

        let load = || {
            let attempts = Arc::clone(&attempts);
            let builds = Arc::clone(&builds);
            async move {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    // The shape `create_dynamic_provider` failures take on the
                    // embedding path: surfaced as an error, not as "unavailable".
                    return Err("cold start failed");
                }
                Ok(builds.load().await.unwrap())
            }
        };

        assert_eq!(
            slot.get_or_load(load).await.unwrap_err(),
            "cold start failed"
        );
        assert_eq!(builds.count(), 0);

        let model = slot.get_or_load(load).await.unwrap();
        assert_eq!(*model.expect("retried after the errored load"), 1);

        // And the recovered model is the cached one from here on.
        let again = slot.get_or_load(load).await.unwrap();
        assert_eq!(*again.unwrap(), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn a_panicking_loader_leaves_the_slot_retryable() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let builds = Builds::new();

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            slot.get_or_load(|| async { panic!("model loader failed") }),
        )
        .await
        .expect("a failed loader must release waiting requests");
        assert!(result.unwrap().is_none());

        for _ in 0..2 {
            let model = slot.get_or_load(|| builds.load()).await.unwrap().unwrap();
            assert_eq!(*model, 1);
        }
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn a_panicking_cold_load_releases_all_waiters() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let attempts = Arc::new(AtomicUsize::new(0));
        let load = || {
            let attempts = Arc::clone(&attempts);
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                panic!("shared model loader failed");
            }
        };

        let (first, second) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(slot.get_or_load(load), slot.get_or_load(load))
        })
        .await
        .expect("all requests waiting on a failed loader must be released");
        assert!(first.unwrap().is_none());
        assert!(second.unwrap().is_none());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_panicking_loader_factory_leaves_the_slot_retryable() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let builds = Builds::new();

        let result = slot
            .get_or_load(|| -> std::future::Ready<Result<Option<Arc<usize>>, ()>> {
                panic!("model loader could not be created");
            })
            .await;
        assert!(result.unwrap().is_none());
        assert_eq!(
            *slot.get_or_load(|| builds.load()).await.unwrap().unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn a_closed_load_channel_does_not_block_a_later_load() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let builds = Builds::new();
        let (sender, receiver) = watch::channel(None);
        drop(sender);

        assert!(
            wait_for_load::<usize, ()>(receiver.clone())
                .await
                .unwrap()
                .is_none()
        );
        slot.state.lock().await.loading = Some(receiver);
        let model =
            tokio::time::timeout(Duration::from_secs(5), slot.get_or_load(|| builds.load()))
                .await
                .expect("a closed load channel must not block a retry")
                .unwrap()
                .unwrap();
        assert_eq!(*model, 1);
    }

    #[tokio::test]
    async fn a_dropped_initiator_does_not_discard_a_cold_load() {
        let slot: Arc<ModelSlot<usize>> = Arc::new(ModelSlot::new(CacheMode::Forever));
        let builds = Arc::new(Builds::new());
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();

        let first = {
            let slot = Arc::clone(&slot);
            let builds = Arc::clone(&builds);
            tokio::spawn(async move {
                slot.get_or_load(move || async move {
                    started.send(()).unwrap();
                    let model = builds.build();
                    released.await.unwrap();
                    Ok(Some(model))
                })
                .await
            })
        };

        started_rx.await.unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());

        let second = {
            let slot = Arc::clone(&slot);
            let builds = Arc::clone(&builds);
            tokio::spawn(async move {
                slot.get_or_load(move || async move { Ok(Some(builds.build())) })
                    .await
            })
        };

        release.send(()).unwrap();
        let model = second.await.unwrap().unwrap().unwrap();
        assert_eq!(*model, 1);
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn a_seeded_model_is_served_without_loading() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let builds = Builds::new();

        slot.seed(Arc::new(9usize)).await;

        for _ in 0..3 {
            let model = slot.get_or_load(|| builds.load()).await.unwrap();
            assert_eq!(*model.unwrap(), 9);
        }
        assert_eq!(builds.count(), 0);
    }

    #[tokio::test]
    async fn per_request_mode_ignores_a_seed() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::PerRequest);
        let builds = Builds::new();

        slot.seed(Arc::new(9usize)).await;

        let model = slot.get_or_load(|| builds.load()).await.unwrap();
        assert_eq!(*model.unwrap(), 1);
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn a_seeded_model_is_evicted_when_idle_like_a_loaded_one() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Idle(Duration::ZERO));
        let builds = Builds::new();

        slot.seed(Arc::new(9usize)).await;
        assert!(slot.evict_if_idle().await);

        let model = slot.get_or_load(|| builds.load()).await.unwrap();
        assert_eq!(*model.unwrap(), 1);
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn one_slots_cold_load_does_not_block_the_other_slot() {
        let embedding: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let reranker: Arc<ModelSlot<usize>> = Arc::new(ModelSlot::new(CacheMode::Forever));

        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let (entered, has_entered) = tokio::sync::oneshot::channel::<()>();
        let slow = {
            let reranker = Arc::clone(&reranker);
            tokio::spawn(async move {
                reranker
                    .get_or_load(move || async move {
                        entered.send(()).unwrap();
                        let _ = released.await;
                        Ok::<_, ()>(Some(Arc::new(7usize)))
                    })
                    .await
                    .unwrap()
            })
        };

        // Only proceed once the reranker load is parked inside its slot lock.
        has_entered.await.unwrap();

        let builds = Builds::new();
        let quick = tokio::time::timeout(
            Duration::from_secs(5),
            embedding.get_or_load(|| builds.load()),
        )
        .await
        .expect("embedding acquire must not wait on the reranker load")
        .unwrap();

        assert_eq!(*quick.unwrap(), 1);
        release.send(()).unwrap();
        assert_eq!(*slow.await.unwrap().unwrap(), 7);
    }

    #[tokio::test]
    async fn an_idle_model_is_evicted_and_the_next_request_rebuilds_it() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Idle(Duration::ZERO));
        let builds = Builds::new();

        let first = slot.get_or_load(|| builds.load()).await.unwrap();
        assert_eq!(*first.unwrap(), 1);

        assert!(slot.evict_if_idle().await);

        let second = slot.get_or_load(|| builds.load()).await.unwrap();
        assert_eq!(*second.unwrap(), 2);
        assert_eq!(builds.count(), 2);
    }

    #[tokio::test]
    async fn eviction_spares_a_model_a_request_still_holds() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Idle(Duration::ZERO));
        let builds = Builds::new();

        let held = slot.get_or_load(|| builds.load()).await.unwrap().unwrap();
        assert!(!slot.evict_if_idle().await);

        drop(held);
        assert!(slot.evict_if_idle().await);
    }

    /// `last_used` is stamped at acquisition, so a request that outlives the
    /// idle window would otherwise be followed by an immediate eviction: the
    /// timeout has to measure inactivity after the last request, not since it
    /// started. Real sleeps, because the slot uses `std::time::Instant`, which
    /// tokio's paused clock does not control. Overshoot under load only makes
    /// the eviction assertions more true; the one budget that matters is the
    /// 1s window covering a `drop` and one call.
    #[tokio::test]
    async fn the_idle_clock_restarts_when_the_last_request_releases_the_model() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Idle(Duration::from_secs(1)));
        let builds = Builds::new();

        let held = slot.get_or_load(|| builds.load()).await.unwrap().unwrap();

        // A request still running well past the idle window.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!slot.evict_if_idle().await, "a held model must be spared");

        drop(held);
        assert!(
            !slot.evict_if_idle().await,
            "releasing the model must start a fresh idle window, not evict at once"
        );

        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            slot.evict_if_idle().await,
            "and it evicts once genuinely idle"
        );
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn a_model_within_its_idle_window_is_not_evicted() {
        let slot: ModelSlot<usize> = ModelSlot::new(CacheMode::Idle(Duration::from_secs(3600)));
        let builds = Builds::new();

        drop(slot.get_or_load(|| builds.load()).await.unwrap());
        assert!(!slot.evict_if_idle().await);

        let again = slot.get_or_load(|| builds.load()).await.unwrap();
        assert_eq!(*again.unwrap(), 1);
        assert_eq!(builds.count(), 1);
    }

    #[tokio::test]
    async fn forever_and_per_request_modes_never_evict() {
        let forever: ModelSlot<usize> = ModelSlot::new(CacheMode::Forever);
        let builds = Builds::new();
        drop(forever.get_or_load(|| builds.load()).await.unwrap());
        assert!(!forever.evict_if_idle().await);

        let per_request: ModelSlot<usize> = ModelSlot::new(CacheMode::PerRequest);
        drop(per_request.get_or_load(|| builds.load()).await.unwrap());
        assert!(!per_request.evict_if_idle().await);
    }

    #[test]
    fn idle_timeout_seconds_map_onto_cache_modes() {
        assert_eq!(CacheMode::from_idle_timeout_secs(0), CacheMode::PerRequest);
        assert_eq!(CacheMode::from_idle_timeout_secs(-1), CacheMode::Forever);
        assert_eq!(CacheMode::from_idle_timeout_secs(-42), CacheMode::Forever);
        assert_eq!(
            CacheMode::from_idle_timeout_secs(300),
            CacheMode::Idle(Duration::from_secs(300))
        );
    }
}
