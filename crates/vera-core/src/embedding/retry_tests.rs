//! Local HTTP tests for retry timing, queue order, and request accounting.

use super::*;
use crate::embedding::{DynamicProvider, EmbeddingRequestStats};
use crate::types::Language;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};

fn small_policy() -> RetryPolicy {
    RetryPolicy {
        base_delay: Duration::from_millis(2),
        max_delay: Duration::from_millis(8),
        max_retry_after: Duration::from_millis(20),
    }
}

#[test]
fn retry_delay_grows_caps_and_applies_equal_jitter() {
    let policy = RetryPolicy::default();
    for (attempt, millis) in [(1, 500), (2, 1000), (3, 2000), (7, 30_000), (100, 30_000)] {
        let full = Duration::from_millis(millis);
        assert_eq!(retry_delay(attempt, None, &policy, 0.0), full / 2);
        assert_eq!(retry_delay(attempt, None, &policy, 0.5), full.mul_f64(0.75));
        for jitter in [0.0, 0.01, 0.5, 0.99, 1.0 - f64::EPSILON] {
            let delay = retry_delay(attempt, None, &policy, jitter);
            assert!((full / 2..=full).contains(&delay));
        }
    }
    for wait in [
        Duration::ZERO,
        Duration::from_secs(5),
        Duration::from_secs(90),
    ] {
        for jitter in [0.0, 0.99] {
            assert_eq!(
                retry_delay(10, Some(wait), &policy, jitter),
                wait.min(Duration::from_secs(60))
            );
        }
    }
    let rate_policy = policy.for_error(&EmbeddingError::RateLimitError {
        message: "busy".into(),
        retry_after: None,
    });
    assert_eq!(
        retry_delay(1, None, &rate_policy, 0.0),
        Duration::from_secs(1)
    );
    assert_eq!(
        retry_delay(2, None, &rate_policy, 0.5),
        Duration::from_secs(3)
    );
    assert_eq!(
        retry_delay(10, None, &rate_policy, 0.0),
        Duration::from_secs(15)
    );
    for _ in 0..100 {
        assert!((0.0..1.0).contains(&retry_jitter()));
    }
}

#[test]
fn retry_after_parses_seconds_dates_and_invalid_values() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert_eq!(
        parse_retry_after(" 12 ", now),
        Some(Duration::from_secs(12))
    );
    assert_eq!(parse_retry_after("0", now), Some(Duration::ZERO));
    let future = httpdate::fmt_http_date(now + Duration::from_secs(30));
    assert_eq!(
        parse_retry_after(&future, now),
        Some(Duration::from_secs(30))
    );
    let past = httpdate::fmt_http_date(now - Duration::from_secs(30));
    assert_eq!(parse_retry_after(&past, now), Some(Duration::ZERO));
    for garbage in ["garbage", "", "-1", "1.5"] {
        assert_eq!(parse_retry_after(garbage, now), None);
    }
}

#[test]
fn retry_classification_excludes_permanent_and_context_errors() {
    for status in [408, 500, 502, 503, 504, 599] {
        assert!(is_retryable_error(&EmbeddingError::ApiError {
            status,
            message: "transient".into(),
        }));
    }
    for error in [
        EmbeddingError::AuthError {
            message: "denied".into(),
        },
        EmbeddingError::ResponseError {
            message: "bad JSON".into(),
        },
        EmbeddingError::ApiError {
            status: 400,
            message: "bad request".into(),
        },
        EmbeddingError::ApiError {
            status: 422,
            message: "invalid input".into(),
        },
        EmbeddingError::ApiError {
            status: 500,
            message: "maximum input length is 8192 tokens".into(),
        },
        EmbeddingError::Cancelled,
    ] {
        assert!(!is_retryable_error(&error));
        assert!(!is_transient_batch_error(&error));
    }
    let timeout = EmbeddingError::TimeoutError {
        message: "busy proxy".into(),
    };
    assert!(!is_retryable_error(&timeout));
    assert!(is_transient_batch_error(&timeout));
}

enum Reply {
    Success,
    Error {
        status: &'static str,
        retry_after: Option<&'static str>,
        body: &'static str,
    },
    Timeout,
    Disconnect,
    InvalidResponse,
}

#[derive(Clone)]
struct RecordedRequest {
    input: Vec<String>,
    at: Instant,
}

struct MockApi {
    url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: JoinHandle<()>,
}

impl Drop for MockApi {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockApi {
    async fn start(handler: impl Fn(&[String], usize) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::<RecordedRequest>::new()));
        let records = requests.clone();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            // Dropping the server task aborts hanging connection handlers too.
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    result = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap().unwrap();
                    }
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.unwrap();
                        let records = records.clone();
                        let handler = handler.clone();
                        connections.spawn(async move {
                            let input = read_input(&mut stream).await;
                            let attempt = {
                                let mut records = records.lock().unwrap();
                                let attempt = records.iter().filter(|r| r.input == input).count() + 1;
                                records.push(RecordedRequest { input: input.clone(), at: Instant::now() });
                                attempt
                            };
                            match handler(&input, attempt) {
                                Reply::Timeout => {
                                    std::future::pending::<()>().await;
                                    drop(stream);
                                }
                                Reply::Disconnect => {}
                                reply => {
                                    let (status, headers, body) = match reply {
                                        Reply::Success => {
                                            let data: Vec<_> = input.iter().enumerate().map(|(index, text)| {
                                                serde_json::json!({"index": index, "embedding": vector_for(text)})
                                            }).collect();
                                            ("200 OK", String::new(), serde_json::json!({"data": data}).to_string())
                                        }
                                        Reply::Error { status, retry_after, body } => {
                                            let headers = retry_after.map(|v| format!("Retry-After: {v}\r\n")).unwrap_or_default();
                                            (status, headers, body.to_string())
                                        }
                                        Reply::InvalidResponse => ("200 OK", String::new(), "not JSON".to_string()),
                                        Reply::Timeout | Reply::Disconnect => unreachable!(),
                                    };
                                    let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}", body.len());
                                    stream.write_all(response.as_bytes()).await.unwrap();
                                }
                            }
                        });
                    }
                }
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    fn provider(&self, max_retries: u32) -> OpenAiProvider {
        let config =
            EmbeddingProviderConfig::new(self.url.clone(), "test-model".into(), "key".into())
                .with_timeout(Duration::from_millis(100))
                .with_max_retries(max_retries);
        OpenAiProvider::new(config)
            .unwrap()
            .with_retry_policy(small_policy())
    }

    fn inputs(&self) -> Vec<Vec<String>> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.input.clone())
            .collect()
    }
}

async fn read_input(stream: &mut TcpStream) -> Vec<String> {
    let mut bytes = Vec::new();
    let body_start = loop {
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0, "request headers ended early");
        bytes.extend_from_slice(&buffer[..count]);
    };
    let headers = std::str::from_utf8(&bytes[..body_start]).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    while bytes.len() < body_start + length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0, "request body ended early");
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body: serde_json::Value =
        serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap();
    serde_json::from_value(body["input"].clone()).unwrap()
}

fn vector_for(text: &str) -> Vec<f32> {
    vec![
        text.len() as f32,
        text.bytes().map(u32::from).sum::<u32>() as f32,
    ]
}

fn chunks(count: usize) -> Vec<Chunk> {
    (0..count)
        .map(|index| Chunk {
            id: format!("chunk-{index}"),
            file_path: "test.rs".into(),
            line_start: index as u32 + 1,
            line_end: index as u32 + 1,
            content: format!("fn chunk_{index}() {{}}"),
            language: Language::Rust,
            symbol_type: None,
            symbol_name: None,
            part_index: None,
        })
        .collect()
}

fn assert_output_order(output: &[(String, Vec<f32>)], chunks: &[Chunk]) {
    let expected: Vec<_> = chunks
        .iter()
        .map(|chunk| {
            (
                chunk.id.clone(),
                vector_for(&chunk_to_embedding_text(chunk, 0)),
            )
        })
        .collect();
    assert_eq!(output, expected);
}

#[tokio::test(start_paused = true)]
async fn sliding_window_completes_other_batches_before_a_slow_first_batch() {
    struct SlowFirstProvider(Mutex<Vec<String>>);

    impl EmbeddingProvider for SlowFirstProvider {
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            if texts[0].contains("fn chunk_0") {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            self.0.lock().unwrap().extend_from_slice(texts);
            Ok(texts.iter().map(|text| vector_for(text)).collect())
        }

        fn expected_dim(&self) -> Option<usize> {
            Some(2)
        }
    }

    let provider = SlowFirstProvider(Mutex::new(Vec::new()));
    let chunks = chunks(5);
    let output = embed_chunks_concurrent(&provider, &chunks, 1, 2, 0)
        .await
        .unwrap();
    let expected: Vec<_> = [1, 2, 3, 4, 0]
        .map(|index| chunk_to_embedding_text(&chunks[index], 0))
        .into();
    assert_eq!(*provider.0.lock().unwrap(), expected);
    assert_output_order(&output, &chunks);
}

#[derive(Default)]
struct TerminalDrainProvider {
    stats: EmbeddingStats,
    hang: bool,
    draining: Notify,
}

impl EmbeddingProvider for TerminalDrainProvider {
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        self.stats.requests.fetch_add(1, Ordering::Relaxed);
        if texts[0].contains("fn chunk_0") {
            return Err(EmbeddingError::ApiError {
                status: 400,
                message: "first terminal error".into(),
            });
        }
        if self.hang {
            self.draining.notify_one();
            return std::future::pending().await;
        }
        let transient = texts[0].contains("fn chunk_2");
        tokio::time::sleep(Duration::from_secs(if transient { 2 } else { 1 })).await;
        if transient {
            Err(EmbeddingError::ApiError {
                status: 500,
                message: "later error".into(),
            })
        } else {
            Ok(texts.iter().map(|text| vector_for(text)).collect())
        }
    }

    fn expected_dim(&self) -> Option<usize> {
        Some(2)
    }

    fn stats(&self) -> Option<&EmbeddingStats> {
        Some(&self.stats)
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_failure_drains_started_batches_and_keeps_the_first_error() {
    let provider = TerminalDrainProvider::default();
    let root = tempfile::tempdir().unwrap();
    let checkpoint = EmbeddingCheckpoint::open(&root.path().join(".vera"), "test-model").unwrap();
    let chunks = chunks(5);
    let progress = Mutex::new(Vec::new());
    let result = embed_chunks_concurrent_with_progress_and_cancellation(
        &provider,
        &chunks,
        1,
        3,
        0,
        &CancellationToken::new(),
        Some(&checkpoint),
        |done, total| progress.lock().unwrap().push((done, total)),
    )
    .await;
    assert!(
        matches!(result, Err(EmbeddingError::ApiError { status: 400, message })
        if message == "first terminal error")
    );
    assert_eq!(
        provider.stats.snapshot(),
        EmbeddingRequestStats {
            requests: 3,
            failed_batches: 2,
            ..Default::default()
        }
    );
    assert_eq!(*progress.lock().unwrap(), vec![(1, 5)]);
    let text = chunk_to_embedding_text(&chunks[1], 0);
    assert_eq!(checkpoint.saved_count(), 1);
    assert_eq!(
        checkpoint.lookup(&[EmbeddingCheckpoint::key(&text)]),
        vec![Some(vector_for(&text))]
    );
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_terminal_drain_returns_the_first_error_at_once() {
    let provider = TerminalDrainProvider {
        hang: true,
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let chunks = chunks(5);
    let future = embed_chunks_concurrent_with_progress_and_cancellation(
        &provider,
        &chunks,
        1,
        3,
        0,
        &cancel,
        None,
        |_, _| panic!("cancelled batches must not report progress"),
    );
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => panic!("embedding ended before drain: {result:?}"),
        _ = provider.draining.notified() => {}
    }
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_millis(1), future)
        .await
        .unwrap();
    assert!(
        matches!(result, Err(EmbeddingError::ApiError { status: 400, .. })),
        "{result:?}"
    );
    assert_eq!(
        provider.stats.snapshot(),
        EmbeddingRequestStats {
            requests: 3,
            failed_batches: 1,
            ..Default::default()
        }
    );
}

#[tokio::test]
async fn server_errors_retry_then_succeed_with_exact_counters() {
    for status in [
        "500 Internal Server Error",
        "408 Request Timeout",
        "503 Service Unavailable",
    ] {
        let server = MockApi::start(move |_, attempt| {
            if attempt == 1 {
                Reply::Error {
                    status,
                    retry_after: None,
                    body: "busy",
                }
            } else {
                Reply::Success
            }
        })
        .await;
        let provider = server.provider(1);
        assert_eq!(
            provider.embed_batch(&["input".into()]).await.unwrap(),
            vec![vector_for("input")]
        );
        assert_eq!(server.inputs(), vec![vec!["input".to_string()]; 2]);
        assert_eq!(
            provider.stats().unwrap().snapshot(),
            EmbeddingRequestStats {
                requests: 2,
                retries: 1,
                ..Default::default()
            }
        );
    }
}

#[tokio::test]
async fn retry_after_is_honored_on_429_and_503_with_exact_counters() {
    for status in ["429 Too Many Requests", "503 Service Unavailable"] {
        let server = MockApi::start(move |_, attempt| {
            if attempt == 1 {
                Reply::Error {
                    status,
                    retry_after: Some("1"),
                    body: "busy",
                }
            } else {
                Reply::Success
            }
        })
        .await;
        let provider = server.provider(0);
        provider.embed_batch(&["input".into()]).await.unwrap();
        let records = server.requests.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert!(records[1].at.duration_since(records[0].at) >= small_policy().max_retry_after);
        assert_eq!(
            provider.stats().unwrap().snapshot(),
            EmbeddingRequestStats {
                requests: 2,
                retries: 1,
                ..Default::default()
            }
        );
    }
}

#[tokio::test]
async fn connection_and_overload_errors_keep_immediate_retries() {
    for disconnect in [true, false] {
        let server = MockApi::start(move |_, attempt| {
            if attempt != 1 {
                Reply::Success
            } else if disconnect {
                Reply::Disconnect
            } else {
                Reply::Error {
                    status: "400 Bad Request",
                    retry_after: None,
                    body: "Unable to process",
                }
            }
        })
        .await;
        let provider = server.provider(1);
        provider.embed_batch(&["input".into()]).await.unwrap();
        assert_eq!(server.inputs().len(), 2);
        assert_eq!(
            provider.stats().unwrap().snapshot(),
            EmbeddingRequestStats {
                requests: 2,
                retries: 1,
                ..Default::default()
            }
        );
    }
}

#[tokio::test]
async fn timed_out_batch_requeues_after_other_batches_without_immediate_retry() {
    let server = MockApi::start(|input, attempt| {
        if input[0].contains("fn chunk_0") && attempt == 1 {
            Reply::Timeout
        } else {
            Reply::Success
        }
    })
    .await;
    let provider = server.provider(3);
    let chunks = chunks(3);
    let output = embed_chunks_concurrent(&provider, &chunks, 1, 1, 0)
        .await
        .unwrap();
    assert_output_order(&output, &chunks);
    let texts: Vec<_> = chunks
        .iter()
        .map(|chunk| vec![chunk_to_embedding_text(chunk, 0)])
        .collect();
    assert_eq!(
        server.inputs(),
        vec![
            texts[0].clone(),
            texts[1].clone(),
            texts[2].clone(),
            texts[0].clone()
        ]
    );
    assert_eq!(
        provider.stats().unwrap().snapshot(),
        EmbeddingRequestStats {
            requests: 4,
            retries: 1,
            timeouts: 1,
            failed_batches: 1
        }
    );
}

#[tokio::test]
async fn mixed_batches_retry_twice_from_back_and_preserve_output_order() {
    let server = MockApi::start(|input, attempt| {
        if input[0].contains("fn chunk_0") && attempt <= 4 {
            Reply::Error {
                status: "500 Internal Server Error",
                retry_after: None,
                body: "busy",
            }
        } else {
            Reply::Success
        }
    })
    .await;
    let provider = server.provider(1);
    let chunks = chunks(4);
    let output = embed_chunks_concurrent(&provider, &chunks, 1, 1, 0)
        .await
        .unwrap();
    assert_output_order(&output, &chunks);
    let texts: Vec<_> = chunks
        .iter()
        .map(|chunk| vec![chunk_to_embedding_text(chunk, 0)])
        .collect();
    let expected = [0, 0, 1, 2, 3, 0, 0, 0].map(|index| texts[index].clone());
    assert_eq!(server.inputs(), expected);
    assert_eq!(
        provider.stats().unwrap().snapshot(),
        EmbeddingRequestStats {
            requests: 8,
            retries: 4,
            timeouts: 0,
            failed_batches: 2
        }
    );
}

#[tokio::test]
async fn timeout_requeues_are_bounded_and_successful_siblings_are_checkpointed() {
    let server = MockApi::start(|input, _| {
        if input[0].contains("fn chunk_0") {
            Reply::Timeout
        } else {
            Reply::Success
        }
    })
    .await;
    let provider = server.provider(3);
    let root = tempfile::tempdir().unwrap();
    let checkpoint = EmbeddingCheckpoint::open(&root.path().join(".vera"), "test-model").unwrap();
    let chunks = chunks(3);
    let result = embed_chunks_concurrent_with_progress_and_cancellation(
        &provider,
        &chunks,
        1,
        2,
        0,
        &CancellationToken::new(),
        Some(&checkpoint),
        |_, _| {},
    )
    .await;
    assert!(matches!(result, Err(EmbeddingError::TimeoutError { .. })));
    assert_eq!(
        provider.stats().unwrap().snapshot(),
        EmbeddingRequestStats {
            requests: 5,
            retries: 2,
            timeouts: 3,
            failed_batches: 3
        }
    );
    let inputs = server.inputs();
    assert_eq!(
        inputs
            .iter()
            .filter(|input| input[0].contains("fn chunk_0"))
            .count(),
        (1 + MAX_REQUEUES) as usize
    );
    let texts: Vec<_> = chunks
        .iter()
        .map(|chunk| chunk_to_embedding_text(chunk, 0))
        .collect();
    let keys: Vec<_> = texts
        .iter()
        .map(|text| EmbeddingCheckpoint::key(text))
        .collect();
    assert_eq!(
        checkpoint.lookup(&keys),
        vec![
            None,
            Some(vector_for(&texts[1])),
            Some(vector_for(&texts[2]))
        ]
    );
    assert_eq!(checkpoint.saved_count(), 2);
}

#[tokio::test]
async fn permanent_client_errors_and_invalid_responses_fail_after_one_request() {
    for status in [
        "400 Bad Request",
        "401 Unauthorized",
        "403 Forbidden",
        "422 Unprocessable Entity",
        "200 OK",
    ] {
        let server = MockApi::start(move |_, _| {
            if status == "200 OK" {
                Reply::InvalidResponse
            } else {
                Reply::Error {
                    status,
                    retry_after: None,
                    body: "invalid request",
                }
            }
        })
        .await;
        let provider = server.provider(3);
        let result = embed_chunks_concurrent(&provider, &chunks(1), 1, 1, 0).await;
        match status {
            "401 Unauthorized" | "403 Forbidden" => {
                assert!(matches!(result, Err(EmbeddingError::AuthError { .. })))
            }
            "200 OK" => assert!(matches!(result, Err(EmbeddingError::ResponseError { .. }))),
            _ => assert!(matches!(result, Err(EmbeddingError::ApiError { .. }))),
        }
        assert_eq!(server.inputs().len(), 1);
        assert_eq!(
            provider.stats().unwrap().snapshot(),
            EmbeddingRequestStats {
                requests: 1,
                failed_batches: 1,
                ..Default::default()
            }
        );
    }
}

struct WaitObserver {
    provider: OpenAiProvider,
    waiting: Notify,
}

impl EmbeddingProvider for WaitObserver {
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        self.provider.embed_batch(texts).await
    }

    fn expected_dim(&self) -> Option<usize> {
        None
    }

    fn stats(&self) -> Option<&EmbeddingStats> {
        self.provider.stats()
    }

    fn requeue_delay(&self, attempt: u32, error: &EmbeddingError) -> Duration {
        self.waiting.notify_one();
        self.provider.requeue_delay(attempt, error)
    }
}

#[tokio::test]
async fn cancellation_during_requeue_wait_returns_promptly_without_resending() {
    let server = MockApi::start(|_, _| Reply::Timeout).await;
    let provider = WaitObserver {
        provider: server.provider(3).with_retry_policy(RetryPolicy {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(1),
            ..small_policy()
        }),
        waiting: Notify::new(),
    };
    let cancel = CancellationToken::new();
    let chunks = chunks(1);
    let future = embed_chunks_concurrent_with_progress_and_cancellation(
        &provider,
        &chunks,
        1,
        1,
        0,
        &cancel,
        None,
        |_, _| {},
    );
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => panic!("embedding ended before requeue wait: {result:?}"),
        _ = provider.waiting.notified() => {}
    }
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_millis(250), future)
        .await
        .expect("cancel must interrupt backoff");
    assert!(matches!(result, Err(EmbeddingError::Cancelled)));
    assert_eq!(server.inputs().len(), 1);
    assert_eq!(
        provider.stats().unwrap().snapshot(),
        EmbeddingRequestStats {
            requests: 1,
            retries: 0,
            timeouts: 1,
            failed_batches: 1
        }
    );
}

#[tokio::test]
async fn no_failure_requests_keep_the_existing_sorted_batch_sequence() {
    let server = MockApi::start(|_, _| Reply::Success).await;
    let provider = server.provider(1);
    let mut chunks = chunks(5);
    chunks[0].content.push_str(" // longest content");
    chunks[3].content.push_str(" // middle");
    let output = embed_chunks_concurrent(&provider, &chunks, 2, 1, 0)
        .await
        .unwrap();
    assert_output_order(&output, &chunks);
    let expected: Vec<Vec<String>> = [1, 2, 4, 3, 0]
        .chunks(2)
        .map(|batch| {
            batch
                .iter()
                .map(|&index| chunk_to_embedding_text(&chunks[index], 0))
                .collect()
        })
        .collect();
    assert_eq!(server.inputs(), expected);
    assert_eq!(
        provider.stats().unwrap().snapshot(),
        EmbeddingRequestStats {
            requests: 3,
            ..Default::default()
        }
    );
}

#[tokio::test]
async fn dynamic_and_cached_providers_expose_api_stats_and_local_hooks_default_to_none() {
    let server = MockApi::start(|_, _| Reply::Success).await;
    let cached = CachedEmbeddingProvider::new(DynamicProvider::Api(server.provider(0)), 2);
    assert_eq!(
        cached.stats().unwrap().snapshot(),
        EmbeddingRequestStats::default()
    );
    cached.embed_batch(&["input".into()]).await.unwrap();
    cached.embed_batch(&["input".into()]).await.unwrap();
    assert_eq!(
        cached.stats().unwrap().snapshot(),
        EmbeddingRequestStats {
            requests: 1,
            ..Default::default()
        }
    );
    assert!(test_helpers::MockProvider::new(2).stats().is_none());
    let serialized = serde_json::to_value(cached.stats().unwrap().snapshot()).unwrap();
    assert_eq!(
        serialized,
        serde_json::json!({"requests": 1, "retries": 0, "timeouts": 0, "failed_batches": 0})
    );
}
