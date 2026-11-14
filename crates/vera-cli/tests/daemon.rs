#![cfg(unix)]

use std::fs;
use std::io::Read;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct RunningDaemon(Child);

impl RunningDaemon {
    fn stop(&mut self) {
        if self.0.try_wait().expect("check daemon status").is_none() {
            // SAFETY: the child PID is valid while the Child is running.
            let result = unsafe { libc::kill(self.0.id() as i32, libc::SIGINT) };
            assert_eq!(result, 0, "send Ctrl-C to daemon");
            let status = self.0.wait().expect("wait for daemon shutdown");
            assert!(status.success(), "daemon exited with {status}");
        }
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            self.0.kill().expect("kill test daemon");
            self.0.wait().expect("reap test daemon");
        }
    }
}

fn assert_search(binary: &Path, home: &Path, runtime: &Path, repo: &Path, expected_mode: &str) {
    let output = Command::new(binary)
        .arg("-C")
        .arg(repo)
        .args(["search", "persistent_daemon_marker", "--json"])
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", runtime)
        .output()
        .expect("run vera search");
    assert!(
        output.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected_mode),
        "search stderr did not indicate {expected_mode:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse JSON search results");
    assert!(
        results.to_string().contains("persistent_daemon_marker"),
        "search did not return the indexed marker: {results}"
    );
}

#[test]
fn cli_search_uses_daemon_and_falls_back_when_it_is_absent() {
    let temp = tempfile::tempdir().expect("create temporary test directory");
    let repo = temp.path().join("repo");
    let runtime = temp.path().join("runtime");
    let home = temp.path().join("home");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(
        repo.join("marker.rs"),
        "pub fn persistent_daemon_marker() {}\n",
    )
    .unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_vera"));

    let index = Command::new(binary)
        .arg("index")
        .arg(&repo)
        .args(["--full-text-only", "--no-progress", "--json"])
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .output()
        .expect("index test repository");
    assert!(
        index.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    assert_search(binary, &home, &runtime, &repo, "vera: local");

    let mut daemon = RunningDaemon(
        Command::new(binary)
            .arg("daemon")
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start daemon"),
    );
    let socket_path = runtime.join("vera/daemon.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if fs::symlink_metadata(&socket_path).is_ok_and(|metadata| metadata.file_type().is_socket())
        {
            break;
        }
        if daemon.0.try_wait().expect("check daemon status").is_some() {
            let mut stderr = String::new();
            if let Some(mut pipe) = daemon.0.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            panic!("daemon exited before listening: {stderr}");
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        fs::symlink_metadata(&socket_path).is_ok_and(|metadata| metadata.file_type().is_socket()),
        "daemon socket did not appear"
    );

    assert_search(binary, &home, &runtime, &repo, "vera: daemon");
    assert_search(binary, &home, &runtime, &repo, "vera: daemon");
    daemon.stop();
    let mut daemon_logs = String::new();
    daemon
        .0
        .stderr
        .as_mut()
        .expect("capture daemon logs")
        .read_to_string(&mut daemon_logs)
        .expect("read daemon logs");
    assert!(
        daemon_logs
            .matches("search request: queries=1, indexes=1")
            .count()
            >= 2,
        "daemon should log both incoming searches: {daemon_logs}"
    );
    assert!(
        !socket_path.exists(),
        "daemon socket should be removed after shutdown"
    );

    assert_search(binary, &home, &runtime, &repo, "vera: local");
}
