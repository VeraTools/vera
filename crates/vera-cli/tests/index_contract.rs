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

const SUCCESS: usize = 0;
const RETRY_ONCE: usize = 1;
const ALWAYS_FAIL: usize = 2;
const SLOW: usize = 3;
const PERMANENT_WITH_SUCCESSFUL_SIBLING: usize = 4;

struct MockApi {
    url: String,
    stop: Arc<AtomicBool>,
    mode: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
}

impl MockApi {
    fn new(mode: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let mode = Arc::new(AtomicUsize::new(mode));
        let stopped = stop.clone();
        let response_mode = mode.clone();
        let thread = thread::spawn(move || {
            let mut calls = 0;
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
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
                assert!(headers.starts_with("POST /v1/embeddings "));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < header_end + length {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                calls += 1;
                let mode = response_mode.load(Ordering::Relaxed);
                let permanent = mode == PERMANENT_WITH_SUCCESSFUL_SIBLING
                    && body["input"][0].as_str().unwrap().contains("example_0");
                let (status, response) =
                    if permanent || mode == ALWAYS_FAIL || (mode == RETRY_ONCE && calls == 1) {
                        (
                            if permanent {
                                "400 Bad Request"
                            } else if mode == RETRY_ONCE {
                                "503 Service Unavailable"
                            } else {
                                "500 Internal Server Error"
                            },
                            json!({"error": "mock embedding failure"}),
                        )
                    } else {
                        if mode == SLOW {
                            thread::sleep(Duration::from_secs(12));
                        }
                        let count = body["input"].as_array().unwrap().len();
                        (
                            "200 OK",
                            json!({"data": (0..count).map(|index| json!({
                        "index": index, "embedding": [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
                    })).collect::<Vec<_>>() }),
                        )
                    };
                let response = response.to_string();
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            }
        });
        Self {
            url: format!("http://{address}/v1"),
            stop,
            mode,
            thread: Some(thread),
        }
    }
}

impl Drop for MockApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let address = self
            .url
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/v1")
            .unwrap();
        let _ = TcpStream::connect(address);
        self.thread.take().unwrap().join().unwrap();
    }
}

struct Fixture {
    dir: TempDir,
    repo: PathBuf,
    api: MockApi,
}

impl Fixture {
    fn new(mode: usize) -> Self {
        let scratch = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        let dir = tempfile::tempdir_in(scratch).unwrap();
        let repo = dir.path().join("repo");
        for path in [&repo, &dir.path().join("home"), &dir.path().join("scratch")] {
            std::fs::create_dir_all(path).unwrap();
        }
        for index in 0..3 {
            std::fs::write(
                repo.join(format!("file_{index}.rs")),
                format!("pub fn example_{index}() {{}}\n"),
            )
            .unwrap();
        }
        let fixture = Self {
            dir,
            repo,
            api: MockApi::new(mode),
        };
        fixture.success(&[
            "config",
            "set",
            "embedding.max_retries",
            if mode == RETRY_ONCE { "1" } else { "0" },
        ]);
        fixture
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_vera"))
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.dir.path().join("home"))
            .env("VERA_HOME", self.dir.path().join("vera-home"))
            .env("TMPDIR", self.dir.path().join("scratch"))
            .env("VERA_NO_UPDATE_CHECK", "1")
            .env("VERA_BACKEND", "api")
            .env("VERA_LOG", "error")
            .env("EMBEDDING_MODEL_BASE_URL", &self.api.url)
            .env("EMBEDDING_MODEL_ID", "contract-embedding")
            .env("EMBEDDING_MODEL_API_KEY", "test-key")
            .args(args)
            .output()
            .unwrap()
    }

    fn success(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn change_file(&self) {
        std::fs::write(
            self.repo.join("file_0.rs"),
            "pub fn changed_example() { let value = 1; }\n",
        )
        .unwrap();
    }
}

fn summary(output: &Output, old_fields: &[&str]) -> Value {
    // from_slice rejects extra JSON values and any non-JSON stdout decoration.
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.is_object());
    for field in old_fields {
        assert!(value.get(*field).is_some(), "missing {field}");
    }
    for field in [
        "embedding_requests",
        "embedding_retries",
        "embedding_timeouts",
        "embedding_failed_batches",
    ] {
        assert!(value[field].as_u64().is_some(), "missing {field}");
    }
    let phases = value["phase_secs"].as_object().unwrap();
    for stage in ["discovery", "parse", "embed", "store"] {
        let seconds = phases[stage].as_f64().unwrap();
        assert!(seconds >= 0.0 && seconds.is_finite());
        assert!((seconds * 1000.0 - (seconds * 1000.0).round()).abs() < 1e-8);
    }
    value
}

const INDEX_FIELDS: &[&str] = &[
    "files_parsed",
    "chunks_created",
    "embeddings_generated",
    "embeddings_reused",
    "binary_skipped",
    "large_skipped",
    "large_skipped_paths",
    "error_skipped",
    "files_with_tree_sitter_errors",
    "files_using_tier0_fallback",
    "parse_errors",
    "elapsed_secs",
];
const UPDATE_FIELDS: &[&str] = &[
    "files_modified",
    "files_added",
    "files_deleted",
    "files_unchanged",
    "files_with_tree_sitter_errors",
    "files_using_tier0_fallback",
    "parse_errors",
    "files_deferred",
    "total_chunks",
    "embeddings_reused",
    "elapsed_secs",
];

fn assert_failure(output: Output, operation: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<_> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with(&format!("Error: {operation} failed:")),
        "{stderr}"
    );
    assert!(lines.last().unwrap().contains("mock embedding failure"));
    let stats: Vec<_> = lines[..lines.len() - 1]
        .iter()
        .filter(|line| line.starts_with("embedding stats:"))
        .collect();
    assert_eq!(stats.len(), 1, "{stderr}");
    for expected in [
        "3 requests",
        "2 retries",
        "0 timeouts",
        "3 failed batches",
        "0/",
    ] {
        assert!(stats[0].contains(expected), "{stderr}");
    }
}

#[test]
fn api_index_json_preserves_old_fields_and_adds_request_stats_and_phase_times() {
    let fixture = Fixture::new(SUCCESS);
    let output = fixture.success(&["index", ".", "--json"]);
    let value = summary(&output, INDEX_FIELDS);
    assert!(value["embedding_requests"].as_u64().unwrap() >= 1);
    assert_eq!(value["embedding_retries"], 0);
    assert!(output.stderr.is_empty(), "fast runs must stay silent");
}

#[test]
fn transient_503_is_counted_as_an_embedding_retry_in_index_json() {
    let fixture = Fixture::new(RETRY_ONCE);
    let output = fixture.success(&["index", ".", "--json"]);
    let value = summary(&output, INDEX_FIELDS);
    assert!(value["embedding_retries"].as_u64().unwrap() >= 1);
    assert_eq!(value["embedding_requests"], 2);
}

#[test]
fn exhausted_index_has_empty_stdout_and_stats_before_the_last_error_line() {
    let fixture = Fixture::new(ALWAYS_FAIL);
    assert_failure(fixture.run(&["index", ".", "--json"]), "indexing");
}

#[test]
fn api_update_json_adds_request_stats_and_noop_update_resets_them_to_zero() {
    let fixture = Fixture::new(SUCCESS);
    fixture.success(&["index", ".", "--json", "--no-progress"]);
    fixture.change_file();
    let output = fixture.success(&["update", ".", "--json"]);
    let value = summary(&output, UPDATE_FIELDS);
    assert!(value["embedding_requests"].as_u64().unwrap() >= 1);
    assert!(value["phase_secs"]["classification"].is_number());
    let output = fixture.success(&["update", ".", "--json"]);
    let value = summary(&output, UPDATE_FIELDS);
    assert_eq!(value["embedding_requests"], 0);
    assert_eq!(value["embedding_retries"], 0);
    assert_eq!(value["phase_secs"]["embed"], 0.0);
    assert!(output.stderr.is_empty());
}

#[test]
fn exhausted_update_reports_failure_stats_even_with_no_progress() {
    let fixture = Fixture::new(SUCCESS);
    fixture.success(&["index", ".", "--json", "--no-progress"]);
    fixture.change_file();
    fixture.api.mode.store(ALWAYS_FAIL, Ordering::Relaxed);
    assert_failure(
        fixture.run(&["update", ".", "--json", "--no-progress"]),
        "update",
    );
}

#[test]
fn failure_stats_include_successful_siblings_of_a_permanent_failure() {
    let fixture = Fixture::new(PERMANENT_WITH_SUCCESSFUL_SIBLING);
    fixture.success(&["config", "set", "embedding.batch_size", "1"]);
    fixture.success(&["config", "set", "embedding.max_concurrent_requests", "2"]);
    let output = fixture.run(&["index", ".", "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("2 requests, 0 retries, 0 timeouts, 1 failed batches, 1/3 chunks embedded"),
        "{stderr}"
    );
    assert!(stderr.lines().last().unwrap().starts_with("Error:"));
}

#[test]
fn slow_non_tty_json_index_prints_plain_periodic_and_final_embedding_lines() {
    let fixture = Fixture::new(SLOW);
    let output = fixture.success(&["index", ".", "--json"]);
    summary(&output, INDEX_FIELDS);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains('\u{1b}'));
    let lines: Vec<_> = stderr.lines().collect();
    assert_eq!(lines.len(), 2, "{stderr}");
    assert!(lines[0].starts_with("embedding 0/3 chunks,"), "{stderr}");
    assert!(lines[1].starts_with("embedding 3/3 chunks,"), "{stderr}");
    for line in lines {
        assert!(line.contains("chunks/s, ETA") && line.ends_with("0 retries, 0 timeouts"));
    }
}
