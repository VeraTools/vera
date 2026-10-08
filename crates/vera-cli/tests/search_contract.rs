use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

const QUERY: &str = "how does authentication handle user requests";

struct MockApi {
    base_url: String,
    stop: Arc<AtomicBool>,
    rerank_calls: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
}

impl MockApi {
    fn new(rerank_success: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let rerank_calls = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::clone(&stop);
        let calls = Arc::clone(&rerank_calls);
        let thread = thread::spawn(move || {
            for connection in listener.incoming() {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let mut stream = connection.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                let header_end = loop {
                    let read = stream.read(&mut buffer).unwrap();
                    assert_ne!(read, 0, "request ended before its headers");
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < header_end + length {
                    let read = stream.read(&mut buffer).unwrap();
                    assert_ne!(read, 0, "request ended before its body");
                    bytes.extend_from_slice(&buffer[..read]);
                }
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                let (status, response) = if headers.starts_with("POST /v1/embeddings ") {
                    let count = body["input"].as_array().map_or(1, Vec::len);
                    (
                        "200 OK",
                        json!({"data": (0..count).map(|index| json!({
                        "index": index, "embedding": [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
                    })).collect::<Vec<_>>() }),
                    )
                } else {
                    assert!(headers.starts_with("POST /v1/rerank "), "{headers}");
                    calls.fetch_add(1, Ordering::Relaxed);
                    assert!(body["query"].is_string());
                    let documents = body["documents"].as_array().unwrap();
                    assert!(!documents.is_empty());
                    if rerank_success {
                        (
                            "200 OK",
                            json!({"results": (0..documents.len()).map(|index| json!({
                            "index": index, "relevance_score": 1.0 / (index + 1) as f64
                        })).collect::<Vec<_>>() }),
                        )
                    } else {
                        (
                            "500 Internal Server Error",
                            json!({"error": "mock reranker failure"}),
                        )
                    }
                };
                let response = response.to_string();
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            stop,
            rerank_calls,
            thread: Some(thread),
        }
    }
}

impl Drop for MockApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let address = self
            .base_url
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/v1")
            .unwrap();
        let _ = TcpStream::connect(address);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

struct Fixture {
    dir: TempDir,
    repo: PathBuf,
    api: MockApi,
}

impl Fixture {
    fn new(rerank_success: bool) -> Self {
        let scratch = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        let dir = tempfile::tempdir_in(scratch).unwrap();
        let repo = dir.path().join("repo");
        for path in [&repo, &dir.path().join("home"), &dir.path().join("scratch")] {
            std::fs::create_dir_all(path).unwrap();
        }
        for i in 0..32 {
            std::fs::write(repo.join(format!("auth_{i}.rs")), format!(
                "/// Authentication handles user requests.\npub fn authenticate_{i}() -> usize {{ {i} }}\n"
            )).unwrap();
        }
        let fixture = Self {
            dir,
            repo,
            api: MockApi::new(rerank_success),
        };
        fixture.config("retrieval.reranking_enabled", "false");
        fixture.config("retrieval.reranker_max_retries", "0");
        fixture.run(&["index", ".", "--json", "--no-progress"], false);
        fixture
    }

    fn command(&self, remote: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vera"));
        command
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.dir.path().join("home"))
            .env("VERA_HOME", self.dir.path().join("vera-home"))
            .env("TMPDIR", self.dir.path().join("scratch"))
            .env("VERA_NO_UPDATE_CHECK", "1")
            .env("VERA_BACKEND", "api")
            .env("EMBEDDING_MODEL_BASE_URL", &self.api.base_url)
            .env("EMBEDDING_MODEL_ID", "contract-embedding")
            .env("EMBEDDING_MODEL_API_KEY", "test-key");
        if remote {
            command
                .env("RERANKER_MODEL_BASE_URL", &self.api.base_url)
                .env("RERANKER_MODEL_ID", "contract-reranker")
                .env("RERANKER_MODEL_API_KEY", "test-key");
        }
        command
    }

    fn run(&self, args: &[&str], remote: bool) -> Output {
        let output = self.command(remote).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn config(&self, key: &str, value: &str) {
        self.run(&["config", "set", key, value], false);
    }
}

fn parsed(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_results_contract(results: &Value) {
    let results = results.as_array().expect("results must be a bare array");
    assert!(!results.is_empty());
    for result in results {
        for key in ["file_path", "line_start", "line_end", "content"] {
            assert!(result.get(key).is_some(), "missing {key}: {result}");
        }
    }
    assert!(
        results
            .iter()
            .any(|result| result["symbol_name"].is_string() && result["symbol_type"].is_string())
    );
}

#[test]
fn default_json_array_and_disabled_status_are_byte_compatible() {
    let fixture = Fixture::new(false);
    let default = fixture.run(&["search", QUERY, "--json", "--limit", "1"], false);
    assert_results_contract(&parsed(&default));
    let envelope = fixture.run(
        &["search", QUERY, "--json", "--rerank-status", "--limit", "1"],
        false,
    );
    let status = parsed(&envelope);
    assert_eq!(status["results"], parsed(&default));
    assert_eq!(status["reranked"], false);
    assert!(status["reranker"].is_null());
    assert!(status["rerank_fallback_reason"].is_null());
    let text = String::from_utf8(envelope.stdout).unwrap();
    let array = text
        .strip_prefix("{\"results\":")
        .unwrap()
        .split_once(",\"reranked\":")
        .unwrap()
        .0;
    assert_eq!(format!("{array}\n").as_bytes(), default.stdout);
    let rejected = fixture
        .command(false)
        .args(["search", QUERY, "--rerank-status"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--json"));
}

#[test]
fn remote_failure_warns_and_keeps_default_array() {
    let fixture = Fixture::new(false);
    fixture.config("retrieval.reranking_enabled", "true");
    let envelope = fixture.run(
        &["search", QUERY, "--json", "--rerank-status", "--limit", "1"],
        true,
    );
    let status = parsed(&envelope);
    assert_results_contract(&status["results"]);
    assert_eq!(status["reranked"], false);
    assert_eq!(status["reranker"], "api");
    assert!(
        status["rerank_fallback_reason"]
            .as_str()
            .unwrap()
            .contains("mock reranker failure")
    );
    assert!(String::from_utf8_lossy(&envelope.stderr).contains("reranker unavailable"));
    let default = fixture.run(&["search", QUERY, "--json", "--limit", "1"], true);
    assert_results_contract(&parsed(&default));
    assert_eq!(parsed(&default), status["results"]);
    assert!(String::from_utf8_lossy(&default.stderr).contains("reranker unavailable"));
    assert_eq!(fixture.api.rerank_calls.load(Ordering::Relaxed), 2);
}

#[test]
fn remote_success_reports_api_and_empty_results_skip_reranking() {
    let fixture = Fixture::new(true);
    fixture.config("retrieval.reranking_enabled", "true");
    let output = fixture.run(
        &["search", QUERY, "--json", "--rerank-status", "--limit", "1"],
        true,
    );
    let status = parsed(&output);
    assert_results_contract(&status["results"]);
    assert_eq!(status["reranked"], true);
    assert_eq!(status["reranker"], "api");
    assert!(status["rerank_fallback_reason"].is_null());
    let calls = fixture.api.rerank_calls.load(Ordering::Relaxed);
    assert!(calls > 0);
    let empty = fixture.run(
        &[
            "search",
            QUERY,
            "--json",
            "--rerank-status",
            "--limit",
            "1",
            "--path",
            "missing/**",
        ],
        true,
    );
    let status = parsed(&empty);
    assert_eq!(status["results"], json!([]));
    assert_eq!(status["reranked"], false);
    assert!(status["rerank_fallback_reason"].is_null());
    assert_eq!(fixture.api.rerank_calls.load(Ordering::Relaxed), calls);
}

#[test]
fn multi_query_and_iterative_deep_search_preserve_fallback_reason() {
    let fixture = Fixture::new(false);
    fixture.config("retrieval.reranking_enabled", "true");
    for args in [
        vec![
            "search",
            QUERY,
            "where are authentication requests handled",
            "--json",
            "--rerank-status",
            "--limit",
            "1",
        ],
        vec![
            "search",
            QUERY,
            "--deep",
            "--json",
            "--rerank-status",
            "--limit",
            "1",
        ],
    ] {
        let output = fixture.run(&args, true);
        let status = parsed(&output);
        assert_results_contract(&status["results"]);
        assert_eq!(status["reranked"], false);
        assert_eq!(status["reranker"], "api");
        assert!(status["rerank_fallback_reason"].is_string());
        assert!(String::from_utf8_lossy(&output.stderr).contains("reranker unavailable"));
    }
    assert!(fixture.api.rerank_calls.load(Ordering::Relaxed) >= 3);
}
