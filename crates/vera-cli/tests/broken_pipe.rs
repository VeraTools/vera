#![cfg(unix)]

use std::os::unix::process::ExitStatusExt;
use std::process::Command;

#[test]
fn closed_stdout_ends_quietly_instead_of_panicking() {
    let temp = tempfile::tempdir().unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_vera"))
        .current_dir(temp.path())
        .env("VERA_HOME", temp.path().join("vera-home"))
        .arg("config")
        .stdout(writer)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(output.status.signal(), Some(libc::SIGPIPE));
}
